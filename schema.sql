CREATE TABLE IF NOT EXISTS spot_prices (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    asset TEXT NOT NULL,
    price REAL NOT NULL,
    ts INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_spot_asset_ts ON spot_prices(asset, ts);

CREATE TABLE IF NOT EXISTS markets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    market_id TEXT NOT NULL UNIQUE,
    asset TEXT NOT NULL,
    window TEXT NOT NULL,
    token_yes TEXT NOT NULL,
    token_no TEXT NOT NULL,
    open_ts INTEGER NOT NULL,
    resolution_ts INTEGER NOT NULL,
    resolved_side TEXT,
    open_price REAL
);
CREATE INDEX IF NOT EXISTS idx_markets_asset_window ON markets(asset, window);
CREATE INDEX IF NOT EXISTS idx_markets_resolution ON markets(resolution_ts);

CREATE TABLE IF NOT EXISTS book_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    market_id TEXT NOT NULL REFERENCES markets(market_id),
    best_bid REAL,
    best_ask REAL,
    midpoint REAL,
    spread REAL,
    ts INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_book_market_ts ON book_snapshots(market_id, ts);

CREATE TABLE IF NOT EXISTS signals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    market_id TEXT NOT NULL REFERENCES markets(market_id),
    p_hat REAL NOT NULL,
    confidence REAL NOT NULL,
    prior REAL NOT NULL,
    n_observations INTEGER NOT NULL,
    ts INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_signals_market_ts ON signals(market_id, ts);

CREATE TABLE IF NOT EXISTS decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    market_id TEXT NOT NULL REFERENCES markets(market_id),
    action TEXT NOT NULL,
    side TEXT,
    size REAL,
    price REAL,
    edge REAL NOT NULL,
    effective_edge REAL NOT NULL,
    fee_rate REAL NOT NULL,
    kelly_fraction REAL,
    skip_reason TEXT,
    ts INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_decisions_market ON decisions(market_id);
CREATE INDEX IF NOT EXISTS idx_decisions_action ON decisions(action);

CREATE TABLE IF NOT EXISTS trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    decision_id INTEGER NOT NULL REFERENCES decisions(id),
    market_id TEXT NOT NULL REFERENCES markets(market_id),
    side TEXT NOT NULL,
    entry_price REAL NOT NULL,
    size REAL NOT NULL,
    fee_rate REAL NOT NULL,
    fee_paid REAL NOT NULL,
    gross_pnl REAL NOT NULL,
    outcome TEXT NOT NULL,
    pnl REAL NOT NULL,
    bankroll_after REAL NOT NULL,
    entry_ts INTEGER NOT NULL,
    resolved_ts INTEGER NOT NULL,
    estimated_slippage REAL NOT NULL DEFAULT 0.0
);
CREATE INDEX IF NOT EXISTS idx_trades_outcome ON trades(outcome);
CREATE INDEX IF NOT EXISTS idx_trades_resolved ON trades(resolved_ts);

CREATE TABLE IF NOT EXISTS config_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    config_json TEXT NOT NULL,
    ts INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS signal_state (
    asset TEXT NOT NULL,
    last_price REAL NOT NULL,
    last_ts INTEGER NOT NULL,
    valid_ticks INTEGER NOT NULL,
    variance REAL NOT NULL,
    drift REAL NOT NULL,
    slow_drift REAL NOT NULL DEFAULT 0.0,
    lambda REAL NOT NULL,
    saved_at INTEGER NOT NULL,
    PRIMARY KEY (asset)
);
