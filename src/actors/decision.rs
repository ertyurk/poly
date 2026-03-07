use crate::math::{kelly, lmsr};
use crate::types::*;

/// Compute raw edge: p_hat - p_market.
pub fn compute_edge(p_hat: f64, p_market: f64) -> f64 {
    p_hat - p_market
}

/// Effective edge after fees: |edge| - fee_rate.
pub fn effective_edge(edge_abs: f64, fee_rate: f64) -> f64 {
    edge_abs - fee_rate
}

/// Hardcoded fee schedule for 15-minute resolution markets (Polymarket approximation).
pub fn default_fee_schedule_15m() -> Vec<FeeScheduleEntry> {
    vec![
        FeeScheduleEntry { prob_low: 0.00, prob_high: 0.10, fee_bps: 100.0 },
        FeeScheduleEntry { prob_low: 0.10, prob_high: 0.20, fee_bps: 150.0 },
        FeeScheduleEntry { prob_low: 0.20, prob_high: 0.35, fee_bps: 200.0 },
        FeeScheduleEntry { prob_low: 0.35, prob_high: 0.65, fee_bps: 315.0 },
        FeeScheduleEntry { prob_low: 0.65, prob_high: 0.80, fee_bps: 200.0 },
        FeeScheduleEntry { prob_low: 0.80, prob_high: 0.90, fee_bps: 150.0 },
        FeeScheduleEntry { prob_low: 0.90, prob_high: 1.00, fee_bps: 100.0 },
    ]
}

/// Look up fee rate (as a fraction, e.g. 0.0315) for a given probability and window.
/// Falls back to the 0.35-0.65 bucket (highest fee) if no match is found.
pub fn lookup_fee(p_market: f64, _window: Window, schedule: &[FeeScheduleEntry]) -> f64 {
    for entry in schedule {
        if p_market >= entry.prob_low && p_market < entry.prob_high {
            return entry.fee_bps / 10_000.0;
        }
    }
    // Edge case: p_market == 1.0 falls into the last bucket
    if let Some(last) = schedule.last() {
        if (p_market - last.prob_high).abs() < 1e-10 {
            return last.fee_bps / 10_000.0;
        }
    }
    // Fallback: highest fee tier
    0.0315
}

/// Entry gate: edge must exceed tau_min + effective_spread.
pub fn check_entry_gate(edge_abs: f64, tau_min: f64, p: f64, b: f64, delta_min: f64) -> bool {
    let spread = lmsr::effective_spread(p, b, delta_min);
    edge_abs > tau_min + spread
}

/// Cap order size to a fraction of 24h volume to reduce market impact.
pub fn apply_stealth_cap(size: f64, volume_24h: f64, max_pct: f64) -> f64 {
    size.min(volume_24h * max_pct)
}

/// Full decision pipeline.
///
/// Returns `Ok(TradeDecision)` when a trade should be placed,
/// or `Err(NoTrade)` when the opportunity is skipped.
#[allow(clippy::too_many_arguments)]
pub fn decide(
    p_hat: f64,
    p_market: f64,
    fee_rate: f64,
    tau_min: f64,
    b: f64,
    kelly_fraction: f64,
    bankroll: f64,
    volume_24h: f64,
    max_volume_pct: f64,
    min_confidence: f64,
    confidence: f64,
    market_id: &str,
) -> Result<TradeDecision, NoTrade> {
    let ts = now_micros();

    // 1. Compute edge
    let edge = compute_edge(p_hat, p_market);
    let edge_abs = edge.abs();

    // Helper to build NoTrade
    let no_trade = |reason: SkipReason, eff_edge: f64| NoTrade {
        market_id: market_id.to_string(),
        edge,
        effective_edge: eff_edge,
        fee_rate,
        reason,
        ts,
    };

    // 2. Check confidence
    if confidence < min_confidence {
        return Err(no_trade(SkipReason::LowConfidence, 0.0));
    }

    // 3. Effective edge
    let eff_edge = effective_edge(edge_abs, fee_rate);
    if eff_edge <= 0.0 {
        return Err(no_trade(SkipReason::FeeTooHigh, eff_edge));
    }

    // 4. Entry gate
    if !check_entry_gate(edge_abs, tau_min, p_market, b, 1.0) {
        return Err(no_trade(SkipReason::InsufficientEdge, eff_edge));
    }

    // 5-7. Compute sizes
    // For No side, flip probabilities so Kelly/LMSR work correctly
    let (p_hat_eff, p_market_eff) = if edge > 0.0 {
        (p_hat, p_market)
    } else {
        (1.0 - p_hat, 1.0 - p_market)
    };

    let lmsr_size = lmsr::optimal_trade_size(p_hat_eff, p_market_eff, b).abs();
    let kelly_size = kelly::position_size(p_hat_eff, p_market_eff, kelly_fraction, bankroll);
    let mut size = lmsr_size.min(kelly_size);

    // 8. Stealth cap
    size = apply_stealth_cap(size, volume_24h, max_volume_pct);

    // 9. Final size check
    if size <= 0.0 {
        return Err(no_trade(SkipReason::InsufficientEdge, eff_edge));
    }

    // 10. Determine side
    let side = if edge > 0.0 { Side::Yes } else { Side::No };

    // 11. Return decision
    Ok(TradeDecision {
        market_id: market_id.to_string(),
        side,
        size,
        price: p_market,
        edge,
        effective_edge: eff_edge,
        fee_rate,
        kelly_fraction,
        ts,
    })
}

