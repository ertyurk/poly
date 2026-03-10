use crate::polymarket::LiveTrader;
use crate::types::*;

/// Maximum price slippage tolerance (fraction) before rejecting a fill.
const MAX_SLIPPAGE: f64 = 0.10;

/// Details returned by a successful fill.
#[derive(Debug, Clone)]
pub struct FillResult {
    pub decision_id: i64,
    pub fill_price: f64,
    pub size_shares: f64,
    pub estimated_slippage: f64,
}

/// Estimate slippage for paper trading based on order size and spread.
///
/// Model: slippage = half_spread + size_impact
/// - half_spread: we cross the spread to get filled
/// - size_impact: larger orders move the price proportionally
///   (linear impact model: impact = size / (size + liquidity_depth))
///
/// Returns the slippage as a price delta (always positive).
#[inline]
fn estimate_slippage(spread: f64, size: f64) -> f64 {
    let half_spread = spread / 2.0;
    // Assume ~$50k liquidity depth on each side for typical Polymarket crypto markets
    const LIQUIDITY_DEPTH: f64 = 50_000.0;
    let size_impact = size / (size + LIQUIDITY_DEPTH) * 0.02; // max 2% impact
    half_spread + size_impact
}

#[derive(Debug, Clone)]
struct OpenPosition {
    decision_id: i64,
    market_id: String,
    side: Side,
    entry_price: f64,
    size_shares: f64,
    fee_rate: f64,
    entry_ts: TsMicros,
    estimated_slippage: f64,
}

/// Determines whether the executor places real orders or simulates fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Paper,
    Live,
}

pub struct Executor {
    mode: Mode,
    bankroll: f64,
    positions: Vec<OpenPosition>,
    next_decision_id: i64,
    trader: Option<LiveTrader>,
    /// Maps market_id → (token_yes, token_no)
    market_tokens: std::collections::HashMap<String, (String, String)>,
    /// Maximum fraction of bankroll that can be committed across all open positions.
    max_total_exposure: f64,
    /// Markets that recently failed to fill — cooldown to prevent API spam.
    failed_cooldown: std::collections::HashMap<String, crate::types::TsMicros>,
}

impl Executor {
    /// Create an executor.
    ///
    /// In `Paper` mode, `trader` can be `None` — fills are simulated locally.
    /// In `Live` mode, `trader` must be `Some` with authenticated SDK client.
    pub fn new(
        mode: Mode,
        initial_bankroll: f64,
        trader: Option<LiveTrader>,
        max_total_exposure: f64,
    ) -> Self {
        Self {
            mode,
            bankroll: initial_bankroll,
            positions: Vec::new(),
            next_decision_id: 1,
            trader,
            market_tokens: std::collections::HashMap::new(),
            max_total_exposure,
            failed_cooldown: std::collections::HashMap::new(),
        }
    }

    /// Restore an open position from a previous session.
    pub fn restore_position(
        &mut self,
        decision_id: i64,
        market_id: String,
        side: Side,
        entry_price: f64,
        size_shares: f64,
        fee_rate: f64,
        entry_ts: TsMicros,
        estimated_slippage: f64,
    ) {
        self.positions.push(OpenPosition {
            decision_id,
            market_id,
            side,
            entry_price,
            size_shares,
            fee_rate,
            entry_ts,
            estimated_slippage,
        });
        if decision_id >= self.next_decision_id {
            self.next_decision_id = decision_id + 1;
        }
    }

    /// Set the next decision ID (for restoring from DB).
    pub fn set_next_decision_id(&mut self, id: i64) {
        self.next_decision_id = id;
    }

    #[allow(dead_code)]
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    pub const fn bankroll(&self) -> f64 {
        self.bankroll
    }

    /// Register token IDs for a market so the executor knows which token to trade.
    pub fn register_market(&mut self, market_id: &str, token_yes: &str, token_no: &str) {
        self.market_tokens.insert(
            market_id.to_string(),
            (token_yes.to_string(), token_no.to_string()),
        );
    }

