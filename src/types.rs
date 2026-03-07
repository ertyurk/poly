use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Asset {
    BTC,
    ETH,
}

impl Asset {
    pub fn as_str(&self) -> &'static str {
        match self {
            Asset::BTC => "BTC",
            Asset::ETH => "ETH",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Window {
    FiveMin,
    FifteenMin,
}

impl Window {
    pub fn as_str(&self) -> &'static str {
        match self {
            Window::FiveMin => "5m",
            Window::FifteenMin => "15m",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Side {
    Yes,
    No,
}

impl Side {
    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Yes => "YES",
            Side::No => "NO",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Outcome {
    Win,
    Loss,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Win => "WIN",
            Outcome::Loss => "LOSS",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SkipReason {
    InsufficientEdge,
    FeeTooHigh,
    VolumeCapHit,
    LowConfidence,
    BankrollDepleted,
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::InsufficientEdge => "INSUFFICIENT_EDGE",
            SkipReason::FeeTooHigh => "FEE_TOO_HIGH",
            SkipReason::VolumeCapHit => "VOLUME_CAP_HIT",
            SkipReason::LowConfidence => "LOW_CONFIDENCE",
            SkipReason::BankrollDepleted => "BANKROLL_DEPLETED",
        }
    }
}

// Timestamps are unix microseconds (i64)
pub type TsMicros = i64;

pub fn now_micros() -> TsMicros {
    chrono::Utc::now().timestamp_micros()
}

// --- Messages passed between actors ---

#[derive(Debug, Clone)]
pub struct SpotPrice {
    pub asset: Asset,
    pub price: f64,
    pub ts: TsMicros,
}

#[derive(Debug, Clone)]
pub struct MarketState {
    pub market_id: String,
    pub asset: Asset,
    pub window: Window,
    pub token_yes: String,
    pub token_no: String,
    pub best_bid: f64,
    pub best_ask: f64,
    pub midpoint: f64,
    pub resolution_ts: TsMicros,
    pub open_ts: TsMicros,
    pub open_price: Option<f64>,
    pub volume_24h: f64,
}

#[derive(Debug, Clone)]
pub struct FeeScheduleEntry {
    pub prob_low: f64,
    pub prob_high: f64,
    pub fee_bps: f64,
}

#[derive(Debug, Clone)]
pub struct FeeUpdate {
    pub window: Window,
    pub schedule: Vec<FeeScheduleEntry>,
}

#[derive(Debug, Clone)]
pub struct Signal {
    pub market_id: String,
    pub p_hat: f64,
    pub confidence: f64,
    pub prior: f64,
    pub n_observations: u32,
    pub ts: TsMicros,
}

#[derive(Debug, Clone)]
pub struct TradeDecision {
    pub market_id: String,
    pub side: Side,
    pub size: f64,
    pub price: f64,
    pub edge: f64,
    pub effective_edge: f64,
    pub fee_rate: f64,
    pub kelly_fraction: f64,
    pub ts: TsMicros,
}

#[derive(Debug, Clone)]
pub struct NoTrade {
    pub market_id: String,
    pub edge: f64,
    pub effective_edge: f64,
    pub fee_rate: f64,
    pub reason: SkipReason,
    pub ts: TsMicros,
}

#[derive(Debug, Clone)]
pub struct TradeResult {
    pub decision_id: i64,
    pub market_id: String,
    pub side: Side,
    pub entry_price: f64,
    pub size: f64,
    pub fee_paid: f64,
    pub outcome: Outcome,
    pub pnl: f64,
    pub bankroll_after: f64,
    pub entry_ts: TsMicros,
    pub resolved_ts: TsMicros,
}

// --- Unified event for SQLite Writer ---

#[derive(Debug, Clone)]
pub enum DbEvent {
    SpotPrice(SpotPrice),
    Market(MarketState),
    BookSnapshot {
        market_id: String,
        best_bid: f64,
        best_ask: f64,
        midpoint: f64,
        spread: f64,
        ts: TsMicros,
    },
    Signal(Signal),
    Decision(TradeDecision),
    Skip(NoTrade),
    Trade(TradeResult),
    ConfigSnapshot {
        config_json: String,
        ts: TsMicros,
    },
}
