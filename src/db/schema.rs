use rusqlite::Connection;

pub fn create_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(include_str!("../../schema.sql"))?;
    migrate(conn)?;
    Ok(())
}

/// Run idempotent migrations for existing databases.
fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Migration: add estimated_slippage column to trades (PR #1)
    if !column_exists(conn, "trades", "estimated_slippage")? {
        conn.execute_batch(
            "ALTER TABLE trades ADD COLUMN estimated_slippage REAL NOT NULL DEFAULT 0.0;",
        )?;
        tracing::info!("migrated: added trades.estimated_slippage");
    }

    // Migration: add slow_drift column to signal_state (PR #1 fix)
    if !column_exists(conn, "signal_state", "slow_drift")? {
        conn.execute_batch(
            "ALTER TABLE signal_state ADD COLUMN slow_drift REAL NOT NULL DEFAULT 0.0;",
        )?;
        tracing::info!("migrated: added signal_state.slow_drift");
    }

    Ok(())
}

/// Check if a column exists in a table using PRAGMA table_info.
fn column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let found = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|r| r.as_deref() == Ok(column));
    Ok(found)
}
