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

/// Details of a live fill (GTD instant, GTD deferred, or FOK fallback).
/// Returned to main.rs so it can emit SaveOpenPosition DB events.
#[derive(Debug, Clone)]
pub struct LiveFill {
    pub decision_id: i64,
    pub market_id: String,
    pub side: Side,
    pub fill_price: f64,
    pub size_shares: f64,
    pub fee_rate: f64,
    pub entry_ts: TsMicros,
    pub event_slug: String,
}

/// Result of placing a GTD order.
#[derive(Debug)]
pub enum GtdResult {
    /// Order was instantly matched — position created, needs DB persistence.
    InstantFill(LiveFill),
    /// Order is resting on the book — poll for status.
    Resting(String),
}

/// Result of polling an active order that completed.
#[derive(Debug)]
pub struct PollCompletion {
    pub market_id: String,
    /// Some if filled (GTD deferred or FOK fallback), None if unfilled.
    pub fill: Option<LiveFill>,
    /// True if filled via maker (GTD), false if filled via taker (FOK).
    pub maker: bool,
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
    // Assume ~$5k liquidity depth on each side for typical Polymarket crypto markets
    const LIQUIDITY_DEPTH: f64 = 5_000.0;
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
    event_slug: String,
}

/// An in-flight order on the Polymarket CLOB (not yet filled).
#[derive(Debug, Clone)]
struct ActiveOrder {
    order_id: String,
    market_id: String,
    side: Side,
    price: f64,
    size_shares: f64,
    fee_rate: f64,
    signal_ts: TsMicros,
    placed_at: TsMicros,
    event_slug: String,
    /// Original TradeDecision for FOK fallback.
    decision: TradeDecision,
}

/// Action to take after polling an active order.
#[derive(Debug, Clone, Copy)]
enum PollAction {
    Filled,
    Expired,
    Cancelled,
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
    /// In-flight GTD orders awaiting fill.
    active_orders: std::collections::HashMap<String, ActiveOrder>,
    /// Market resolution timestamps (for the resolution guard).
    market_resolution_ts: std::collections::HashMap<String, TsMicros>,
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
            active_orders: std::collections::HashMap::new(),
            market_resolution_ts: std::collections::HashMap::new(),
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
            event_slug: String::new(),
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
    pub fn register_market(
        &mut self,
        market_id: &str,
        token_yes: &str,
        token_no: &str,
        resolution_ts: TsMicros,
    ) {
        self.market_tokens.insert(
            market_id.to_string(),
            (token_yes.to_string(), token_no.to_string()),
        );
        self.market_resolution_ts
            .insert(market_id.to_string(), resolution_ts);
    }

