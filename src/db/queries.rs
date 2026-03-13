use rusqlite::{params, Connection, OptionalExtension};

use crate::types::*;

pub fn insert_spot_price(conn: &Connection, sp: &SpotPrice) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO spot_prices (asset, price, ts) VALUES (?1, ?2, ?3)",
        params![sp.asset.to_string(), sp.price, sp.ts],
    )?;
    Ok(())
}

pub fn update_market_resolution(
    conn: &Connection,
    market_id: &str,
    resolved_side: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE markets SET resolved_side = ?1 WHERE market_id = ?2",
        params![resolved_side, market_id],
    )?;
    Ok(())
}

pub fn insert_market(conn: &Connection, ms: &MarketState) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR IGNORE INTO markets (market_id, condition_id, asset, window, token_yes, token_no, open_ts, resolution_ts, open_price)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            ms.market_id,
            ms.condition_id,
            ms.asset.to_string(),
            ms.window.to_string(),
            ms.token_yes,
            ms.token_no,
            ms.open_ts,
            ms.resolution_ts,
            ms.open_price,
        ],
    )?;
    Ok(())
}

pub fn insert_book_snapshot(
    conn: &Connection,
    market_id: &str,
    best_bid: f64,
    best_ask: f64,
    midpoint: f64,
    spread: f64,
    ts: TsMicros,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO book_snapshots (market_id, best_bid, best_ask, midpoint, spread, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![market_id, best_bid, best_ask, midpoint, spread, ts],
    )?;
    Ok(())
}

pub fn insert_signal(conn: &Connection, sig: &Signal) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO signals (market_id, p_hat, confidence, prior, n_observations, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            sig.market_id,
            sig.p_hat,
            sig.confidence,
            sig.prior,
            sig.n_observations,
            sig.ts,
        ],
    )?;
    Ok(())
}

pub fn insert_decision(conn: &Connection, dec: &TradeDecision) -> Result<i64, rusqlite::Error> {
    conn.execute(
        "INSERT INTO decisions (market_id, action, side, size, price, edge, effective_edge, fee_rate, kelly_fraction, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            dec.market_id,
            "TRADE",
            dec.side.to_string(),
            dec.size_usd,
            dec.price,
            dec.edge,
            dec.effective_edge,
            dec.fee_rate,
            dec.kelly_fraction,
            dec.ts,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_skip(conn: &Connection, skip: &NoTrade) -> Result<i64, rusqlite::Error> {
    conn.execute(
        "INSERT INTO decisions (market_id, action, edge, effective_edge, fee_rate, skip_reason, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            skip.market_id,
            "SKIP",
            skip.edge,
            skip.effective_edge,
            skip.fee_rate,
            skip.reason.to_string(),
            skip.ts,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_trade(conn: &Connection, tr: &TradeResult) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO trades (decision_id, market_id, side, entry_price, size, fee_rate, fee_paid, gross_pnl, outcome, pnl, bankroll_after, entry_ts, resolved_ts, estimated_slippage)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            tr.decision_id,
            tr.market_id,
            tr.side.to_string(),
            tr.entry_price,
            tr.size_shares,
            tr.fee_rate,
            tr.fee_paid,
            tr.gross_pnl,
            tr.outcome.to_string(),
            tr.pnl,
            tr.bankroll_after,
            tr.entry_ts,
            tr.resolved_ts,
            tr.estimated_slippage,
        ],
    )?;
    Ok(())
}

pub fn insert_config_snapshot(
    conn: &Connection,
    json: &str,
    ts: TsMicros,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO config_snapshots (config_json, ts) VALUES (?1, ?2)",
        params![json, ts],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Open position persistence (survive restarts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PersistedPosition {
    pub decision_id: i64,
    pub market_id: String,
    pub side: String,
    pub entry_price: f64,
    pub size: f64,
    pub fee_rate: f64,
    pub entry_ts: i64,
    pub estimated_slippage: f64,
}

pub fn save_open_position(
    conn: &Connection,
    pos: &PersistedPosition,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO open_positions (decision_id, market_id, side, entry_price, size, fee_rate, entry_ts, estimated_slippage)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            pos.decision_id, pos.market_id, pos.side, pos.entry_price,
            pos.size, pos.fee_rate, pos.entry_ts, pos.estimated_slippage,
        ],
    )?;
    Ok(())
}

pub fn delete_open_positions(conn: &Connection, market_id: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM open_positions WHERE market_id = ?1",
        params![market_id],
    )?;
    Ok(())
}

