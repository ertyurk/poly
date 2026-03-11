use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::time::{self, Duration};

use crate::cli::{AssetFilter, WindowFilter};
use crate::polymarket::types::ParsedBook;
use crate::polymarket::PolymarketClient;
use crate::types::*;

/// Discovers real Polymarket crypto prediction markets, fetches live order books,
/// and detects market resolution. Replaces the simulator for real market data.
pub struct MarketFetcher {
    client: PolymarketClient,
    asset_filter: AssetFilter,
    window_filter: WindowFilter,
    poll_interval: Duration,
    book_refresh: Duration,
    max_spread: f64,
}

/// Tracked market state for the fetcher.
struct TrackedMarket {
    market_id: String,
    condition_id: String,
    asset: Asset,
    window: Window,
    token_yes: String,
    token_no: String,
    resolution_ts: TsMicros,
    open_ts: TsMicros,
    open_price: Option<f64>,
    volume_24h: f64,
    market_type: MarketType,
    resolved: bool,
    event_slug: String,
}

impl MarketFetcher {
    pub const fn new(
        client: PolymarketClient,
        asset_filter: AssetFilter,
        window_filter: WindowFilter,
        poll_interval_secs: u64,
        max_spread: f64,
    ) -> Self {
        Self {
            client,
            asset_filter,
            window_filter,
            poll_interval: Duration::from_secs(poll_interval_secs),
            book_refresh: Duration::from_secs(5),
            max_spread,
        }
    }

    pub async fn run(
        &self,
        market_tx_signal: mpsc::Sender<MarketState>,
        market_tx_decision: mpsc::Sender<MarketState>,
        settle_tx: mpsc::Sender<SettleCommand>,
        db_tx: mpsc::Sender<DbEvent>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
        restored_markets: Vec<crate::db::queries::RestoredMarket>,
    ) {
        let mut tracked: HashMap<String, TrackedMarket> = HashMap::new();
        let mut skipped: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Seed tracked map with markets from restored open positions so they can be
        // resolved even if they no longer appear in discovery.
        for rm in restored_markets {
            let asset = match rm.asset.as_str() {
                "BTC" => Asset::BTC,
                "ETH" => Asset::ETH,
                _ => continue,
            };
            let window = match rm.window.as_str() {
                "5m" => Window::FiveMin,
                "15m" => Window::FifteenMin,
                "1h" => Window::Hourly,
                "1d" => Window::Daily,
                _ => continue,
            };
            tracing::info!(
                market = %rm.market_id,
                condition_id = %rm.condition_id,
                "seeded tracked market from restored position"
            );
            tracked.insert(
                rm.condition_id.clone(),
                TrackedMarket {
                    market_id: rm.market_id,
                    condition_id: rm.condition_id,
                    asset,
                    window,
                    token_yes: rm.token_yes,
                    token_no: rm.token_no,
                    resolution_ts: rm.resolution_ts,
                    open_ts: rm.open_ts,
                    open_price: rm.open_price,
                    volume_24h: 0.0,
                    market_type: MarketType::UpDown,
                    resolved: false,
                    event_slug: String::new(),
                },
            );
        }

        let mut discovery_tick = time::interval(self.poll_interval);
        let mut book_tick = time::interval(self.book_refresh);

        // Consume initial ticks
        discovery_tick.tick().await;
        book_tick.tick().await;

        // Initial discovery
        self.discover_markets(&mut tracked, &mut skipped, &market_tx_signal, &market_tx_decision, &db_tx)
            .await;

        loop {
            tokio::select! {
                biased;

                _ = shutdown.changed() => {
                    tracing::info!("market fetcher shutting down");
                    return;
                }

                _ = discovery_tick.tick() => {
                    self.discover_markets(&mut tracked, &mut skipped, &market_tx_signal, &market_tx_decision, &db_tx).await;
                    self.check_resolutions(&mut tracked, &settle_tx, &db_tx).await;
                }

                _ = book_tick.tick() => {
                    self.refresh_books(&tracked, &market_tx_decision, &db_tx).await;
                }
            }
        }
    }

