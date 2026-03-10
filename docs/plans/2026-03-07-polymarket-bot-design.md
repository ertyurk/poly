# Polymarket Paper-Trading Bot — Design Document

**Date:** 2026-03-07
**Status:** Approved (updated 2026-03-10)

## Overview

Rust-based paper-trading bot for Polymarket crypto prediction markets (BTC/ETH, 5m/15m/1h/1d windows). Implements LMSR pricing, log-normal probability model, quarter-Kelly position sizing, and fee-aware edge gating. Logs all activity to SQLite (WAL mode) for dashboard reporting. Supports Telegram notifications and position persistence across restarts.

## Architecture: Actor-Style Monolith

Single binary, tokio async runtime. Seven actors communicating via `tokio::mpsc` channels.

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

**Log-normal probability model** (replaced Bayesian momentum accumulator in 2026-03-09):

Per-asset `AssetTracker` computes:
1. **EWM variance** from Binance tick log-returns: `σ² += λ(r² - σ²)`
2. **Dual-timescale drift:** fast (~5min) and slow (~20min) exponential moving averages
3. **Market-specific p_hat** via log-normal CDF: `p_hat = Φ((ln(S/K) + μT) / (σ√T))`

`MarketType` enum determines CDF calculation:
- `Above(strike)` — YES = price > strike at resolution
- `Below(strike)` — YES = price < strike at resolution
- `Between(lo, hi)` — YES = lo < price < hi at resolution
- `UpDown` — YES = price went up from open

Requires `MIN_TICKS = 30` before emitting signals (vol estimate stability).

Emits: `Signal { market_id, p_hat, confidence, prior, n_observations, ts }`

State persisted on shutdown for warm restart (variance, drift, slow_drift, lambda).

## Actor 3: Decision Engine

Entry and sizing pipeline:

1. **Edge:** `edge = p_hat - market_price`
2. **Fee-adjusted edge:** `effective_edge = |edge| - fee_rate`
3. **Entry gate:** `effective_edge > min_edge_threshold`
4. **Quarter-Kelly sizing:** `f = kelly_fraction * edge / (1 - market_price)`, capped at `max_bet_fraction * bankroll`
5. **Stealth constraint:** `size = min(size, 0.02 * volume_24h)`
6. **Side:** `p_hat > market_price` → YES, otherwise → NO
7. **Minimum order:** $5 (Polymarket minimum)

Risk controls:
- `max_bet_fraction = 0.10` — per-trade cap (10% of bankroll)
- `max_total_exposure = 0.50` — total committed capital cap (50%)
- `kelly_fraction = 0.25` — quarter-Kelly for conservative sizing

Emits: `TradeDecision { ... }` or `NoTrade { ... , reason: SkipReason }`

## Actor 4: Executor (Paper/Live)

- **Paper mode:** Simulates fill with slippage model (half-spread + linear impact). Fill price clamped to [0.01, 0.95] with max 5¢ slippage.
- **Live mode:** Places EIP-712 signed orders on Polymarket CLOB API.
- Tracks positions and bankroll. Settles on market resolution ($1 if correct, $0 if wrong, minus fees).
- **One position per market** enforced. Total exposure checked before each fill.
- **Positions persist** to `open_positions` table (fire-and-forget writes, sync load on startup).
- **Bankroll auto-restores** from last trade's `bankroll_after` on restart.

Emits: `TradeResult { decision_id, market_id, side, entry_price, size, fee_paid, outcome, pnl, bankroll_after, estimated_slippage }`

## Actor 6: Telegram (optional)

- Sends alerts on trade fills and settlements
- Periodic summary (configurable interval, default 60min)
- Final session summary on graceful shutdown
- Rate-limited to 1 message/sec to avoid Telegram API limits
- Validates HTTP status and Telegram `ok` field on every send

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

See `schema.sql` for the full schema. Idempotent migrations run on startup (safe on existing DBs). Key additions: `open_positions`, `signal_state` tables; `estimated_slippage` column on trades; `slow_drift` column on signal_state.

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
      signal.rs         # Log-normal model (AssetTracker)
      decision.rs
      executor.rs       # Paper/live + position persistence
      telegram.rs       # Telegram alerts + summaries
      writer.rs
    math/
      mod.rs
      kelly.rs
      lmsr.rs
    db/
      mod.rs
      schema.rs         # Table creation + migrations
      queries.rs        # Insert/update/restore helpers
  data/
    bot.db              (gitignored)
  docs/
    dashboard-queries.md
    plans/
```

## Configuration

See `config.toml` for all settings with inline documentation. Key sections: `[general]`, `[bankroll]`, `[strategy]` (edge thresholds, Kelly fraction, exposure limits), `[binance]`, `[polymarket]`, `[writer]`, `[telegram]` (optional).

## Startup Flow

1. Parse CLI + load config.toml
2. Initialize SQLite (WAL mode, run idempotent migrations)
3. **Restore state from DB:**
   - Bankroll from last trade's `bankroll_after` (CLI override wins)
   - Open positions from `open_positions` table → executor
   - Signal warm-up state from `signal_state` (if < 1 hour old) → signal actor
   - `next_decision_id` from `MAX(id)` in decisions
4. Snapshot config to `config_snapshots` table
5. Create tokio::mpsc channels between actors
6. Spawn all actors as tokio tasks (including telegram if configured)
7. Wait for ctrl+c → send shutdown signal → graceful drain
8. **Shutdown:** persist signal state, send final Telegram summary
