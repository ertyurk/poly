# Polymarket Paper-Trading Bot — Design Document

**Date:** 2026-03-07
**Status:** Approved

## Overview

Rust-based paper-trading bot for Polymarket crypto prediction markets (BTC/ETH, 5-min and 15-min windows). Implements LMSR pricing, Bayesian signal processing, half-Kelly position sizing, and fee-aware edge gating. Logs all activity to SQLite (WAL mode) for dashboard reporting.

## Architecture: Actor-Style Monolith

Single binary, tokio async runtime. Five actors communicating via `tokio::mpsc` channels.

```
+--------------------------------------------------------------+
|                        polymarket-bot                         |
|                                                               |
|  +--------------+    +--------------+    +---------------+    |
|  | Data Ingest  |    | Signal Engine|    | Decision      |    |
|  |              |    |              |    | Engine        |    |
|  | - Binance WS |--->| - Bayesian   |--->| - Edge calc   |    |
|  | - Polymarket |    |   updater    |    | - Half-Kelly  |    |
|  |   WS + REST  |    | - Signal     |    | - Fee-aware   |    |
|  | - Fee fetcher|    |   decay      |    | - Volume cap  |    |
|  +--------------+    | - Multi-src  |    +-------+-------+    |
|                      |   fusion     |            |            |
|                      +--------------+            v            |
|                                          +---------------+    |
|  +--------------+                        | Paper Executor|    |
|  | SQLite Writer|<-----------------------|               |    |
|  |              |    (all actors log)    | - Simulate    |    |
|  | - Trades     |                        |   fill/reject |    |
|  | - Signals    |                        | - Track P&L   |    |
|  | - Prices     |                        | - Stealth chk |    |
|  | - Metrics    |                        +---------------+    |
|  +--------------+                                             |
+--------------------------------------------------------------+
```

## Actor 1: Data Ingestion

Maintains persistent WebSocket connections:
- **Binance:** `btcusdt@trade`, `ethusdt@trade` streams
- **Polymarket:** order book updates for active 5m/15m markets

REST polling:
- Gamma API every 30s for new market windows (token IDs, resolution timestamps)
- Fee rate endpoint every 60s for dynamic taker fees

Emitted types:
- `SpotPrice { asset, price, ts }`
- `MarketState { market_id, token_yes, token_no, best_bid, best_ask, midpoint, resolution_ts }`
- `FeeUpdate { market_type, fee_schedule }`
- `MarketDiscovery { new markets }`

Reconnection: exponential backoff, max 5 retries then alert.

## Actor 2: Signal Engine

Bayesian pipeline (doc pages 2, 4):

1. **Prior:** 0.50 on new window open
2. **Sequential Bayesian update** in log-space: `log P(UP|D) = log P(UP) + sum(log P(D_k|UP)) - log Z`
3. **Signal decay:** `w_k = exp(-lambda * (t - t_k))`, lambda = 2.3e-3 (5-min half-life)
4. **Multi-source fusion:** `p_hat_fused = sum(w_s * p_hat_s / sigma_s^2) / sum(w_s / sigma_s^2)`

Emits: `Signal { market_id, p_hat, confidence, ts }`

State per window: prior, ring buffer (300 ticks), current p_hat, variance. Discarded 10s after resolution.

## Actor 3: Decision Engine

Entry and sizing pipeline (doc pages 3, 4):

1. **Edge:** `edge = p_hat - p_market`
2. **Fee-adjusted edge:** `effective_edge = |edge| - taker_fee(p_market)`
3. **Entry gate:** `effective_edge > tau_min + c_i` where `c_i = p_i(1-p_i)/b * delta_min`, tau_min = 0.05
4. **Optimal size:** `delta* = b * ln(p_hat/p * (1-p)/(1-p_hat))`
5. **Half-Kelly cap:** `f_prod = (p_hat - p) / 2(1-p)`, `max_position = f_prod * bankroll`
6. **Stealth constraint:** `delta = min(delta, 0.02 * volume_24h)`
7. **Side:** edge > 0 -> YES, edge < 0 -> NO

Emits: `TradeDecision { market_id, side, size, price, edge, fee, kelly_fraction }` or `NoTrade { market_id, reason }`

## Actor 4: Paper Executor

- Simulates fill against current order book state
- Tracks virtual positions and bankroll
- Settles on market resolution (payout $1 if correct, $0 if wrong, minus fees)
- No network calls — pure computation, swappable for LiveExecutor later

