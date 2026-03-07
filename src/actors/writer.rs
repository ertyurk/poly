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
    pub fn new(db_path: &str, batch_size: usize, flush_interval_ms: u64) -> Result<Self, rusqlite::Error> {
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
                    match msg {
                        Some(event) => {
                            self.buffer.push(event);
                            if self.buffer.len() >= self.batch_size {
                                self.flush();
                            }
                        }
                        None => {
                            // Channel closed — flush remaining and exit.
                            if !self.buffer.is_empty() {
                                self.flush();
                            }
                            break;
                        }
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
        let events: Vec<DbEvent> = self.buffer.drain(..).collect();
        if let Err(e) = self.write_batch(&events) {
            tracing::error!(error = %e, count = events.len(), "failed to write batch");
        } else {
            tracing::debug!(count = events.len(), "flushed batch");
        }
    }

    fn write_batch(&mut self, events: &[DbEvent]) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;
        for event in events {
            match event {
                DbEvent::SpotPrice(sp) => {
                    db::queries::insert_spot_price(&tx, sp)?;
                }
                DbEvent::Market(ms) => {
                    db::queries::insert_market(&tx, ms)?;
                }
                DbEvent::BookSnapshot { market_id, best_bid, best_ask, midpoint, spread, ts } => {
                    db::queries::insert_book_snapshot(&tx, market_id, *best_bid, *best_ask, *midpoint, *spread, *ts)?;
                }
                DbEvent::Signal(sig) => {
                    db::queries::insert_signal(&tx, sig)?;
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
                DbEvent::ConfigSnapshot { config_json, ts } => {
                    db::queries::insert_config_snapshot(&tx, config_json, *ts)?;
                }
            }
        }
        tx.commit()
    }
}
