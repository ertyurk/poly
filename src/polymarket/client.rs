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
    pub async fn fetch_markets(
        &self,
    ) -> Result<Vec<GammaMarket>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "{}/events?tag=crypto&active=true&closed=false&limit=100&order=volume24hr&ascending=false",
            self.gamma_url
        );
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Gamma API {status}: {body}").into());
        }
        let events: Vec<GammaEvent> = resp.json().await?;
        let markets: Vec<GammaMarket> = events
            .into_iter()
            .flat_map(|e| e.markets.unwrap_or_default())
            .collect();
        Ok(markets)
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
