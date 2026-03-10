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
}

/// Tracked market state for the fetcher.
struct TrackedMarket {
    market_id: String,
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
}

impl MarketFetcher {
    pub const fn new(
        client: PolymarketClient,
        asset_filter: AssetFilter,
        window_filter: WindowFilter,
        poll_interval_secs: u64,
    ) -> Self {
        Self {
            client,
            asset_filter,
            window_filter,
            poll_interval: Duration::from_secs(poll_interval_secs),
            book_refresh: Duration::from_secs(5),
        }
    }

    pub async fn run(
        &self,
        market_tx_signal: mpsc::Sender<MarketState>,
        market_tx_decision: mpsc::Sender<MarketState>,
        settle_tx: mpsc::Sender<SettleCommand>,
        db_tx: mpsc::Sender<DbEvent>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut tracked: HashMap<String, TrackedMarket> = HashMap::new();
        let mut discovery_tick = time::interval(self.poll_interval);
        let mut book_tick = time::interval(self.book_refresh);

        // Consume initial ticks
        discovery_tick.tick().await;
        book_tick.tick().await;

        // Initial discovery
        self.discover_markets(&mut tracked, &market_tx_signal, &market_tx_decision, &db_tx)
            .await;

        loop {
            tokio::select! {
                biased;

                _ = shutdown.changed() => {
                    tracing::info!("market fetcher shutting down");
                    return;
                }

                _ = discovery_tick.tick() => {
                    self.discover_markets(&mut tracked, &market_tx_signal, &market_tx_decision, &db_tx).await;
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

            // Skip already-tracked markets
            if tracked.contains_key(condition_id) {
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

            // Skip illiquid markets (spread > 10% or no real order book)
            if book.spread > 0.10
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
                            "failed to parse open_ts from slug, skipping market"
                        );
                        continue;
                    }
                }
            } else {
                now
            };

            let ms = MarketState {
                market_id: market_id.clone(),
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
            };

            let _ = market_tx_signal.send(ms.clone()).await;
            let _ = market_tx_decision.send(ms.clone()).await;
            let _ = db_tx.try_send(DbEvent::Market(ms));

            tracked.insert(
                condition_id.to_string(),
                TrackedMarket {
                    market_id: market_id.clone(),
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
            let resolved_side = match self.client.fetch_market_by_id(&cid).await {
                Ok(Some(gm)) if gm.closed.unwrap_or(false) => {
                    if let Some(side) = determine_outcome(&gm) {
                        side
                    } else {
                        tracing::warn!(
                            condition_id = %cid,
                            "cannot determine outcome from API response, skipping settlement"
                        );
                        continue;
                    }
                }
                Ok(Some(_)) => {
                    // Past resolution time but not closed yet — check again later
                    continue;
                }
                _ => {
                    // API error or not found — skip
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

fn determine_outcome(gm: &crate::polymarket::types::GammaMarket) -> Option<Side> {
    // outcomePrices is typically "[1.0, 0.0]" for YES or "[0.0, 1.0]" for NO
    if let Some(prices_str) = &gm.outcome_prices {
        if let Ok(prices) = serde_json::from_str::<Vec<String>>(prices_str) {
            if let Some(yes_price) = prices.first().and_then(|p| p.parse::<f64>().ok()) {
                return Some(if yes_price > 0.5 { Side::Yes } else { Side::No });
            }
        }
    }
    // Fallback: check token prices
    if let Some(tokens) = &gm.tokens {
        for t in tokens {
            if matches!(t.outcome.as_deref(), Some("Yes" | "Up")) {
                if let Some(price) = t.price {
                    return Some(if price > 0.5 { Side::Yes } else { Side::No });
                }
            }
        }
    }
    None
}
