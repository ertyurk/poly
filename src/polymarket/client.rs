use reqwest::Client;
use std::time::{SystemTime, UNIX_EPOCH};

use super::auth;
use super::signing::{self, OrderParams};
use super::types::*;

/// Polymarket API client for Gamma (market discovery) and CLOB (order book + trading).
pub struct PolymarketClient {
    http: Client,
    gamma_url: String,
    clob_url: String,
    // Auth credentials — only needed for order placement (real trading mode)
    api_key: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
    private_key: Option<String>,
    wallet_address: Option<String>,
}

impl PolymarketClient {
    /// Create a client for read-only access (paper trade mode).
    /// No API keys needed — public endpoints only.
    pub fn new_readonly(gamma_url: &str, clob_url: &str) -> Self {
        Self {
            http: Client::new(),
            gamma_url: gamma_url.trim_end_matches('/').to_string(),
            clob_url: clob_url.trim_end_matches('/').to_string(),
            api_key: None,
            api_secret: None,
            passphrase: None,
            private_key: None,
            wallet_address: None,
        }
    }

    /// Create a client with full trading credentials (real trading mode).
    pub fn new_authenticated(
        gamma_url: &str,
        clob_url: &str,
        api_key: String,
        api_secret: String,
        passphrase: String,
        private_key: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let wallet_address = signing::address_from_key(&private_key)?;
        Ok(Self {
            http: Client::new(),
            gamma_url: gamma_url.trim_end_matches('/').to_string(),
            clob_url: clob_url.trim_end_matches('/').to_string(),
            api_key: Some(api_key),
            api_secret: Some(api_secret),
            passphrase: Some(passphrase),
            private_key: Some(private_key),
            wallet_address: Some(wallet_address),
        })
    }

    #[allow(dead_code)]
    pub const fn is_authenticated(&self) -> bool {
        self.api_key.is_some() && self.private_key.is_some()
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
                tracing::info!(count = new_count, "discovered up/down markets via slug lookup");
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
        const WINDOWS: &[(&str, i64, i64)] = &[
            ("5m", 300, 3),
            ("15m", 900, 3),
            ("4h", 14400, 2),
        ];

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
            .flat_map(|e| e.markets.unwrap_or_default())
            .collect())
    }

    /// Fetch a specific market by condition ID to check resolution status.
    pub async fn fetch_market_by_id(
        &self,
        condition_id: &str,
    ) -> Result<Option<GammaMarket>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/markets?condition_id={condition_id}", self.gamma_url);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let mut markets: Vec<GammaMarket> = resp.json().await?;
        Ok(markets.pop())
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

    /// Fetch the raw `feeRateBps` for a token from the CLOB API.
    /// This is the opaque value required for order signing (typically 1000 for crypto).
    pub async fn fetch_fee_rate_bps(
        &self,
        token_id: &str,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/fee-rate?token_id={token_id}", self.clob_url);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("CLOB fee-rate {status}: {body}").into());
        }
        #[derive(serde::Deserialize)]
        struct FeeRateResp {
            base_fee: u64,
        }
        let fr: FeeRateResp = resp.json().await?;
        Ok(fr.base_fee)
    }

    // -----------------------------------------------------------------------
    // CLOB API — order placement (auth required)
    // -----------------------------------------------------------------------

    /// Place an order on Polymarket. Requires authenticated client.
    pub async fn place_order(
        &self,
        token_id: &str,
        side_buy: bool,
        price: f64,
        size: f64,
        fee_rate_bps: u64,
    ) -> Result<OrderResponse, Box<dyn std::error::Error + Send + Sync>> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or("missing POLYMARKET_API_KEY")?;
        let api_secret = self
            .api_secret
            .as_deref()
            .ok_or("missing POLYMARKET_API_SECRET")?;
        let passphrase = self
            .passphrase
            .as_deref()
            .ok_or("missing POLYMARKET_PASSPHRASE")?;
        let private_key = self.private_key.as_deref().ok_or("missing PRIVATE_KEY")?;
        let wallet = self
            .wallet_address
            .as_deref()
            .ok_or("missing wallet address")?;

        // USDC has 6 decimals on Polygon
        let usdc_decimals: f64 = 1_000_000.0;
        let maker_amount = if side_buy {
            (size * price * usdc_decimals) as u128
        } else {
            (size * usdc_decimals) as u128
        };
        let taker_amount = if side_buy {
            (size * usdc_decimals) as u128
        } else {
            (size * price * usdc_decimals) as u128
        };

        let salt: u128 = rand::random();
        let token_id_num: u128 = token_id.parse().unwrap_or(0);

        let params = OrderParams {
            salt,
            maker: wallet.to_string(),
            signer: wallet.to_string(),
            taker: "0x0000000000000000000000000000000000000000".to_string(),
            token_id: token_id_num,
            maker_amount,
            taker_amount,
            expiration: 0,
            nonce: 0,
            fee_rate_bps: u128::from(fee_rate_bps),
            side: u8::from(!side_buy),
            signature_type: 2,
        };

        let signature = signing::sign_order(private_key, &params)?;

        let order = SignedOrder {
            salt: salt.to_string(),
            maker: wallet.to_string(),
            signer: wallet.to_string(),
            taker: "0x0000000000000000000000000000000000000000".to_string(),
            token_id: token_id.to_string(),
            maker_amount: maker_amount.to_string(),
            taker_amount: taker_amount.to_string(),
            expiration: "0".to_string(),
            nonce: "0".to_string(),
            fee_rate_bps: fee_rate_bps.to_string(),
            side: if side_buy {
                "BUY".to_string()
            } else {
                "SELL".to_string()
            },
            signature_type: 2,
            signature,
        };

        let placement = OrderPlacement {
            order,
            owner: wallet.to_string(),
            order_type: "GTC".to_string(),
        };

        let body = serde_json::to_string(&placement)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default();

        let path = "/order";
        let headers = auth::build_headers(
            api_key, api_secret, passphrase, &timestamp, "POST", path, &body,
        );

        let url = format!("{}{path}", self.clob_url);
        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body);

        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp = req.send().await?;
        let order_resp: OrderResponse = resp.json().await?;
        Ok(order_resp)
    }
}
