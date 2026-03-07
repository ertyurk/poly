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
                continue;
            };
            let secs_until = (resolution_ts - now) / 1_000_000;
            if secs_until < 60 {
                continue; // Skip markets resolving in < 1 minute
            }
            let window = classify_window(secs_until);
            if !self.window_filter.matches(window) {
                continue;
            }

            // Extract tokens
            let tokens = match gm.tokens.as_ref() {
                Some(t) if t.len() >= 2 => t,
                _ => continue,
            };
            let (token_yes, token_no) = extract_tokens(tokens);
            if token_yes.is_empty() || token_no.is_empty() {
                continue;
            }

            // Fetch order book for the YES token to get initial prices
            let book = match self.client.fetch_order_book(&token_yes).await {
                Ok(b) => ParsedBook::from_response(&b),
                Err(e) => {
                    tracing::debug!(error = %e, "failed to fetch book for {condition_id}");
                    continue;
                }
            };

            let volume = gm
                .volume
                .as_deref()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(10_000.0);

            let open_price = tokens
                .iter()
                .find(|t| t.outcome.as_deref() == Some("Yes"))
                .and_then(|t| t.price);

            let market_id = format!(
                "{asset}_{window}_{cid}",
                cid = &condition_id[..condition_id.len().min(8)]
            );

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
                open_ts: now,
                open_price,
                volume_24h: volume,
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
                    open_ts: now,
                    open_price,
                    volume_24h: volume,
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
                Ok(Some(gm)) if gm.closed.unwrap_or(false) => determine_outcome(&gm),
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
        } else if lower.contains("eth") || lower.contains("ethereum") {
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
    } else {
        Window::FifteenMin
    }
}

fn extract_tokens(tokens: &[crate::polymarket::types::GammaToken]) -> (String, String) {
    let mut yes_id = String::new();
    let mut no_id = String::new();

    for t in tokens {
        match t.outcome.as_deref() {
            Some("Yes") => {
                if let Some(id) = &t.token_id {
                    yes_id.clone_from(id);
                }
            }
            Some("No") => {
                if let Some(id) = &t.token_id {
                    no_id.clone_from(id);
                }
            }
            _ => {}
        }
    }

    (yes_id, no_id)
}

fn determine_outcome(gm: &crate::polymarket::types::GammaMarket) -> Side {
    // outcomePrices is typically "[1.0, 0.0]" for YES or "[0.0, 1.0]" for NO
    if let Some(prices_str) = &gm.outcome_prices {
        if let Ok(prices) = serde_json::from_str::<Vec<String>>(prices_str) {
            if let Some(yes_price) = prices.first().and_then(|p| p.parse::<f64>().ok()) {
                return if yes_price > 0.5 { Side::Yes } else { Side::No };
            }
        }
    }
    // Fallback: check token prices
    if let Some(tokens) = &gm.tokens {
        for t in tokens {
            if t.outcome.as_deref() == Some("Yes") {
                if let Some(price) = t.price {
                    return if price > 0.5 { Side::Yes } else { Side::No };
                }
            }
        }
    }
    Side::Yes // default
}
