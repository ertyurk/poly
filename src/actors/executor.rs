use crate::polymarket::PolymarketClient;
use crate::types::*;

/// Maximum price slippage tolerance (fraction) before rejecting a fill.
const MAX_SLIPPAGE: f64 = 0.10;

#[derive(Debug, Clone)]
struct OpenPosition {
    decision_id: i64,
    market_id: String,
    side: Side,
    entry_price: f64,
    size: f64,
    fee_rate: f64,
    entry_ts: TsMicros,
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
    client: Option<PolymarketClient>,
    /// Maps market_id → (token_yes, token_no)
    market_tokens: std::collections::HashMap<String, (String, String)>,
    /// Maximum fraction of bankroll that can be committed across all open positions.
    max_total_exposure: f64,
}

impl Executor {
    /// Create an executor.
    ///
    /// In `Paper` mode, `client` can be `None` — fills are simulated locally.
    /// In `Live` mode, `client` must be `Some` with authenticated credentials.
    pub fn new(
        mode: Mode,
        initial_bankroll: f64,
        client: Option<PolymarketClient>,
        max_total_exposure: f64,
    ) -> Self {
        Self {
            mode,
            bankroll: initial_bankroll,
            positions: Vec::new(),
            next_decision_id: 1,
            client,
            market_tokens: std::collections::HashMap::new(),
            max_total_exposure,
        }
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
    /// In `Live` mode: places an order via Polymarket CLOB API.
    pub async fn try_fill(
        &mut self,
        dec: &TradeDecision,
        best_ask: f64,
        best_bid: f64,
    ) -> Option<i64> {
        // Only one position per market
        if self.positions.iter().any(|p| p.market_id == dec.market_id) {
            return None;
        }

        // Check total exposure limit: committed capital can't exceed max_total_exposure
        let committed: f64 = self
            .positions
            .iter()
            .map(|p| p.size * p.entry_price)
            .sum();
        let max_exposure = self.bankroll * self.max_total_exposure;
        let available = (max_exposure - committed).max(0.0);
        let cost = dec.size * dec.price;
        if cost > available {
            tracing::debug!(
                market_id = %dec.market_id,
                cost = cost,
                available = available,
                committed = committed,
                max_exposure = max_exposure,
                "fill rejected: exposure limit"
            );
            return None;
        }

        let fill_price = match dec.side {
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
            return None;
        }

        let token_id = self.token_for_trade(&dec.market_id, dec.side);

        match self.mode {
            Mode::Paper => {
                tracing::info!(
                    market_id = %dec.market_id,
                    side = %dec.side,
                    size = dec.size,
                    price = fill_price,
                    "paper fill"
                );
            }
            Mode::Live => {
                if let Some(ref client) = self.client {
                    if let Some(ref tid) = token_id {
                        let side_buy = dec.side == Side::Yes;
                        let fee_bps = match client.fetch_fee_rate_bps(tid).await {
                            Ok(bps) => bps,
                            Err(e) => {
                                tracing::warn!(
                                    market_id = %dec.market_id,
                                    error = %e,
                                    "failed to fetch fee rate, using default 1000"
                                );
                                1000
                            }
                        };
                        match client
                            .place_order(tid, side_buy, fill_price, dec.size, fee_bps)
                            .await
                        {
                            Ok(resp) => {
                                if resp.success.unwrap_or(false) {
                                    tracing::info!(
                                        market_id = %dec.market_id,
                                        order_id = resp.order_id.as_deref().unwrap_or("?"),
                                        side = %dec.side,
                                        size = dec.size,
                                        price = fill_price,
                                        "live fill"
                                    );
                                } else {
                                    tracing::warn!(
                                        market_id = %dec.market_id,
                                        error = resp.error_msg.as_deref().unwrap_or("unknown"),
                                        "order rejected"
                                    );
                                    return None;
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    market_id = %dec.market_id,
                                    error = %e,
                                    "order placement failed"
                                );
                                return None;
                            }
                        }
                    } else {
                        tracing::warn!(market_id = %dec.market_id, "no token ID for market");
                        return None;
                    }
                }
            }
        }

        let id = self.next_decision_id;
        self.next_decision_id += 1;

        self.positions.push(OpenPosition {
            decision_id: id,
            market_id: dec.market_id.clone(),
            side: dec.side,
            entry_price: fill_price,
            size: dec.size,
            fee_rate: dec.fee_rate,
            entry_ts: dec.ts,
        });

        Some(id)
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
            let fee_paid = pos.size * pos.entry_price * pos.fee_rate;
            let gross_pnl = if won {
                pos.size * (1.0 - pos.entry_price)
            } else {
                -(pos.size * pos.entry_price)
            };
            let pnl = gross_pnl - fee_paid;
            self.bankroll += pnl;

            let outcome = if won { Outcome::Win } else { Outcome::Loss };

            results.push(TradeResult {
                decision_id: pos.decision_id,
                market_id: pos.market_id,
                side: pos.side,
                entry_price: pos.entry_price,
                size: pos.size,
                fee_rate: pos.fee_rate,
                fee_paid,
                gross_pnl,
                outcome,
                pnl,
                bankroll_after: self.bankroll,
                entry_ts: pos.entry_ts,
                resolved_ts,
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
