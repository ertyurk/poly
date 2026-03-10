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
    best_bid: f64,
    best_ask: f64,
) -> Result<TradeDecision, NoTrade> {
    let ts = now_micros();

    // Helper to build NoTrade
    let no_trade = |edge: f64, reason: SkipReason, eff_edge: f64| NoTrade {
        market_id: market_id.to_string(),
        edge,
        effective_edge: eff_edge,
        fee_rate,
        reason,
        ts,
    };

    // 1. Check confidence (doesn't need fill price)
    if confidence < min_confidence {
        return Err(no_trade(p_hat - p_market, SkipReason::LowConfidence, 0.0));
    }

    // 2. Determine trade direction from midpoint
    let side = if p_hat > p_market { Side::Yes } else { Side::No };

    // 3. Actual fill price — what we'd pay per share.
    // This is what matters for profitability, not the midpoint.
    let fill_price = match side {
        Side::Yes => best_ask,
        Side::No => 1.0 - best_bid,
    };

    // 4. Range check on fill price (not midpoint)
    if fill_price < 0.35 || fill_price > 0.65 {
        return Err(no_trade(p_hat - p_market, SkipReason::InsufficientEdge, 0.0));
    }

    // 5. Compute edge against actual fill price.
    // YES: we pay fill_price, receive $1 with probability p_hat → edge = p_hat - fill_price
    // NO:  we pay fill_price, receive $1 with probability (1-p_hat) → edge = (1-p_hat) - fill_price
    let edge = match side {
        Side::Yes => p_hat - fill_price,
        Side::No => (1.0 - p_hat) - fill_price,
    };

    // Edge must be positive at the actual price we'd pay
    if edge <= 0.0 {
        return Err(no_trade(edge, SkipReason::InsufficientEdge, 0.0));
    }

    // 6. Effective edge after fees
    let eff_edge = effective_edge(edge, fee_rate);
    if eff_edge <= 0.0 {
        return Err(no_trade(edge, SkipReason::FeeTooHigh, eff_edge));
    }

    // 7. Entry gate (using fill price, not midpoint)
    if !check_entry_gate(edge, tau_min, fill_price, b, 1.0) {
        return Err(no_trade(edge, SkipReason::InsufficientEdge, eff_edge));
    }

    // 8. Kelly/LMSR sizing against fill price.
    // p_hat_eff = our model's probability for the side we're betting on.
    // fill_price = the cost per share (what the market charges us).
    let p_hat_eff = match side {
        Side::Yes => p_hat,
        Side::No => 1.0 - p_hat,
    };

    let lmsr_size = lmsr::optimal_trade_size(p_hat_eff, fill_price, b).abs();
    let kelly_size = kelly::position_size(p_hat_eff, fill_price, kelly_fraction, bankroll);
    let mut size_usd = lmsr_size.min(kelly_size);

    // 9. Bankroll hard cap
    size_usd = size_usd.min(bankroll * max_bet_fraction);

    // 10. Stealth cap
    size_usd = apply_stealth_cap(size_usd, volume_24h, max_volume_pct);

    // 11. Polymarket minimum order size
    const MIN_ORDER_SIZE: f64 = 5.0;
    if size_usd < MIN_ORDER_SIZE {
        return Err(no_trade(edge, SkipReason::InsufficientEdge, eff_edge));
    }

    // 12. Return decision — price is the actual fill price, not midpoint
    Ok(TradeDecision {
        market_id: market_id.to_string(),
        side,
        size_usd,
        price: fill_price,
        edge,
        effective_edge: eff_edge,
        fee_rate,
        kelly_fraction,
        best_bid,
        best_ask,
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
    BankrollUpdate(f64),
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
                DecisionInput::BankrollUpdate(b) => {
                    self.bankroll = b;
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
                        ms.best_bid,
                        ms.best_ask,
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
