use rusqlite::{params, Connection};

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
        "INSERT OR IGNORE INTO markets (market_id, asset, window, token_yes, token_no, open_ts, resolution_ts, open_price)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            ms.market_id,
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
            dec.size,
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
        "INSERT INTO trades (decision_id, market_id, side, entry_price, size, fee_rate, fee_paid, gross_pnl, outcome, pnl, bankroll_after, entry_ts, resolved_ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            tr.decision_id,
            tr.market_id,
            tr.side.to_string(),
            tr.entry_price,
            tr.size,
            tr.fee_rate,
            tr.fee_paid,
            tr.gross_pnl,
            tr.outcome.to_string(),
            tr.pnl,
            tr.bankroll_after,
            tr.entry_ts,
            tr.resolved_ts,
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
