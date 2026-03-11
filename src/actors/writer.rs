use rusqlite::Connection;
use tokio::sync::mpsc;
use tokio::time::{self, Duration};

use crate::db;
use crate::types::*;

pub struct WriterActor {
    conn: Connection,
    batch_size: usize,
    flush_interval: Duration,
    buffer: Vec<DbEvent>,
}

impl WriterActor {
    pub fn new(
        db_path: &str,
        batch_size: usize,
        flush_interval_ms: u64,
    ) -> Result<Self, rusqlite::Error> {
        let conn = db::init(db_path)?;
        Ok(Self {
            conn,
            batch_size,
            flush_interval: Duration::from_millis(flush_interval_ms),
            buffer: Vec::with_capacity(batch_size),
        })
    }

    pub async fn run(&mut self, mut rx: mpsc::Receiver<DbEvent>) {
        let mut interval = time::interval(self.flush_interval);

        loop {
            tokio::select! {
                msg = rx.recv() => {
                    if let Some(event) = msg {
                        self.buffer.push(event);
                        if self.buffer.len() >= self.batch_size {
                            self.flush();
                        }
                    } else {
                        // Channel closed — flush remaining and exit.
                        if !self.buffer.is_empty() {
                            self.flush();
                        }
                        break;
                    }
                }
                _ = interval.tick() => {
                    if !self.buffer.is_empty() {
                        self.flush();
                    }
                }
            }
        }
    }

    fn flush(&mut self) {
        if let Err(e) = self.write_batch() {
            tracing::error!(error = %e, count = self.buffer.len(), "failed to write batch");
        } else {
            tracing::debug!(count = self.buffer.len(), "flushed batch");
        }
        self.buffer.clear();
    }

    fn write_batch(&mut self) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;
        for event in &self.buffer {
            match event {
                DbEvent::SpotPrice(sp) => {
                    db::queries::insert_spot_price(&tx, sp)?;
                }
                DbEvent::Market(ms) => {
                    db::queries::insert_market(&tx, ms)?;
                }
                DbEvent::BookSnapshot {
                    market_id,
                    best_bid,
                    best_ask,
                    midpoint,
                    spread,
                    ts,
                } => {
                    db::queries::insert_book_snapshot(
                        &tx, market_id, *best_bid, *best_ask, *midpoint, *spread, *ts,
                    )?;
                }
                DbEvent::Signal(sig) => {
                    if sig.p_hat.is_finite() && sig.confidence.is_finite() {
                        db::queries::insert_signal(&tx, sig)?;
                    }
                }
                DbEvent::Decision(dec) => {
                    let _ = db::queries::insert_decision(&tx, dec)?;
                }
                DbEvent::Skip(skip) => {
                    let _ = db::queries::insert_skip(&tx, skip)?;
                }
                DbEvent::Trade(tr) => {
                    db::queries::insert_trade(&tx, tr)?;
                }
                DbEvent::MarketResolution {
                    market_id,
                    resolved_side,
                } => {
                    db::queries::update_market_resolution(&tx, market_id, resolved_side)?;
                }
                DbEvent::ConfigSnapshot { config_json, ts } => {
                    db::queries::insert_config_snapshot(&tx, config_json, *ts)?;
                }
                DbEvent::SaveOpenPosition {
                    decision_id,
                    market_id,
                    side,
                    entry_price,
                    size,
                    fee_rate,
                    entry_ts,
                    estimated_slippage,
                } => {
                    let pos = db::queries::PersistedPosition {
                        decision_id: *decision_id,
                        market_id: market_id.clone(),
                        side: side.to_string(),
                        entry_price: *entry_price,
                        size: *size,
                        fee_rate: *fee_rate,
                        entry_ts: *entry_ts,
                        estimated_slippage: *estimated_slippage,
                    };
                    db::queries::save_open_position(&tx, &pos)?;
                }
                DbEvent::ClearOpenPositions { market_id } => {
                    db::queries::delete_open_positions(&tx, market_id)?;
                }
                DbEvent::FillRejection {
                    market_id,
                    side,
                    size,
                    price,
                    reason,
                    ts,
                } => {
                    db::queries::insert_fill_rejection(
                        &tx,
                        market_id,
                        &side.to_string(),
                        *size,
                        *price,
                        reason,
                        *ts,
                    )?;
                }
                DbEvent::SaveSignalState {
                    asset,
                    last_price,
                    last_ts,
                    valid_ticks,
                    variance,
                    drift,
                    slow_drift,
                    lambda,
                    slow_variance,
                } => {
                    let state = db::queries::SavedSignalState {
                        asset: asset.clone(),
                        last_price: *last_price,
                        last_ts: *last_ts,
                        valid_ticks: *valid_ticks,
                        variance: *variance,
                        drift: *drift,
                        slow_drift: *slow_drift,
                        lambda: *lambda,
                        slow_variance: *slow_variance,
                    };
                    db::queries::save_signal_state(&tx, &state, crate::types::now_micros())?;
                }
            }
        }
        tx.commit()
    }
}