// ---------------------------------------------------------------------------
// DecisionActor — async actor that receives signals and market state
// ---------------------------------------------------------------------------

use tokio::sync::mpsc;

/// Messages the DecisionActor can receive.
#[derive(Debug, Clone)]
pub enum DecisionInput {
    Signal(Signal),
    Market(MarketState),
    Fee(FeeUpdate),
}

/// Messages the DecisionActor emits.
#[derive(Debug, Clone)]
pub enum DecisionOutput {
    Trade(TradeDecision),
    Skip(NoTrade),
}

pub struct DecisionActor {
    rx: mpsc::Receiver<DecisionInput>,
    tx: mpsc::Sender<DecisionOutput>,
    /// Cached latest market state per market_id.
    markets: std::collections::HashMap<String, MarketState>,
    /// Cached fee schedule per window.
    fee_schedules: std::collections::HashMap<String, Vec<FeeScheduleEntry>>,
    /// Configuration
    tau_min: f64,
    b: f64,
    kelly_fraction: f64,
    bankroll: f64,
    max_volume_pct: f64,
    min_confidence: f64,
}

impl DecisionActor {
    pub fn new(
        rx: mpsc::Receiver<DecisionInput>,
        tx: mpsc::Sender<DecisionOutput>,
        tau_min: f64,
        b: f64,
        kelly_fraction: f64,
        bankroll: f64,
        max_volume_pct: f64,
        min_confidence: f64,
    ) -> Self {
        let mut fee_schedules = std::collections::HashMap::new();
        fee_schedules.insert("15m".to_string(), default_fee_schedule_15m());
        Self {
            rx,
            tx,
            markets: std::collections::HashMap::new(),
            fee_schedules,
            tau_min,
            b,
            kelly_fraction,
            bankroll,
            max_volume_pct,
            min_confidence,
        }
    }

    pub async fn run(&mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                DecisionInput::Market(ms) => {
                    self.markets.insert(ms.market_id.clone(), ms);
                }
                DecisionInput::Fee(fu) => {
                    self.fee_schedules
                        .insert(fu.window.as_str().to_string(), fu.schedule);
                }
                DecisionInput::Signal(sig) => {
                    let Some(ms) = self.markets.get(&sig.market_id) else {
                        continue;
                    };

                    let window_key = ms.window.as_str().to_string();
                    let schedule = self
                        .fee_schedules
                        .get(&window_key)
                        .cloned()
                        .unwrap_or_else(default_fee_schedule_15m);

                    let fee_rate = lookup_fee(ms.midpoint, ms.window, &schedule);

                    let result = decide(
                        sig.p_hat,
                        ms.midpoint,
                        fee_rate,
                        self.tau_min,
                        self.b,
                        self.kelly_fraction,
                        self.bankroll,
                        ms.volume_24h,
                        self.max_volume_pct,
                        self.min_confidence,
                        sig.confidence,
                        &sig.market_id,
                    );

                    let output = match result {
                        Ok(td) => DecisionOutput::Trade(td),
                        Err(nt) => DecisionOutput::Skip(nt),
                    };

                    if self.tx.send(output).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}
