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

    // Migration: add slow_variance column to signal_state
    if !column_exists(conn, "signal_state", "slow_variance")? {
        conn.execute_batch(
            "ALTER TABLE signal_state ADD COLUMN slow_variance REAL NOT NULL DEFAULT 0.0;",
        )?;
        // Backfill: use fast variance as fallback for existing rows
        conn.execute_batch(
            "UPDATE signal_state SET slow_variance = variance WHERE slow_variance = 0.0;",
        )?;
        tracing::info!("migrated: added signal_state.slow_variance");
    }

    // Migration: create open_positions table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS open_positions (
            decision_id INTEGER NOT NULL,
            market_id TEXT NOT NULL,
            side TEXT NOT NULL,
            entry_price REAL NOT NULL,
            size REAL NOT NULL,
            fee_rate REAL NOT NULL,
            entry_ts INTEGER NOT NULL,
            estimated_slippage REAL NOT NULL DEFAULT 0.0,
            PRIMARY KEY (market_id)
        );",
    )?;

    // Migration: add condition_id to markets table (for resolution lookup on restart)
    if !column_exists(conn, "markets", "condition_id")? {
        conn.execute_batch(
            "ALTER TABLE markets ADD COLUMN condition_id TEXT NOT NULL DEFAULT '';",
        )?;
        tracing::info!("migrated: added markets.condition_id");
    }

    // Migration: create fill_rejections table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS fill_rejections (
            id INTEGER PRIMARY KEY,
            market_id TEXT NOT NULL,
            side TEXT NOT NULL,
            size REAL NOT NULL,
            price REAL NOT NULL,
            reason TEXT NOT NULL,
            ts INTEGER NOT NULL
        );",
    )?;

    // Migration: create weather_forecasts table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS weather_forecasts (
            id INTEGER PRIMARY KEY,
            city TEXT NOT NULL,
            target_date TEXT NOT NULL,
            model TEXT NOT NULL,
            member INTEGER NOT NULL,
            temp_max REAL NOT NULL,
            fetched_ts INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_wf_city_date
            ON weather_forecasts(city, target_date);",
    )?;

    // Migration: create weather_markets table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS weather_markets (
            id INTEGER PRIMARY KEY,
            event_id TEXT NOT NULL,
            city TEXT NOT NULL,
            target_date TEXT NOT NULL,
            bucket_index INTEGER NOT NULL,
            bucket_label TEXT NOT NULL,
            bucket_lo REAL,
            bucket_hi REAL,
            token_yes TEXT NOT NULL,
            token_no TEXT NOT NULL,
            best_bid REAL,
            best_ask REAL,
            midpoint REAL,
            p_ensemble REAL,
            edge REAL,
            ts INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_wm_city_date
            ON weather_markets(city, target_date);",
    )?;

    Ok(())
}

/// Check if a column exists in a table using PRAGMA table_info.
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let found = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|r| r.as_deref() == Ok(column));
    Ok(found)
}
