use serde::Deserialize;

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
        // CLOB API returns bids sorted ascending — best (highest) bid is last.
        // Asks are sorted ascending — best (lowest) ask is first.
        let best_bid = resp
            .bids
            .as_ref()
            .and_then(|b| {
                b.iter()
                    .filter_map(|l| l.price.parse::<f64>().ok())
                    .reduce(f64::max)
            })
            .unwrap_or(0.0);

        let best_ask = resp
            .asks
            .as_ref()
            .and_then(|a| {
                a.iter()
                    .filter_map(|l| l.price.parse::<f64>().ok())
                    .reduce(f64::min)
            })
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