    /// Try to fill a trade decision.
    ///
    /// In `Paper` mode: simulates the fill at market prices.
    /// In `Live` mode: places an order via Polymarket official SDK.
    pub async fn try_fill(
        &mut self,
        dec: &TradeDecision,
        best_ask: f64,
        best_bid: f64,
    ) -> Result<FillResult, String> {
        // Only one position per market
        if self.positions.iter().any(|p| p.market_id == dec.market_id) {
            return Err("duplicate_position".to_string());
        }

        // Cooldown: skip markets that failed recently (60s)
        const COOLDOWN_MICROS: i64 = 60_000_000;
        let now = crate::types::now_micros();
        if let Some(&failed_at) = self.failed_cooldown.get(&dec.market_id) {
            if now - failed_at < COOLDOWN_MICROS {
                return Err("cooldown".to_string());
            }
            self.failed_cooldown.remove(&dec.market_id);
        }

        // Check total exposure limit: committed capital can't exceed max_total_exposure
        // size is in shares, so committed = shares * entry_price = USD cost
        let committed: f64 = self
            .positions
            .iter()
            .map(|p| p.size_shares * p.entry_price)
            .sum();
        let max_exposure = self.bankroll * self.max_total_exposure;
        let available = (max_exposure - committed).max(0.0);
        // dec.size_usd is in USD (from Kelly), so cost = dec.size_usd directly
        let cost = dec.size_usd;
        if cost > available {
            tracing::debug!(
                market_id = %dec.market_id,
                cost = cost,
                available = available,
                committed = committed,
                max_exposure = max_exposure,
                "fill rejected: exposure limit"
            );
            return Err("exposure_limit".to_string());
        }

        let mut fill_price = match dec.side {
            Side::Yes => best_ask,
            Side::No => 1.0 - best_bid,
        };

        // Reject if price slipped beyond tolerance
        if dec.price > 0.0 && (fill_price - dec.price).abs() / dec.price > MAX_SLIPPAGE {
            tracing::debug!(
                market_id = %dec.market_id,
                expected = dec.price,
                actual = fill_price,
                "fill rejected: price slipped"
            );
            return Err("price_slippage".to_string());
        }

        let token_id = self.token_for_trade(&dec.market_id, dec.side);

        // Compute slippage for paper mode
        let spread = (best_ask - best_bid).abs();
        let slippage = if self.mode == Mode::Paper {
            estimate_slippage(spread, dec.size_usd)
        } else {
            0.0
        };

        match self.mode {
            Mode::Paper => {
                // Apply slippage: buying pushes price up
                // Clamp to at most 5 cents above the original price to avoid
                // unrealistic fills near 1.0 (which leave no upside).
                fill_price = (fill_price + slippage).min(fill_price + 0.05).clamp(0.01, 0.95);
            }
            Mode::Live => {
                if let Some(ref trader) = self.trader {
                    if let Some(ref tid) = token_id {
                        // Opening a position = always BUY the relevant token.
                        // token_for_trade selects token_yes for YES, token_no for NO.
                        let side_buy = true;
                        // Convert USD size to whole shares for the SDK order.
                        // CLOB requires maker_amount (shares × price) to have ≤ 2 decimals.
                        // Whole shares × 2-decimal price = always ≤ 2 decimals.
                        let order_shares = (dec.size_usd / fill_price).floor();
                        if order_shares < 1.0 {
                            return Err("order_too_small".to_string());
                        }
                        match trader
                            .place_order(tid, side_buy, fill_price, order_shares)
                            .await
                        {
                            Ok(result) => {
                                if result.success && result.matched {
                                    tracing::info!(
                                        market_id = %dec.market_id,
                                        order_id = %result.order_id,
                                        side = %dec.side,
                                        shares = format_args!("{order_shares:.2}"),
                                        price = fill_price,
                                        "live fill (FOK matched)"
                                    );
                                } else {
                                    tracing::warn!(
                                        market_id = %dec.market_id,
                                        order_id = %result.order_id,
                                        side = %dec.side,
                                        shares = format_args!("{order_shares:.2}"),
                                        price = fill_price,
                                        success = result.success,
                                        matched = result.matched,
                                        "order not filled"
                                    );
                                    self.failed_cooldown.insert(dec.market_id.clone(), crate::types::now_micros());
                                    return Err("order_not_matched".to_string());
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    market_id = %dec.market_id,
                                    error = %e,
                                    side = %dec.side,
                                    shares = format_args!("{order_shares:.2}"),
                                    price = fill_price,
                                    "order placement failed"
                                );
                                self.failed_cooldown.insert(dec.market_id.clone(), crate::types::now_micros());
                                return Err(format!("order_failed: {e}"));
                            }
                        }
                    } else {
                        tracing::warn!(market_id = %dec.market_id, "no token ID for market");
                        return Err("no_token_id".to_string());
                    }
                }
            }
        }

        // Convert USD (from Kelly) to shares: shares = usd / fill_price
        // Live mode uses floored whole shares; paper mode uses exact fractional shares.
        let size_shares = if self.mode == Mode::Live {
            (dec.size_usd / fill_price).floor()
        } else {
            dec.size_usd / fill_price
        };

        tracing::info!(
            market_id = %dec.market_id,
            side = %dec.side,
            shares = format_args!("{size_shares:.2}"),
            usd = format_args!("${:.2}", dec.size_usd),
            price = fill_price,
            slippage = format_args!("{slippage:.4}"),
            mode = ?self.mode,
            "fill"
        );

        let id = self.next_decision_id;
        self.next_decision_id += 1;

        self.positions.push(OpenPosition {
            decision_id: id,
            market_id: dec.market_id.clone(),
            side: dec.side,
            entry_price: fill_price,
            size_shares,
            fee_rate: dec.fee_rate,
            entry_ts: dec.ts,
            estimated_slippage: slippage,
        });

        Ok(FillResult {
            decision_id: id,
            fill_price,
            size_shares,
            estimated_slippage: slippage,
        })
    }