Emits: `TradeResult { decision_id, market_id, side, entry_price, size, fee_paid, outcome, pnl, bankroll_after }`

## Actor 5: SQLite Writer

Fire-and-forget channel from all actors. Write batching: 100 events or 500ms, whichever first. Single transaction per batch.

SQLite pragmas:
```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
```

Retention: `spot_prices` and `book_snapshots` auto-pruned after 7 days. All other tables kept indefinitely.

### Schema

```sql
CREATE TABLE spot_prices (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    asset TEXT NOT NULL,
    price REAL NOT NULL,
    ts INTEGER NOT NULL
);
CREATE INDEX idx_spot_asset_ts ON spot_prices(asset, ts);

CREATE TABLE markets (
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
CREATE INDEX idx_markets_asset_window ON markets(asset, window);
CREATE INDEX idx_markets_resolution ON markets(resolution_ts);

CREATE TABLE book_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    market_id TEXT NOT NULL REFERENCES markets(market_id),
    best_bid REAL,
    best_ask REAL,
    midpoint REAL,
    spread REAL,
    ts INTEGER NOT NULL
);
CREATE INDEX idx_book_market_ts ON book_snapshots(market_id, ts);

CREATE TABLE signals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    market_id TEXT NOT NULL REFERENCES markets(market_id),
    p_hat REAL NOT NULL,
    confidence REAL NOT NULL,
    prior REAL NOT NULL,
    n_observations INTEGER NOT NULL,
    ts INTEGER NOT NULL
);
CREATE INDEX idx_signals_market_ts ON signals(market_id, ts);

CREATE TABLE decisions (
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
CREATE INDEX idx_decisions_market ON decisions(market_id);
CREATE INDEX idx_decisions_action ON decisions(action);

CREATE TABLE trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    decision_id INTEGER NOT NULL REFERENCES decisions(id),
    market_id TEXT NOT NULL REFERENCES markets(market_id),
    side TEXT NOT NULL,
    entry_price REAL NOT NULL,
    size REAL NOT NULL,
    fee_paid REAL NOT NULL,
    outcome TEXT NOT NULL,
    pnl REAL NOT NULL,
    bankroll_after REAL NOT NULL,
    entry_ts INTEGER NOT NULL,
    resolved_ts INTEGER NOT NULL
);
CREATE INDEX idx_trades_outcome ON trades(outcome);
CREATE INDEX idx_trades_resolved ON trades(resolved_ts);

CREATE TABLE config_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    config_json TEXT NOT NULL,
    ts INTEGER NOT NULL
);
```

## Crate Layout

```
polymarket-bot/
  Cargo.toml
  config.toml
  src/
    main.rs
    config.rs
    types.rs
    actors/
      mod.rs
      ingest.rs
      signal.rs
      decision.rs
      executor.rs
      writer.rs
    math/
      mod.rs
      bayesian.rs
      kelly.rs
      lmsr.rs
      decay.rs
    db/
      mod.rs
      schema.rs
      queries.rs
  data/
    bot.db              (gitignored)
  docs/
    dashboard-queries.md
```

## Configuration

```toml
[general]
mode = "paper"
log_level = "info"
db_path = "data/bot.db"
db_retention_days = 7

[bankroll]
initial = 100000.0

[strategy]
tau_min = 0.05
kelly_fraction = 0.5
max_volume_pct = 0.02
min_confidence = 0.60
liquidity_b = 100000.0

[strategy.decay]
spot_lambda = 0.00230
news_lambda = 0.00019
social_lambda = 0.00039
onchain_lambda = 0.000096

[markets]
enabled = ["BTC_5m", "BTC_15m", "ETH_5m", "ETH_15m"]

[binance]
ws_url = "wss://stream.binance.com:9443/ws"
streams = ["btcusdt@trade", "ethusdt@trade"]

[polymarket]
clob_url = "https://clob.polymarket.com"
ws_url = "wss://ws-subscriptions-clob.polymarket.com"
gamma_url = "https://gamma-api.polymarket.com"
poll_interval_secs = 30
fee_refresh_secs = 60

[writer]
batch_size = 100
flush_interval_ms = 500
```

## Key Dependencies

```toml
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.21"
polymarket-client-sdk = "0.4"
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = "0.3"
```

## Startup Flow

1. Parse config.toml
2. Initialize SQLite (WAL mode, run migrations)
3. Snapshot config to config_snapshots table
4. Create tokio::mpsc channels between actors
5. Spawn all actors as tokio tasks
6. Wait for ctrl+c -> send shutdown signal -> graceful drain