    async fn discover_markets(
        &self,
        tracked: &mut HashMap<String, TrackedMarket>,
        skipped: &mut std::collections::HashSet<String>,
        market_tx_signal: &mpsc::Sender<MarketState>,
        market_tx_decision: &mpsc::Sender<MarketState>,
        db_tx: &mpsc::Sender<DbEvent>,
    ) {
        let markets = match self.client.fetch_markets().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "failed to fetch markets from Gamma API");
                return;
            }
        };

        let now = now_micros();

        for gm in &markets {
            let question = gm.question.as_deref().unwrap_or("?");
            let condition_id = match gm.condition_id.as_deref() {
                Some(id) if !id.is_empty() => id,
                _ => continue,
            };

            // Skip already-tracked or previously-skipped markets
            if tracked.contains_key(condition_id) || skipped.contains(condition_id) {
                continue;
            }

            // Skip closed markets
            if gm.closed.unwrap_or(false) {
                continue;
            }

            // Determine asset from tags or question
            let Some(asset) = detect_asset(gm) else {
                continue;
            };
            if !self.asset_filter.matches(asset) {
                continue;
            }

            // Determine window from end_date
            let Some(resolution_ts) = parse_end_date(gm) else {
                tracing::debug!(question, "skipped: no end_date");
                continue;
            };
            let secs_until = (resolution_ts - now) / 1_000_000;
            if secs_until < 60 {
                tracing::debug!(question, secs_until, "skipped: resolves too soon");
                continue;
            }
            // Skip markets resolving more than 48 hours out — our short-term
            // Binance signal has no predictive power for weekly/monthly/yearly markets.
            if secs_until > 48 * 3600 {
                continue;
            }
            let window = classify_window(secs_until);
            if !self.window_filter.matches(window) {
                continue;
            }

            // Extract tokens (handles both tokens array and clobTokenIds string)
            let (token_yes, token_no) = extract_market_tokens(gm);
            if token_yes.is_empty() || token_no.is_empty() {
                tracing::debug!(question, "skipped: no tokens");
                continue;
            }

            // Fetch order book for the positive token to get initial prices
            let book = match self.client.fetch_order_book(&token_yes).await {
                Ok(b) => ParsedBook::from_response(&b),
                Err(e) => {
                    tracing::debug!(question, error = %e, "skipped: book fetch failed");
                    continue;
                }
            };

            // Skip illiquid markets (spread exceeds configured max or no real order book)
            if book.spread > self.max_spread
                || (book.best_bid < f64::EPSILON
                    && book.best_ask > 1.0 - f64::EPSILON)
            {
                tracing::debug!(
                    question,
                    spread = book.spread,
                    bid = book.best_bid,
                    ask = book.best_ask,
                    "skipped: illiquid"
                );
                continue;
            }

            let volume = gm
                .volume
                .as_deref()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
                // Fresh up/down markets often have 0 volume; use a floor so the
                // stealth cap doesn't zero out every trade on new markets.
                .max(10_000.0);

            // Get open price from tokens array or outcomePrices
            let open_price = gm
                .tokens
                .as_ref()
                .and_then(|tokens| {
                    tokens
                        .iter()
                        .find(|t| matches!(t.outcome.as_deref(), Some("Yes" | "Up")))
                        .and_then(|t| t.price)
                })
                .or_else(|| {
                    // Parse from outcomePrices JSON string (first element is positive outcome)
                    gm.outcome_prices
                        .as_deref()
                        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                        .and_then(|v| v.first().and_then(|p| p.parse::<f64>().ok()))
                });

            let market_type = parse_market_type(question);

            // Only trade UpDown markets — our signal model is tuned for directional bets.
            // "Above $X" / "Between" markets need a different edge source.
            if !matches!(market_type, MarketType::UpDown) {
                continue;
            }

            // Skip UpDown markets resolving > 5 hours out (focus on 5m/15m/1h/4h)
            if secs_until > 5 * 3600 {
                continue;
            }

            let market_id = format!(
                "{asset}_{window}_{cid}",
                cid = &condition_id[..condition_id.len().min(8)]
            );

            // Use actual market start time from slug for UpDown markets;
            // fall back to discovery time for other market types.
            let open_ts = if matches!(market_type, MarketType::UpDown) {
                match parse_open_ts_from_slug(gm) {
                    Some(ts) => ts,
                    None => {
                        tracing::warn!(
                            market_id = %market_id,
                            slug = ?gm.slug,
                            "failed to parse open_ts from slug, skipping market"
                        );
                        skipped.insert(condition_id.to_string());
                        continue;
                    }
                }
            } else {
                now
            };

            let event_slug = gm.event_slug.clone().unwrap_or_default();

            let ms = MarketState {
                market_id: market_id.clone(),
                condition_id: condition_id.to_string(),
                asset,
                window,
                token_yes: token_yes.clone(),
                token_no: token_no.clone(),
                best_bid: book.best_bid,
                best_ask: book.best_ask,
                midpoint: book.midpoint,
                resolution_ts,
                open_ts,
                open_price,
                volume_24h: volume,
                market_type,
                event_slug: event_slug.clone(),
            };

            let _ = market_tx_signal.send(ms.clone()).await;
            let _ = market_tx_decision.send(ms.clone()).await;
            let _ = db_tx.try_send(DbEvent::Market(ms));

            tracked.insert(
                condition_id.to_string(),
                TrackedMarket {
                    market_id: market_id.clone(),
                    condition_id: condition_id.to_string(),
                    asset,
                    window,
                    token_yes,
                    token_no,
                    resolution_ts,
                    open_ts,
                    open_price,
                    volume_24h: volume,
                    market_type,
                    resolved: false,
                    event_slug,
                },
            );

            tracing::info!(
                market = %market_id,
                question = gm.question.as_deref().unwrap_or("?"),
                midpoint = book.midpoint,
                "discovered market"
            );
        }
    }

    async fn refresh_books(
        &self,
        tracked: &HashMap<String, TrackedMarket>,
        market_tx_decision: &mpsc::Sender<MarketState>,
        db_tx: &mpsc::Sender<DbEvent>,
    ) {
        for tm in tracked.values() {
            if tm.resolved {
                continue;
            }

            let book = match self.client.fetch_order_book(&tm.token_yes).await {
                Ok(b) => ParsedBook::from_response(&b),
                Err(_) => continue,
            };

            let ms = MarketState {
                market_id: tm.market_id.clone(),
                condition_id: tm.condition_id.clone(),
                asset: tm.asset,
                window: tm.window,
                token_yes: tm.token_yes.clone(),
                token_no: tm.token_no.clone(),
                best_bid: book.best_bid,
                best_ask: book.best_ask,
                midpoint: book.midpoint,
                resolution_ts: tm.resolution_ts,
                open_ts: tm.open_ts,
                open_price: tm.open_price,
                volume_24h: tm.volume_24h,
                market_type: tm.market_type,
                event_slug: tm.event_slug.clone(),
            };

            let _ = market_tx_decision.send(ms).await;

            let _ = db_tx.try_send(DbEvent::BookSnapshot {
                market_id: tm.market_id.clone(),
                best_bid: book.best_bid,
                best_ask: book.best_ask,
                midpoint: book.midpoint,
                spread: book.spread,
                ts: now_micros(),
            });
        }
    }

    async fn check_resolutions(
        &self,
        tracked: &mut HashMap<String, TrackedMarket>,
        settle_tx: &mpsc::Sender<SettleCommand>,
        db_tx: &mpsc::Sender<DbEvent>,
    ) {
        let now = now_micros();
        let mut to_resolve = Vec::new();

        for (cid, tm) in tracked.iter() {
            if tm.resolved {
                continue;
            }
            // Check if past resolution time or poll for resolution
            if now >= tm.resolution_ts {
                to_resolve.push(cid.clone());
            }
        }

        for cid in to_resolve {
            let resolved_side = match self.client.fetch_market_for_resolution(&cid).await {
                Ok(Some(cm)) => {
                    // Use the authoritative `winner` flag from CLOB API tokens
                    if let Some(side) = determine_outcome_from_clob(&cm) {
                        side
                    } else {
                        // Market exists but no winner yet — retry next cycle
                        tracing::info!(
                            condition_id = %cid,
                            "no winner flag set yet, deferring settlement"
                        );
                        continue;
                    }
                }
                Ok(None) => {
                    tracing::info!(condition_id = %cid, "CLOB market not found for resolution");
                    continue;
                }
                Err(e) => {
                    tracing::warn!(condition_id = %cid, error = %e, "CLOB resolution fetch failed");
                    continue;
                }
            };

            if let Some(tm) = tracked.get_mut(&cid) {
                tm.resolved = true;

                tracing::info!(
                    market = %tm.market_id,
                    outcome = %resolved_side,
                    "market resolved"
                );

                let _ = db_tx.try_send(DbEvent::MarketResolution {
                    market_id: tm.market_id.clone(),
                    resolved_side: resolved_side.to_string(),
                });

                let _ = settle_tx
                    .send(SettleCommand {
                        market_id: tm.market_id.clone(),
                        resolved_side,
                        resolved_ts: now_micros(),
                    })
                    .await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn detect_asset(gm: &crate::polymarket::types::GammaMarket) -> Option<Asset> {
    let check = |s: &str| -> Option<Asset> {
        let lower = s.to_lowercase();
        if lower.contains("btc") || lower.contains("bitcoin") {
            Some(Asset::BTC)
        } else if lower.contains("ethereum") {
            Some(Asset::ETH)
        } else {
            None
        }
    };

    // Check tags first
    if let Some(tags) = &gm.tags {
        for tag in tags {
            if let Some(a) = check(tag) {
                return Some(a);
            }
        }
    }

    // Check question
    if let Some(q) = &gm.question {
        return check(q);
    }

    None
}

fn parse_end_date(gm: &crate::polymarket::types::GammaMarket) -> Option<TsMicros> {
    let end_date = gm.end_date.as_deref()?;
    let dt = chrono::DateTime::parse_from_rfc3339(end_date).ok()?;
    Some(dt.timestamp_micros())
}

const fn classify_window(secs_until_resolution: i64) -> Window {
    if secs_until_resolution <= 600 {
        Window::FiveMin
    } else if secs_until_resolution <= 1800 {
        Window::FifteenMin
    } else if secs_until_resolution <= 7200 {
        Window::Hourly
    } else {
        Window::Daily
    }
}

/// Extract (positive_token, negative_token) from a market.
///
/// Handles both:
/// - `tokens` array with outcome "Yes"/"No" or "Up"/"Down"
/// - `clobTokenIds` JSON string + `outcomes` JSON string (when tokens array is absent)
fn extract_market_tokens(gm: &crate::polymarket::types::GammaMarket) -> (String, String) {
    // Try the tokens array first
    if let Some(tokens) = &gm.tokens {
        if tokens.len() >= 2 {
            let mut yes_id = String::new();
            let mut no_id = String::new();
            for t in tokens {
                match t.outcome.as_deref() {
                    Some("Yes" | "Up") => {
                        if let Some(id) = &t.token_id {
                            yes_id.clone_from(id);
                        }
                    }
                    Some("No" | "Down") => {
                        if let Some(id) = &t.token_id {
                            no_id.clone_from(id);
                        }
                    }
                    _ => {}
                }
            }
            if !yes_id.is_empty() && !no_id.is_empty() {
                return (yes_id, no_id);
            }
        }
    }

    // Fallback: parse clobTokenIds + outcomes JSON strings
    let token_ids: Vec<String> = gm
        .clob_token_ids
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let outcomes: Vec<String> = gm
        .outcomes
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    if token_ids.len() >= 2 && outcomes.len() >= 2 {
        // First outcome is positive (Yes/Up), second is negative (No/Down)
        return (token_ids[0].clone(), token_ids[1].clone());
    }

    (String::new(), String::new())
}

/// Parse market type from question text to determine what p_hat should represent.
///
/// Examples:
///   "Will the price of Bitcoin be above $78,000 on March 9?" → Above(78000)
///   "Will the price of Bitcoin be greater than $78,000..." → Above(78000)
///   "Will the price of Bitcoin be less than $60,000..." → Below(60000)
///   "Will the price of Bitcoin be between $68,000 and $70,000..." → Between(68000, 70000)
///   "Bitcoin Up or Down on March 9?" → UpDown
fn parse_market_type(question: &str) -> MarketType {
    let lower = question.to_lowercase();

    if lower.contains("up or down") {
        return MarketType::UpDown;
    }

    if lower.contains("between") {
        let amounts = extract_dollar_amounts(question);
        if amounts.len() >= 2 {
            let lo = amounts[0].min(amounts[1]);
            let hi = amounts[0].max(amounts[1]);
            return MarketType::Between(lo, hi);
        }
    }

    if lower.contains("above") || lower.contains("greater than") {
        let amounts = extract_dollar_amounts(question);
        if let Some(&strike) = amounts.first() {
            return MarketType::Above(strike);
        }
    }

    if lower.contains("less than") || lower.contains("below") {
        let amounts = extract_dollar_amounts(question);
        if let Some(&strike) = amounts.first() {
            return MarketType::Below(strike);
        }
    }

    // Fallback: treat as UpDown (safest default)
    MarketType::UpDown
}

/// Parse the market open timestamp from the event slug.
///
/// Slug format: `{asset}-updown-{window}-{start_unix_ts}`
/// e.g., "btc-updown-5m-1741579500"
fn parse_open_ts_from_slug(gm: &crate::polymarket::types::GammaMarket) -> Option<TsMicros> {
    let slug = gm.slug.as_deref()?;
    // Slug ends with a unix timestamp after the last '-'
    let last_part = slug.rsplit('-').next()?;
    let unix_secs: i64 = last_part.parse().ok()?;
    // Sanity: must be a reasonable timestamp (after 2020, before 2030)
    if unix_secs < 1_577_836_800 || unix_secs > 1_893_456_000 {
        return None;
    }
    Some(unix_secs * 1_000_000)
}

/// Extract dollar amounts from text. E.g., "$78,000" → 78000.0
fn extract_dollar_amounts(text: &str) -> Vec<f64> {
    let mut amounts = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            i += 1;
            let mut num_str = String::new();
            while i < chars.len()
                && (chars[i].is_ascii_digit() || chars[i] == ',' || chars[i] == '.')
            {
                if chars[i] != ',' {
                    num_str.push(chars[i]);
                }
                i += 1;
            }
            if let Ok(val) = num_str.parse::<f64>() {
                amounts.push(val);
            }
        } else {
            i += 1;
        }
    }
    amounts
}

/// Determine which side won using the CLOB API's authoritative `winner` flag.
fn determine_outcome_from_clob(cm: &crate::polymarket::types::ClobMarket) -> Option<Side> {
    let tokens = cm.tokens.as_ref()?;
    for t in tokens {
        if t.winner == Some(true) {
            let outcome = t.outcome.as_deref().unwrap_or("");
            let side = match outcome {
                "Yes" | "Up" => Side::Yes,
                "No" | "Down" => Side::No,
                _ => {
                    tracing::warn!(
                        outcome = outcome,
                        "unknown winning outcome label"
                    );
                    continue;
                }
            };
            return Some(side);
        }
    }
    // No token has winner=true yet
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polymarket::types::{ClobMarket, ClobToken};

    fn make_clob_market(tokens: Vec<ClobToken>) -> ClobMarket {
        ClobMarket {
            tokens: Some(tokens),
        }
    }

    fn token(outcome: &str, winner: Option<bool>) -> ClobToken {
        ClobToken {
            outcome: Some(outcome.into()),
            winner,
        }
    }

    // --- winner flag tests (CLOB API) ---

    #[test]
    fn clob_up_wins() {
        let cm = make_clob_market(vec![token("Up", Some(true)), token("Down", Some(false))]);
        assert_eq!(determine_outcome_from_clob(&cm), Some(Side::Yes));
    }

    #[test]
    fn clob_down_wins() {
        let cm = make_clob_market(vec![token("Up", Some(false)), token("Down", Some(true))]);
        assert_eq!(determine_outcome_from_clob(&cm), Some(Side::No));
    }

    #[test]
    fn clob_yes_wins() {
        let cm = make_clob_market(vec![token("Yes", Some(true)), token("No", Some(false))]);
        assert_eq!(determine_outcome_from_clob(&cm), Some(Side::Yes));
    }

    #[test]
    fn clob_no_wins() {
        let cm = make_clob_market(vec![token("Yes", Some(false)), token("No", Some(true))]);
        assert_eq!(determine_outcome_from_clob(&cm), Some(Side::No));
    }

    #[test]
    fn clob_reversed_order_up_wins() {
        // Tokens in reversed order — still works because we check by name
        let cm = make_clob_market(vec![token("Down", Some(false)), token("Up", Some(true))]);
        assert_eq!(determine_outcome_from_clob(&cm), Some(Side::Yes));
    }

    #[test]
    fn clob_no_winner_yet() {
        // No winner flag set — should return None (retry later)
        let cm = make_clob_market(vec![token("Up", None), token("Down", None)]);
        assert_eq!(determine_outcome_from_clob(&cm), None);
    }

    #[test]
    fn clob_no_winner_false_both() {
        // Both false — market resolved but no winner? Return None
        let cm =
            make_clob_market(vec![token("Up", Some(false)), token("Down", Some(false))]);
        assert_eq!(determine_outcome_from_clob(&cm), None);
    }

    #[test]
    fn clob_no_tokens() {
        let cm = ClobMarket {
            tokens: None,
        };
        assert_eq!(determine_outcome_from_clob(&cm), None);
    }
}
