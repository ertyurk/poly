use reqwest::Client;

use super::types::*;

/// Polymarket API client for Gamma (market discovery) and CLOB (order book reading).
/// Order placement is handled by `LiveTrader` via the official SDK.
pub struct PolymarketClient {
    http: Client,
    gamma_url: String,
    clob_url: String,
}

impl PolymarketClient {
    /// Create a read-only client for market discovery and order book queries.
    pub fn new(gamma_url: &str, clob_url: &str) -> Self {
        Self {
            http: Client::new(),
            gamma_url: gamma_url.trim_end_matches('/').to_string(),
            clob_url: clob_url.trim_end_matches('/').to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Gamma API — market discovery (no auth needed)
    // -----------------------------------------------------------------------

    /// Fetch active crypto price prediction markets from the Gamma Events API.
    /// Returns flattened markets from all matching events.
    ///
    /// Two strategies:
    /// 1. Volume-sorted query for established daily/weekly crypto markets.
    /// 2. Deterministic slug-based lookups for short-term Up/Down markets
    ///    (5m, 15m, 4h) which never rank high enough by volume.
    pub async fn fetch_markets(
        &self,
    ) -> Result<Vec<GammaMarket>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "{}/events?tag=crypto&active=true&closed=false&limit=100&order=volume24hr&ascending=false",
            self.gamma_url
        );
        let resp = self.http.get(&url).send().await?;
        let mut markets = Self::parse_event_response(resp).await?;

        // Fetch short-term up/down markets by computed slug
        let updown = self.fetch_updown_markets().await;
        if !updown.is_empty() {
            let seen: std::collections::HashSet<String> = markets
                .iter()
                .filter_map(|m| m.condition_id.clone())
                .collect();
            let new_count = updown
                .iter()
                .filter(|m| {
                    m.condition_id
                        .as_deref()
                        .map_or(false, |id| !seen.contains(id))
                })
                .count();
            if new_count > 0 {
                tracing::info!(
                    count = new_count,
                    "discovered up/down markets via slug lookup"
                );
            }
            markets.extend(updown.into_iter().filter(|m| {
                m.condition_id
                    .as_deref()
                    .map_or(true, |id| !seen.contains(id))
            }));
        }

        Ok(markets)
    }

    /// Fetch short-term up/down markets by computing expected event slugs.
    ///
    /// Slug pattern: `{asset}-updown-{window}-{start_ts}`
    /// where start_ts is aligned to window boundaries (300s/900s/14400s).
    async fn fetch_updown_markets(&self) -> Vec<GammaMarket> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        const ASSETS: &[&str] = &["btc", "eth"];
        // (slug label, alignment in seconds, how many windows ahead to fetch)
        const WINDOWS: &[(&str, i64, i64)] = &[("5m", 300, 3), ("15m", 900, 3), ("4h", 14400, 2)];

        let mut slugs = Vec::new();
        for asset in ASSETS {
            for &(label, align, ahead) in WINDOWS {
                let current = (now / align) * align;
                // Start from -1 to include the previous window (may still be resolving)
                for i in -1..ahead {
                    slugs.push(format!("{asset}-updown-{label}-{}", current + i * align));
                }
            }
        }

        // Fetch all slugs concurrently
        let futs: Vec<_> = slugs
            .iter()
            .map(|slug| {
                let url = format!("{}/events?slug={slug}", self.gamma_url);
                self.http.get(&url).send()
            })
            .collect();
        let results = futures_util::future::join_all(futs).await;

        let mut markets = Vec::new();
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(resp) => match Self::parse_event_response(resp).await {
                    Ok(m) => markets.extend(m),
                    Err(e) => {
                        tracing::debug!(slug = %slugs[i], error = %e, "slug lookup failed");
                    }
                },
                Err(e) => {
                    tracing::debug!(slug = %slugs[i], error = %e, "slug request failed");
                }
            }
        }
        markets
    }

    async fn parse_event_response(
        resp: reqwest::Response,
    ) -> Result<Vec<GammaMarket>, Box<dyn std::error::Error + Send + Sync>> {
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Gamma API {status}: {body}").into());
        }
        let events: Vec<GammaEvent> = resp.json().await?;
        Ok(events
            .into_iter()
            .flat_map(|e| {
                let event_slug = e.slug.clone().unwrap_or_default();
                e.markets.unwrap_or_default().into_iter().map(move |mut m| {
                    m.event_slug = Some(event_slug.clone());
                    m
                })
            })
            .collect())
    }

    /// Fetch a market from the CLOB API to check resolution status.
    /// Uses `GET /markets/{condition_id}` which returns `tokens[].winner`
    /// — the authoritative resolution signal.
    pub async fn fetch_market_for_resolution(
        &self,
        condition_id: &str,
    ) -> Result<Option<ClobMarket>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/markets/{condition_id}", self.clob_url);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let market: ClobMarket = resp.json().await?;
        Ok(Some(market))
    }

    // -----------------------------------------------------------------------
    // CLOB API — order book (no auth needed)
    // -----------------------------------------------------------------------

    /// Fetch order book for a specific token.
    pub async fn fetch_order_book(
        &self,
        token_id: &str,
    ) -> Result<OrderBookResponse, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/book?token_id={token_id}", self.clob_url);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("CLOB book {status}: {body}").into());
        }
        let book: OrderBookResponse = resp.json().await?;
        Ok(book)
    }

    /// Fetch midpoint price for a specific token.
    #[allow(dead_code)]
    pub async fn fetch_midpoint(
        &self,
        token_id: &str,
    ) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/midpoint?token_id={token_id}", self.clob_url);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Ok(0.5); // fallback
        }
        let mid: MidpointResponse = resp.json().await?;
        Ok(mid.mid.and_then(|m| m.parse::<f64>().ok()).unwrap_or(0.5))
    }
}