pub fn load_open_positions(conn: &Connection) -> Result<Vec<PersistedPosition>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT decision_id, market_id, side, entry_price, size, fee_rate, entry_ts, estimated_slippage
         FROM open_positions",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PersistedPosition {
            decision_id: row.get(0)?,
            market_id: row.get(1)?,
            side: row.get(2)?,
            entry_price: row.get(3)?,
            size: row.get(4)?,
            fee_rate: row.get(5)?,
            entry_ts: row.get(6)?,
            estimated_slippage: row.get(7)?,
        })
    })?;
    rows.collect()
}

/// Load market metadata for all open positions (used to seed market_fetcher on restart).
pub fn load_markets_for_open_positions(
    conn: &Connection,
) -> Result<Vec<RestoredMarket>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT m.market_id, m.condition_id, m.asset, m.window, m.token_yes, m.token_no,
                m.resolution_ts, m.open_ts, m.open_price
         FROM open_positions op
         INNER JOIN markets m ON m.market_id = op.market_id
         WHERE m.resolved_side IS NULL AND m.condition_id != ''",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(RestoredMarket {
            market_id: row.get(0)?,
            condition_id: row.get(1)?,
            asset: row.get(2)?,
            window: row.get(3)?,
            token_yes: row.get(4)?,
            token_no: row.get(5)?,
            resolution_ts: row.get(6)?,
            open_ts: row.get(7)?,
            open_price: row.get(8)?,
        })
    })?;
    rows.collect()
}

#[derive(Debug, Clone)]
pub struct RestoredMarket {
    pub market_id: String,
    pub condition_id: String,
    pub asset: String,
    pub window: String,
    pub token_yes: String,
    pub token_no: String,
    pub resolution_ts: i64,
    pub open_ts: i64,
    pub open_price: Option<f64>,
}

pub fn insert_fill_rejection(
    conn: &Connection,
    market_id: &str,
    side: &str,
    size: f64,
    price: f64,
    reason: &str,
    ts: TsMicros,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO fill_rejections (market_id, side, size, price, reason, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![market_id, side, size, price, reason, ts],
    )?;
    Ok(())
}

pub fn last_bankroll(conn: &Connection) -> Result<Option<f64>, rusqlite::Error> {
    conn.query_row(
        "SELECT bankroll_after FROM trades ORDER BY resolved_ts DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
}

pub fn max_decision_id(conn: &Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row("SELECT COALESCE(MAX(id), 0) FROM decisions", [], |row| {
        row.get(0)
    })
}

// ---------------------------------------------------------------------------
// Signal state persistence
// ---------------------------------------------------------------------------

/// Persisted signal state for warm-up recovery.
#[derive(Debug, Clone)]
pub struct SavedSignalState {
    pub asset: String,
    pub last_price: f64,
    pub last_ts: i64,
    pub valid_ticks: u32,
    pub variance: f64,
    pub drift: f64,
    pub slow_drift: f64,
    pub lambda: f64,
    pub slow_variance: f64,
}

pub fn save_signal_state(
    conn: &Connection,
    state: &SavedSignalState,
    saved_at: TsMicros,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO signal_state (asset, last_price, last_ts, valid_ticks, variance, drift, slow_drift, lambda, saved_at, slow_variance)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            state.asset,
            state.last_price,
            state.last_ts,
            state.valid_ticks,
            state.variance,
            state.drift,
            state.slow_drift,
            state.lambda,
            saved_at,
            state.slow_variance,
        ],
    )?;
    Ok(())
}

pub fn load_signal_states(
    conn: &Connection,
    max_age_secs: i64,
) -> Result<Vec<SavedSignalState>, rusqlite::Error> {
    let cutoff = crate::types::now_micros() - max_age_secs * 1_000_000;
    let mut stmt = conn.prepare(
        "SELECT asset, last_price, last_ts, valid_ticks, variance, drift, slow_drift, lambda, COALESCE(slow_variance, variance)
         FROM signal_state WHERE saved_at > ?1",
    )?;
    let rows = stmt.query_map(params![cutoff], |row| {
        Ok(SavedSignalState {
            asset: row.get(0)?,
            last_price: row.get(1)?,
            last_ts: row.get(2)?,
            valid_ticks: row.get(3)?,
            variance: row.get(4)?,
            drift: row.get(5)?,
            slow_drift: row.get(6)?,
            lambda: row.get(7)?,
            slow_variance: row.get(8)?,
        })
    })?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}
