use crate::math::{kelly, lmsr};
use crate::types::*;

/// Polymarket crypto fee rate parameter (same for all durations).
/// fee_per_share = price * FEE_RATE * (price * (1 - price))^2
const FEE_RATE: f64 = 0.25;

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

/// Compute ADAPT composite score from signal components.
pub fn composite_score(
    confidence: f64,
    ofi_directional: f64,
    cross_directional: f64,
    vol_ratio: f64,
    w_zscore: f64,
    w_ofi: f64,
    w_cross: f64,
    w_volume: f64,
) -> f64 {
    let base = confidence * w_zscore;
    let ofi_contrib = ofi_directional.max(0.0) * w_ofi;
    let cross_contrib = cross_directional.max(0.0) * w_cross;
    let vol_boost = (vol_ratio.min(3.0) / 3.0) * w_volume;
    base + ofi_contrib + cross_contrib + vol_boost
}

/// Full decision pipeline.
///
/// Returns `Ok(TradeDecision)` when a trade should be placed,
/// or `Err(NoTrade)` when the opportunity is skipped.
#[allow(clippy::too_many_arguments)]
pub fn decide(
    p_hat: f64,
    p_market: f64,
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
    event_slug: &str,
    max_fill_price: f64,
    min_fill_price: f64,
    direction_guard: bool,
) -> Result<TradeDecision, NoTrade> {
    let ts = now_micros();

    // 1. Check confidence (doesn't need fill price)
    if confidence < min_confidence {
        return Err(NoTrade {
            market_id: market_id.to_string(),
            edge: p_hat - p_market,
            effective_edge: 0.0,
            fee_rate: 0.0,
            reason: SkipReason::LowConfidence,
            ts,
        });
    }

    // 2. Determine trade direction from midpoint
    let side = if p_hat > p_market {
        Side::Yes
    } else {
        Side::No
    };

    // 3. Actual fill price — what we'd pay per share.
    // This is what matters for profitability, not the midpoint.
    let fill_price = match side {
        Side::Yes => best_ask,
        Side::No => 1.0 - best_bid,
    };

    // 4a. Compute fee based on fill price (not midpoint) for accuracy.
    let fee_rate = polymarket_fee_rate(fill_price);

    // Helper to build NoTrade
    let no_trade = |edge: f64, reason: SkipReason, eff_edge: f64| NoTrade {
        market_id: market_id.to_string(),
        edge,
        effective_edge: eff_edge,
        fee_rate,
        reason,
        ts,
    };

    // 4b. Range check on fill price for risk/reward.
    // max_fill_price is regime-dependent:
    //   Standard regime: 0.50 (R/R ≥ 1.0)
    //   Convergence regime: 0.95 (near-deterministic outcome, fees ~0)
    if fill_price < min_fill_price || fill_price > max_fill_price {
        return Err(no_trade(
            p_hat - p_market,
            SkipReason::PriceOutOfRange,
            0.0,
        ));
    }

    // 4c. Direction guard: never bet against our model's probability direction.
    // If p_hat > 0.5 (model says YES likely) → only allow YES bets.
    // If p_hat < 0.5 (model says NO likely) → only allow NO bets.
    // This prevents contrarian bets where the market has already priced in
    // the displacement and we'd be betting against our own signal.
    // Disabled for multi-outcome markets (weather) where p_hat < 0.5 is normal.
    if direction_guard {
        let model_favors_yes = p_hat > 0.5;
        if (model_favors_yes && side == Side::No) || (!model_favors_yes && side == Side::Yes) {
            return Err(no_trade(
                p_hat - p_market,
                SkipReason::DirectionGuard,
                0.0,
            ));
        }
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
        event_slug: event_slug.to_string(),
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
    /// Notify that a position on this market has been settled/closed.
    PositionClosed(String),
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
    /// EMA-smoothed midpoint per market: (ema_value, last_update_ts_micros).
    midpoint_ema: std::collections::HashMap<String, (f64, i64)>,
    /// Markets with pending (unfilled or open) positions — suppress duplicate decisions.
    pending: std::collections::HashSet<String>,
    /// Consecutive signal direction tracker: (is_yes, streak_count).
    /// Requires 3+ consecutive signals agreeing before allowing a trade.
    signal_streak: std::collections::HashMap<String, (bool, u32)>,
    /// Configuration
    tau_min: f64,
    b: f64,
    kelly_fraction: f64,
    bankroll: f64,
    max_volume_pct: f64,
    max_bet_fraction: f64,
    adapt: crate::config::Adapt,
    midpoint_ema_tau: f64,
    min_displacement_pct: f64,
    max_fill_price: f64,
    kelly_fraction_no: f64,
    vol_regime_min: f64,
    vol_regime_max: f64,
    vr_block_threshold: f64,
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
        adapt: crate::config::Adapt,
        midpoint_ema_tau: f64,
        min_displacement_pct: f64,
        max_fill_price: f64,
        kelly_fraction_no: f64,
        vol_regime_min: f64,
        vol_regime_max: f64,
        vr_block_threshold: f64,
    ) -> Self {
        Self {
            rx,
            tx,
            markets: std::collections::HashMap::new(),
            midpoint_ema: std::collections::HashMap::new(),
            pending: std::collections::HashSet::new(),
            signal_streak: std::collections::HashMap::new(),
            tau_min,
            b,
            kelly_fraction,
            bankroll,
            max_volume_pct,
            max_bet_fraction,
            adapt,
            midpoint_ema_tau,
            min_displacement_pct,
            max_fill_price,
            kelly_fraction_no,
            vol_regime_min,
            vol_regime_max,
            vr_block_threshold,
        }
    }

    pub async fn run(&mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                DecisionInput::Market(ms) => {
                    let now = crate::types::now_micros();
                    let mid = ms.midpoint;
                    let ema = self
                        .midpoint_ema
                        .entry(ms.market_id.clone())
                        .or_insert((mid, now));
                    let dt_secs =
                        (now - ema.1).max(0) as f64 / 1_000_000.0;
                    let alpha = 1.0
                        - (-dt_secs / self.midpoint_ema_tau).exp();
                    ema.0 = alpha * mid + (1.0 - alpha) * ema.0;
                    ema.1 = now;
                    self.markets.insert(ms.market_id.clone(), ms);
                }
                DecisionInput::BankrollUpdate(b) => {
                    self.bankroll = b;
                }
                DecisionInput::PositionClosed(market_id) => {
                    self.pending.remove(&market_id);
                }
                DecisionInput::Signal(sig) => {
                    if sig.market_id.contains("_1d_") {
                        tracing::info!(
                            market = %sig.market_id,
                            p_hat = format_args!("{:.3}", sig.p_hat),
                            "decision actor received 1d signal"
                        );
                    }
                    // Skip if we already have a pending position on this market.
                    if self.pending.contains(&sig.market_id) {
                        continue;
                    }
                    let Some(ms) = self.markets.get(&sig.market_id) else {
                        if sig.market_id.contains("_1d_") {
                            tracing::warn!(
                                market = %sig.market_id,
                                "1d signal but no market state"
                            );
                        }
                        continue;
                    };

                    // =====================================================
                    // Two-regime entry: STANDARD (early) + CONVERGENCE (late)
                    // Data shows: early entries (>10 min remaining) win 71%,
                    // mid-window (50-85% elapsed) entries win only 22%.
                    // =====================================================

                    // Dead zone: ≥80% elapsed — reversals and thin liquidity.
                    if sig.elapsed_pct >= 0.80 {
                        continue;
                    }

                    // Elapsed gate: market-type dependent.
                    // UpDown: 50-80% is the sweet spot (data: 67% win rate
                    // at 60-75% elapsed vs 17% at <40%).
                    // Strike markets: 5% — match signal actor gate.
                    let is_updown_market =
                        matches!(ms.market_type, MarketType::UpDown);
                    let min_elapsed = if is_updown_market { 0.50 } else { 0.05 };
                    if sig.elapsed_pct < min_elapsed {
                        continue;
                    }

                    // Volatility regime filter (UpDown only).
                    // Strike markets exempt: their edge comes from strike distance
                    // vs vol surface, not from short-term vol regime.
                    if is_updown_market {
                        if sig.vol_regime < self.vol_regime_min {
                            continue;
                        }
                        if sig.vol_regime > self.vol_regime_max {
                            continue;
                        }
                    }

                    // --- Common definitions ---
                    let signal_yes = sig.p_hat > 0.5;
                    let is_updown = matches!(ms.market_type, MarketType::UpDown);
                    // UpDown: tight fill cap (coin-flip dynamics, need R/R).
                    // Strike: higher cap — the model has genuine edge from
                    // vol/drift estimation, so fills up to 0.85 are profitable
                    // when p_hat > 0.90.
                    let max_fill_price = if is_updown {
                        self.max_fill_price
                    } else {
                        0.85
                    };
                    let effective_kelly = if signal_yes {
                        self.kelly_fraction
                    } else {
                        self.kelly_fraction_no
                    };

                    // P_hat conviction filter: require extreme signal.
                    let min_p_yes = 0.80_f64;
                    let max_p_no = 0.30_f64;
                    if sig.p_hat > 0.5 && sig.p_hat < min_p_yes {
                        continue;
                    }
                    if sig.p_hat <= 0.5 && sig.p_hat > max_p_no {
                        continue;
                    }
                    // Block extreme NO overreactions on UpDown markets.
                    // Very extreme signals (< 0.10) are mostly noise dips.
                    if is_updown && !signal_yes && sig.p_hat < 0.10 {
                        continue;
                    }

                    // Market agreement: only block when market is extremely
                    // convicted in the opposite direction.
                    let ema_mid = self
                        .midpoint_ema
                        .get(&sig.market_id)
                        .map(|e| e.0)
                        .unwrap_or(ms.midpoint);
                    if signal_yes && ema_mid < 0.25 {
                        if sig.market_id.contains("_1d_") {
                            tracing::info!(market=%sig.market_id, ema_mid, "1d BLOCKED: mkt agree YES<0.25");
                        }
                        continue;
                    }
                    if !signal_yes && ema_mid > 0.75 {
                        if sig.market_id.contains("_1d_") {
                            tracing::info!(market=%sig.market_id, ema_mid, "1d BLOCKED: mkt agree NO>0.75");
                        }
                        continue;
                    }

                    // Signal streak: require consecutive same-direction signals.
                    // YES needs 3 (higher reversal risk on UpDown markets).
                    // NO needs 2 (momentum-aligned, faster reaction needed).
                    let sig_is_yes = sig.p_hat > 0.5;
                    let streak = self
                        .signal_streak
                        .entry(sig.market_id.clone())
                        .or_insert((sig_is_yes, 0));
                    if streak.0 == sig_is_yes {
                        streak.1 += 1;
                    } else {
                        *streak = (sig_is_yes, 1);
                    }
                    let min_streak = if sig_is_yes { 3 } else { 2 };
                    if streak.1 < min_streak {
                        if sig.market_id.contains("_1d_") && streak.1 == 1 {
                            tracing::info!(market=%sig.market_id, streak=streak.1, min_streak, "1d BLOCKED: streak");
                        }
                        continue;
                    }

                    // Displacement gate: require meaningful move.
                    // Only applies to UpDown — strike markets have inherent
                    // displacement from the strike distance itself.
                    if is_updown
                        && sig.displacement_pct.abs() < self.min_displacement_pct
                    {
                        continue;
                    }

                    // Displacement direction must match signal direction.
                    // Only for UpDown markets where displacement IS the scoreboard.
                    // For Above/Below markets the relationship is different —
                    // the strike distance determines direction, not displacement.
                    if is_updown {
                        let price_above_open = sig.displacement_pct > 0.0;
                        if price_above_open && !signal_yes {
                            continue;
                        }
                        if !price_above_open && signal_yes {
                            continue;
                        }
                    }

                    // Variance ratio filter (UpDown only):
                    // Data: VR < 1.0 → 75% win rate, VR >= 1.5 → 17%.
                    // For NO trades: only in moderate range (0.85 – 1.5).
                    // For YES trades: only below 1.5.
                    // Strike markets exempt — different VR dynamics.
                    if is_updown {
                        if let Some(vr) = sig.variance_ratio {
                            if !signal_yes && (vr < self.vr_block_threshold || vr > 1.5)
                            {
                                continue;
                            }
                            if signal_yes && vr > 1.5 {
                                continue;
                            }
                        }
                    }

                    // Note: confidence gate removed — it's mathematically
                    // redundant with p_hat thresholds (confidence = |p_hat-0.5|*2).
                    // The "low confidence wins" finding was actually "moderate p_hat
                    // wins" — now captured by p_hat 0.15-0.30 for NO and ≥0.80 for YES.

                    // Determine trade direction for directional OFI/cross-asset
                    let signal_says_up = sig.p_hat > 0.5;
                    let direction = if signal_says_up { 1.0 } else { -1.0 };

                    // ADAPT composite score
                    let adapt_confidence = composite_score(
                        sig.confidence,
                        sig.ofi_10s * direction,
                        sig.cross_asset_signal * direction,
                        sig.vol_ratio,
                        self.adapt.w_zscore,
                        self.adapt.w_ofi,
                        self.adapt.w_cross,
                        self.adapt.w_volume,
                    );

                    // Regime-adjusted min confidence
                    let effective_min_conf = if sig.vol_regime < 0.7 {
                        self.adapt.min_confidence_quiet
                    } else if sig.vol_regime > 1.5 {
                        self.adapt.min_confidence_hot
                    } else {
                        self.adapt.min_confidence_normal
                    };

                    // Log regime + large trades
                    if sig.large_trade {
                        tracing::debug!(
                            market = %sig.market_id,
                            ofi = format_args!("{:.2}", sig.ofi_10s),
                            vol_ratio = format_args!("{:.1}", sig.vol_ratio),
                            "large trade detected on underlying"
                        );
                    }

                    if sig.market_id.contains("_1d_") {
                        tracing::info!(
                            market = %sig.market_id,
                            p_hat = format_args!("{:.3}", sig.p_hat),
                            mid = format_args!("{:.3}", ms.midpoint),
                            bid = format_args!("{:.3}", ms.best_bid),
                            ask = format_args!("{:.3}", ms.best_ask),
                            max_fill = format_args!("{:.2}", max_fill_price),
                            is_updown = is_updown,
                            "1d PASSED ALL FILTERS → decide()"
                        );
                    }

                    let result = decide(
                        sig.p_hat,
                        ms.midpoint,
                        self.tau_min,
                        self.b,
                        effective_kelly,
                        self.bankroll,
                        ms.volume_24h,
                        self.max_volume_pct,
                        self.max_bet_fraction,
                        effective_min_conf,
                        adapt_confidence,
                        &sig.market_id,
                        ms.best_bid,
                        ms.best_ask,
                        &ms.event_slug,
                        max_fill_price,
                        0.05, // min_fill_price: crypto markets
                        true, // direction_guard: binary crypto markets
                    );

                    let output = match &result {
                        Ok(td) => {
                            self.pending.insert(td.market_id.clone());
                            tracing::info!(
                                side = %td.side,
                                fill = format_args!("{:.3}", td.price),
                                edge = format_args!("{:.3}", td.edge),
                                usd = format_args!("${:.2}", td.size_usd),
                                "TRADE"
                            );
                            DecisionOutput::Trade(td.clone())
                        }
                        Err(nt) => DecisionOutput::Skip(nt.clone()),
                    };

                    if self.tx.send(output).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}