    /// Get the resolution timestamp for a market (i64::MAX if unknown).
    pub fn market_resolution_ts(&self, market_id: &str) -> TsMicros {
        self.market_resolution_ts
            .get(market_id)
            .copied()
            .unwrap_or(i64::MAX)
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
        // Guard: try_fill is for Paper mode only. Live mode must use try_place_gtd.
        if self.mode == Mode::Live {
            return Err("use_gtd_for_live".to_string());
        }

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
            Side::No => ((1.0 - best_bid) * 100.0).round() / 100.0,
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
                fill_price = (fill_price + slippage)
                    .min(fill_price + 0.05)
                    .clamp(0.01, 0.95);
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
                                    self.failed_cooldown
                                        .insert(dec.market_id.clone(), crate::types::now_micros());
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
                                self.failed_cooldown
                                    .insert(dec.market_id.clone(), crate::types::now_micros());
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
            event_slug: dec.event_slug.clone(),
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
                event_slug: pos.event_slug,
            });
        }

        results
    }

    /// Place a GTD limit order (Phase 1). If instantly matched, creates
    /// an OpenPosition. If resting, stores in active_orders for polling.
    pub async fn try_place_gtd(
        &mut self,
        dec: &TradeDecision,
        best_ask: f64,
        best_bid: f64,
        gtd_expiry_secs: u64,
        resolution_ts: TsMicros,
        min_time_before_resolution_secs: u64,
        gtd_price_bump: f64,
    ) -> Result<GtdResult, String> {
        // No duplicate positions per market
        if self.positions.iter().any(|p| p.market_id == dec.market_id) {
            return Err("duplicate_position".to_string());
        }

        // No duplicate active orders per market
        if self
            .active_orders
            .values()
            .any(|o| o.market_id == dec.market_id)
        {
            return Err("active_order_exists".to_string());
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

        // Resolution guard: no orders too close to resolution
        let secs_to_resolution = (resolution_ts - now) / 1_000_000;
        if secs_to_resolution < min_time_before_resolution_secs as i64 {
            return Err("too_close_to_resolution".to_string());
        }

        // Exposure limit: committed capital includes positions + active orders
        let pos_committed: f64 = self
            .positions
            .iter()
            .map(|p| p.size_shares * p.entry_price)
            .sum();
        let order_committed: f64 = self
            .active_orders
            .values()
            .map(|o| o.size_shares * o.price)
            .sum();
        let committed = pos_committed + order_committed;
        let max_exposure = self.bankroll * self.max_total_exposure;
        let available = (max_exposure - committed).max(0.0);
        if dec.size_usd > available {
            tracing::debug!(
                market_id = %dec.market_id,
                cost = dec.size_usd,
                available = available,
                committed = committed,
                "GTD rejected: exposure limit"
            );
            return Err("exposure_limit".to_string());
        }

        // Compute fill price: YES uses best_ask, NO uses complement of best_bid
        // Apply gtd_price_bump to cross the spread slightly for faster fills.
        let fill_price = match dec.side {
            Side::Yes => ((best_ask + gtd_price_bump) * 100.0).round() / 100.0,
            Side::No => (((1.0 - best_bid) + gtd_price_bump) * 100.0).round() / 100.0,
        }
        .min(0.95);

        // Slippage check
        if dec.price > 0.0 && (fill_price - dec.price).abs() / dec.price > MAX_SLIPPAGE {
            return Err("price_slippage".to_string());
        }

        let token_id = self
            .token_for_trade(&dec.market_id, dec.side)
            .ok_or_else(|| "no_token_id".to_string())?;

        let order_shares = (dec.size_usd / fill_price).floor();
        if order_shares < 1.0 {
            return Err("order_too_small".to_string());
        }

        let trader = self
            .trader
            .as_ref()
            .ok_or_else(|| "no_trader".to_string())?;

        let side_buy = true;
        match trader
            .place_gtd_order(
                &token_id,
                side_buy,
                fill_price,
                order_shares,
                gtd_expiry_secs,
            )
            .await
        {
            Ok(result) => {
                if result.matched {
                    // Instant fill — create position directly
                    let id = self.next_decision_id;
                    self.next_decision_id += 1;
                    tracing::info!(
                        market_id = %dec.market_id,
                        order_id = %result.order_id,
                        side = %dec.side,
                        shares = format_args!("{order_shares:.2}"),
                        price = fill_price,
                        "GTD instant fill"
                    );
                    self.positions.push(OpenPosition {
                        decision_id: id,
                        market_id: dec.market_id.clone(),
                        side: dec.side,
                        entry_price: fill_price,
                        size_shares: order_shares,
                        fee_rate: 0.0, // maker fill
                        entry_ts: dec.ts,
                        estimated_slippage: 0.0,
                        event_slug: dec.event_slug.clone(),
                    });
                    Ok(GtdResult::InstantFill(LiveFill {
                        decision_id: id,
                        market_id: dec.market_id.clone(),
                        side: dec.side,
                        fill_price,
                        size_shares: order_shares,
                        fee_rate: 0.0,
                        entry_ts: dec.ts,
                        event_slug: dec.event_slug.clone(),
                    }))
                } else if result.success {
                    // Resting on book — track for polling
                    tracing::info!(
                        market_id = %dec.market_id,
                        order_id = %result.order_id,
                        side = %dec.side,
                        shares = format_args!("{order_shares:.2}"),
                        price = fill_price,
                        "GTD resting on book"
                    );
                    self.active_orders.insert(
                        result.order_id.clone(),
                        ActiveOrder {
                            order_id: result.order_id.clone(),
                            market_id: dec.market_id.clone(),
                            side: dec.side,
                            price: fill_price,
                            size_shares: order_shares,
                            fee_rate: dec.fee_rate,
                            signal_ts: dec.ts,
                            placed_at: now,
                            event_slug: dec.event_slug.clone(),
                            decision: dec.clone(),
                        },
                    );
                    Ok(GtdResult::Resting(result.order_id))
                } else {
                    self.failed_cooldown
                        .insert(dec.market_id.clone(), crate::types::now_micros());
                    Err("gtd_rejected".to_string())
                }
            }
            Err(e) => {
                tracing::error!(
                    market_id = %dec.market_id,
                    error = %e,
                    "GTD order placement failed"
                );
                self.failed_cooldown
                    .insert(dec.market_id.clone(), crate::types::now_micros());
                Err(format!("gtd_failed: {e}"))
            }
        }
    }

    /// Poll all active orders for fills/expirations.
    /// Returns Vec<(market_id, was_filled)> for completed orders.
    pub async fn poll_active_orders(
        &mut self,
        gtd_expiry_secs: u64,
        max_signal_age_secs: u64,
        fok_price_bump: f64,
    ) -> Vec<PollCompletion> {
        let now = crate::types::now_micros();
        let expiry_micros = gtd_expiry_secs as i64 * 1_000_000;
        let max_age_micros = max_signal_age_secs as i64 * 1_000_000;

        // Snapshot orders to avoid borrow conflicts with &mut self
        let orders: Vec<ActiveOrder> = self.active_orders.values().cloned().collect();
        if orders.is_empty() {
            return Vec::new();
        }

        // Phase 1: check statuses (immutable borrow of self.trader)
        let mut actions: Vec<(ActiveOrder, PollAction)> = Vec::new();
        {
            let trader = match self.trader.as_ref() {
                Some(t) => t,
                None => return Vec::new(),
            };
            for order in orders {
                let age = now - order.placed_at;
                match trader.check_order_status(&order.order_id).await {
                    Ok(Some(true)) => {
                        actions.push((order, PollAction::Filled));
                    }
                    Ok(Some(false)) if age >= expiry_micros => {
                        actions.push((order, PollAction::Expired));
                    }
                    Ok(Some(false)) => {} // still live, skip
                    Ok(None) => {
                        actions.push((order, PollAction::Cancelled));
                    }
                    Err(e) => {
                        tracing::warn!(
                            order_id = %order.order_id,
                            error = %e,
                            "failed to check order status"
                        );
                        // Don't remove — retry next poll
                    }
                }
            }
        }
        // trader borrow dropped here

        // Phase 2: process actions (mutable borrow of self)
        let mut completed = Vec::new();
        for (order, action) in &actions {
            match action {
                PollAction::Filled => {
                    let id = self.next_decision_id;
                    self.next_decision_id += 1;
                    tracing::info!(
                        market_id = %order.market_id,
                        order_id = %order.order_id,
                        side = %order.side,
                        shares = format_args!(
                            "{:.2}", order.size_shares
                        ),
                        price = order.price,
                        "GTD order filled (maker)"
                    );
                    self.positions.push(OpenPosition {
                        decision_id: id,
                        market_id: order.market_id.clone(),
                        side: order.side,
                        entry_price: order.price,
                        size_shares: order.size_shares,
                        fee_rate: 0.0,
                        entry_ts: order.signal_ts,
                        estimated_slippage: 0.0,
                        event_slug: order.event_slug.clone(),
                    });
                    completed.push(PollCompletion {
                        market_id: order.market_id.clone(),
                        fill: Some(LiveFill {
                            decision_id: id,
                            market_id: order.market_id.clone(),
                            side: order.side,
                            fill_price: order.price,
                            size_shares: order.size_shares,
                            fee_rate: 0.0,
                            entry_ts: order.signal_ts,
                            event_slug: order.event_slug.clone(),
                        }),
                        maker: true,
                    });
                }
                PollAction::Expired | PollAction::Cancelled => {
                    let label = match action {
                        PollAction::Expired => "expired",
                        _ => "cancelled",
                    };
                    tracing::info!(
                        market_id = %order.market_id,
                        order_id = %order.order_id,
                        reason = label,
                        "GTD order done, trying FOK fallback"
                    );
                    let fill = self
                        .try_fok_fallback(order, now, max_age_micros, fok_price_bump)
                        .await;
                    completed.push(PollCompletion {
                        market_id: order.market_id.clone(),
                        fill,
                        maker: false,
                    });
                }
            }
        }

        // Remove processed orders
        for (order, _) in &actions {
            self.active_orders.remove(&order.order_id);
        }

        completed
    }

    /// Attempt a FOK fallback after a GTD order expired or was cancelled.
    /// Returns Some(LiveFill) if the fallback filled, None otherwise.
    async fn try_fok_fallback(
        &mut self,
        order: &ActiveOrder,
        now: TsMicros,
        max_age_micros: i64,
        fok_price_bump: f64,
    ) -> Option<LiveFill> {
        let signal_age = now - order.signal_ts;
        if signal_age > max_age_micros {
            tracing::info!(
                market_id = %order.market_id,
                signal_age_s = signal_age / 1_000_000,
                "signal too stale for FOK fallback"
            );
            return None;
        }

        // I1: Explicitly cancel GTD before FOK to prevent double-fill
        // (belt-and-suspenders with auto-expiry — CLOB clock may lag local)
        if let Some(ref trader) = self.trader {
            let _ = trader.cancel_order(&order.order_id).await;
        }

        // C4: Re-check exposure (may have changed since GTD was placed 15s+ ago)
        // Exclude the current order — it just expired/cancelled, not committed.
        let pos_committed: f64 = self
            .positions
            .iter()
            .map(|p| p.size_shares * p.entry_price)
            .sum();
        let order_committed: f64 = self
            .active_orders
            .values()
            .filter(|o| o.order_id != order.order_id)
            .map(|o| o.size_shares * o.price)
            .sum();
        let committed = pos_committed + order_committed;
        let max_exposure = self.bankroll * self.max_total_exposure;
        let available = (max_exposure - committed).max(0.0);
        if order.decision.size_usd > available {
            tracing::info!(
                market_id = %order.market_id,
                cost = order.decision.size_usd,
                available = available,
                "FOK fallback skipped: exposure limit"
            );
            return None;
        }

        // I5: Cap at 0.95 to maintain minimum R/R
        let bump_price = (order.price + fok_price_bump).min(0.95);
        let token_id = self.token_for_trade(&order.market_id, order.side);
        let tid = match token_id {
            Some(t) => t,
            None => return None,
        };
        let fok_shares = (order.decision.size_usd / bump_price).floor();
        if fok_shares < 1.0 {
            return None;
        }

        let trader = match self.trader.as_ref() {
            Some(t) => t,
            None => return None,
        };

        match trader.place_order(&tid, true, bump_price, fok_shares).await {
            Ok(r) if r.success && r.matched => {
                let id = self.next_decision_id;
                self.next_decision_id += 1;
                tracing::info!(
                    market_id = %order.market_id,
                    order_id = %r.order_id,
                    price = bump_price,
                    shares = format_args!("{fok_shares:.2}"),
                    "FOK fallback filled (taker)"
                );
                self.positions.push(OpenPosition {
                    decision_id: id,
                    market_id: order.market_id.clone(),
                    side: order.side,
                    entry_price: bump_price,
                    size_shares: fok_shares,
                    fee_rate: order.fee_rate,
                    entry_ts: order.signal_ts,
                    estimated_slippage: 0.0,
                    event_slug: order.event_slug.clone(),
                });
                Some(LiveFill {
                    decision_id: id,
                    market_id: order.market_id.clone(),
                    side: order.side,
                    fill_price: bump_price,
                    size_shares: fok_shares,
                    fee_rate: order.fee_rate,
                    entry_ts: order.signal_ts,
                    event_slug: order.event_slug.clone(),
                })
            }
            Ok(_) => {
                tracing::warn!(
                    market_id = %order.market_id,
                    "FOK fallback not matched"
                );
                None
            }
            Err(e) => {
                tracing::error!(
                    market_id = %order.market_id,
                    error = %e,
                    "FOK fallback failed"
                );
                None
            }
        }
    }

    /// Cancel all active orders (shutdown safety net).
    /// Retries up to 3 times on failure — orphaned orders are the worst outcome.
    pub async fn cancel_all_active_orders(&mut self) {
        if let Some(ref trader) = self.trader {
            let mut attempts = 0;
            loop {
                attempts += 1;
                match trader.cancel_all_orders().await {
                    Ok(n) => {
                        tracing::info!(
                            canceled = n,
                            "canceled all open orders on shutdown"
                        );
                        break;
                    }
                    Err(e) => {
                        if attempts >= 3 {
                            tracing::error!(
                                error = %e,
                                attempts,
                                "failed to cancel orders after retries"
                            );
                            break;
                        }
                        tracing::warn!(
                            error = %e,
                            attempt = attempts,
                            "cancel-all failed, retrying..."
                        );
                        tokio::time::sleep(
                            tokio::time::Duration::from_secs(1),
                        )
                        .await;
                    }
                }
            }
        }
        self.active_orders.clear();
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
