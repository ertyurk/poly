use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Asset {
    BTC,
    ETH,
}

impl fmt::Display for Asset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BTC => f.write_str("BTC"),
            Self::ETH => f.write_str("ETH"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Window {
    FiveMin,
    FifteenMin,
}

impl Window {
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FiveMin => "5m",
            Self::FifteenMin => "15m",
        }
    }
}

impl fmt::Display for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Side {
    Yes,
    No,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Yes => f.write_str("YES"),
            Self::No => f.write_str("NO"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Outcome {
    Win,
    Loss,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win => f.write_str("WIN"),
            Self::Loss => f.write_str("LOSS"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SkipReason {
    InsufficientEdge,
    FeeTooHigh,
    LowConfidence,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsufficientEdge => f.write_str("INSUFFICIENT_EDGE"),
            Self::FeeTooHigh => f.write_str("FEE_TOO_HIGH"),
            Self::LowConfidence => f.write_str("LOW_CONFIDENCE"),
        }
    }
}

// Timestamps are unix microseconds (i64)
pub type TsMicros = i64;

/// Milliseconds-to-microseconds conversion factor.
pub const MS_TO_MICROS: i64 = 1000;

#[inline]
pub fn now_micros() -> TsMicros {
    chrono::Utc::now().timestamp_micros()
}

// --- Messages passed between actors ---

#[derive(Debug, Clone, Copy)]
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
    #[allow(dead_code)]
    pub best_bid: f64,
    #[allow(dead_code)]
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

/// Command to settle a resolved market.
#[derive(Debug, Clone)]
pub struct SettleCommand {
    pub market_id: String,
    pub resolved_side: Side,
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
    MarketResolution {
        market_id: String,
        resolved_side: String,
    },
    ConfigSnapshot {
        config_json: String,
        ts: TsMicros,
    },
}