    /// Settle all positions for a resolved market.
    pub fn settle(
        &mut self,
        market_id: &str,
        resolved_side: Side,
        resolved_ts: TsMicros,
    ) -> Vec<TradeResult> {
        let (to_settle, remaining): (Vec<_>, Vec<_>) = self
            .positions
            .drain(..)
            .partition(|p| p.market_id == market_id);

        self.positions = remaining;

        let mut results = Vec::new();
        for pos in to_settle {
            let won = pos.side == resolved_side;
            let fee_paid = pos.size_shares * pos.entry_price * pos.fee_rate;
            let gross_pnl = if won {
                pos.size_shares * (1.0 - pos.entry_price)
            } else {
                -(pos.size_shares * pos.entry_price)
            };
            let pnl = gross_pnl - fee_paid;
            self.bankroll += pnl;

            let outcome = if won { Outcome::Win } else { Outcome::Loss };

            results.push(TradeResult {
                decision_id: pos.decision_id,
                market_id: pos.market_id,
                side: pos.side,
                entry_price: pos.entry_price,
                size_shares: pos.size_shares,
                fee_rate: pos.fee_rate,
                fee_paid,
                gross_pnl,
                outcome,
                pnl,
                bankroll_after: self.bankroll,
                entry_ts: pos.entry_ts,
                resolved_ts,
                estimated_slippage: pos.estimated_slippage,
            });
        }

        results
    }

    fn token_for_trade(&self, market_id: &str, side: Side) -> Option<String> {
        self.market_tokens
            .get(market_id)
            .map(|(yes, no)| match side {
                Side::Yes => yes.clone(),
                Side::No => no.clone(),
            })
    }
}
