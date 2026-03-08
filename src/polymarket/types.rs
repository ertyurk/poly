use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Gamma API responses
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GammaMarket {
    pub id: Option<String>,
    pub question: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub slug: Option<String>,
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,
    pub tokens: Option<Vec<GammaToken>>,
    pub active: Option<bool>,
    pub closed: Option<bool>,
    pub volume: Option<String>,
    pub liquidity: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(rename = "outcomePrices")]
    pub outcome_prices: Option<String>,
    /// Outcomes as JSON string, e.g. `["Up", "Down"]` or `["Yes", "No"]`
    pub outcomes: Option<String>,
    /// Token IDs as JSON string when `tokens` array is absent
    #[serde(rename = "clobTokenIds")]
    pub clob_token_ids: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GammaToken {
    pub token_id: Option<String>,
    pub outcome: Option<String>,
    pub price: Option<f64>,
}

// ---------------------------------------------------------------------------
// Gamma API — events (contain nested markets)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GammaEvent {
    pub id: Option<String>,
    pub title: Option<String>,
    pub slug: Option<String>,
    pub markets: Option<Vec<GammaMarket>>,
}

// ---------------------------------------------------------------------------
// CLOB API responses
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct OrderBookResponse {
    pub bids: Option<Vec<OrderBookLevel>>,
    pub asks: Option<Vec<OrderBookLevel>>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct OrderBookLevel {
    pub price: String,
    pub size: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct MidpointResponse {
    pub mid: Option<String>,
}

// ---------------------------------------------------------------------------
// Order placement (real trading)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SignedOrder {
    pub salt: String,
    pub maker: String,
    pub signer: String,
    pub taker: String,
    #[serde(rename = "tokenId")]
    pub token_id: String,
    #[serde(rename = "makerAmount")]
    pub maker_amount: String,
    #[serde(rename = "takerAmount")]
    pub taker_amount: String,
    pub expiration: String,
    pub nonce: String,
    #[serde(rename = "feeRateBps")]
    pub fee_rate_bps: String,
    pub side: String,
    #[serde(rename = "signatureType")]
    pub signature_type: u8,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrderPlacement {
    pub order: SignedOrder,
    pub owner: String,
    #[serde(rename = "orderType")]
    pub order_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OrderResponse {
    pub success: Option<bool>,
    #[serde(rename = "orderID")]
    pub order_id: Option<String>,
    #[serde(rename = "errorMsg")]
    pub error_msg: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsed order book (internal use)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ParsedBook {
    pub best_bid: f64,
    pub best_ask: f64,
    pub midpoint: f64,
    pub spread: f64,
}

impl ParsedBook {
    pub fn from_response(resp: &OrderBookResponse) -> Self {
        let best_bid = resp
            .bids
            .as_ref()
            .and_then(|b| b.first())
            .and_then(|l| l.price.parse::<f64>().ok())
            .unwrap_or(0.0);

        let best_ask = resp
            .asks
            .as_ref()
            .and_then(|a| a.first())
            .and_then(|l| l.price.parse::<f64>().ok())
            .unwrap_or(1.0);

        let midpoint = f64::midpoint(best_bid, best_ask);
        let spread = best_ask - best_bid;

        Self {
            best_bid,
            best_ask,
            midpoint,
            spread,
        }
    }
}
