use crate::types::*;

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

pub struct PaperExecutor {
    bankroll: f64,
    positions: Vec<OpenPosition>,
    next_decision_id: i64,
}

impl PaperExecutor {
    pub fn new(initial_bankroll: f64) -> Self {
        Self {
            bankroll: initial_bankroll,
            positions: Vec::new(),
            next_decision_id: 1,
        }
    }

    pub fn bankroll(&self) -> f64 {
        self.bankroll
    }

    pub fn position_count(&self) -> usize {
        self.positions.len()
    }

    /// Try to fill a trade decision against the current order book.
    /// Returns the simulated decision_id if filled.
    pub fn try_fill(&mut self, dec: &TradeDecision, best_ask: f64, best_bid: f64) -> Option<i64> {
        let fill_price = match dec.side {
            Side::Yes => best_ask,
            Side::No => 1.0 - best_bid,
        };

        // Reject if price slipped more than 10% from expected
        if (fill_price - dec.price).abs() / dec.price > 0.10 {
            tracing::debug!(
                market_id = %dec.market_id,
                expected = dec.price,
                actual = fill_price,
                "fill rejected: price slipped"
            );
            return None;
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

        tracing::info!(
            market_id = %dec.market_id,
            side = ?dec.side,
            size = dec.size,
            price = fill_price,
            "paper fill"
        );

        Some(id)
    }

    /// Settle all positions for a resolved market
    pub fn settle(&mut self, market_id: &str, resolved_side: Side, resolved_ts: TsMicros) -> Vec<TradeResult> {
        let (to_settle, remaining): (Vec<_>, Vec<_>) = self.positions
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
                fee_paid,
                outcome,
                pnl,
                bankroll_after: self.bankroll,
                entry_ts: pos.entry_ts,
                resolved_ts,
            });
        }

        results
    }
}
