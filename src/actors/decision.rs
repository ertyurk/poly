use crate::math::{kelly, lmsr};
use crate::types::*;

/// Polymarket crypto fee rate parameter (same for all durations).
/// fee_per_share = price * FEE_RATE * (price * (1 - price))^2
const FEE_RATE: f64 = 0.25;

/// Compute raw edge: p_hat - p_market.
#[inline]
pub fn compute_edge(p_hat: f64, p_market: f64) -> f64 {
    p_hat - p_market
}

/// Effective edge after fees: |edge| - fee_rate.
#[inline]
pub fn effective_edge(edge_abs: f64, fee_rate: f64) -> f64 {
    edge_abs - fee_rate
}

/// Polymarket fee rate as a fraction of notional (price * size).
///
/// Formula: `FEE_RATE * (p * (1 - p))^FEE_EXPONENT`
/// At p=0.50 this is ~1.56%, dropping toward zero at extremes.
#[inline]
pub fn polymarket_fee_rate(p: f64) -> f64 {
    let p = p.clamp(0.0, 1.0);
    let pq = p * (1.0 - p);
    FEE_RATE * pq * pq
}

/// Entry gate: edge must exceed tau_min + effective_spread.
#[inline]
pub fn check_entry_gate(edge_abs: f64, tau_min: f64, p: f64, b: f64, delta_min: f64) -> bool {
    let spread = lmsr::effective_spread(p, b, delta_min);
    edge_abs > tau_min + spread
}

/// Cap order size to a fraction of 24h volume to reduce market impact.
#[inline]
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
    max_bet_fraction: f64,
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

    // 8. Bankroll hard cap
    size = size.min(bankroll * max_bet_fraction);

    // 9. Stealth cap
    size = apply_stealth_cap(size, volume_24h, max_volume_pct);

    // 10. Polymarket minimum order size
    const MIN_ORDER_SIZE: f64 = 5.0;
    if size < MIN_ORDER_SIZE {
        return Err(no_trade(SkipReason::InsufficientEdge, eff_edge));
    }

    // 11. Determine side
    let side = if edge > 0.0 { Side::Yes } else { Side::No };

    // 12. Return decision
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
    /// Configuration
    tau_min: f64,
    b: f64,
    kelly_fraction: f64,
    bankroll: f64,
    max_volume_pct: f64,
    max_bet_fraction: f64,
    min_confidence: f64,
}

impl DecisionActor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rx: mpsc::Receiver<DecisionInput>,
        tx: mpsc::Sender<DecisionOutput>,
        tau_min: f64,
        b: f64,
        kelly_fraction: f64,
        bankroll: f64,
        max_volume_pct: f64,
        max_bet_fraction: f64,
        min_confidence: f64,
    ) -> Self {
        Self {
            rx,
            tx,
            markets: std::collections::HashMap::new(),
            tau_min,
            b,
            kelly_fraction,
            bankroll,
            max_volume_pct,
            max_bet_fraction,
            min_confidence,
        }
    }

    pub async fn run(&mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                DecisionInput::Market(ms) => {
                    self.markets.insert(ms.market_id.clone(), ms);
                }
                DecisionInput::Signal(sig) => {
                    let Some(ms) = self.markets.get(&sig.market_id) else {
                        continue;
                    };

                    let fee_rate = polymarket_fee_rate(ms.midpoint);

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
                        self.max_bet_fraction,
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
