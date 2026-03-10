# Architecture

## Overview

Single Rust binary, async tokio runtime. Seven actors communicate via `mpsc` channels. All persistence is fire-and-forget through a batched SQLite writer. State survives restarts.

```
Binance WS ──► Ingest ──► Signal Engine ──► Decision Engine ──► Executor
  (spot prices)            (log-normal)       (edge/sizing)     (paper or live)
                                                                      │
Polymarket ──► Market Fetcher ──► Signal + Decision                   │
  (Gamma API)   (real markets,      (real prices,                     ▼
  (CLOB API)     order books)        real fees)              ┌──► Telegram
                                                             │   (alerts + summaries)
                                                             ▼
                                                        SQLite Writer
                                                      (batched, WAL mode)
```

## Actors

### 1. Ingest (`actors/ingest.rs`)

Binance WebSocket for real-time BTC/ETH trades. Reconnects with exponential backoff. Emits `SpotPrice { asset, price, ts }` on every trade tick.

### 2. Market Fetcher (`actors/market_fetcher.rs`)

Polls Polymarket Gamma Events API for crypto prediction markets. Fetches order books from CLOB API every 5s. Detects market resolution. Parses question text to determine `MarketType` (Above/Below/Between/UpDown) and extract strike prices.

Emits `MarketState` (market metadata + order book) and `SettleCommand` (on resolution).

### 3. Signal Engine (`actors/signal.rs`)

Per-asset `AssetTracker` estimates realized volatility and drift from Binance tick log-returns using exponential weighted moving averages.

**Volatility estimation:**
```
variance += alpha * (log_return^2 / dt - variance)
vol = sqrt(variance) * VOL_SAFETY_MARGIN
```

**Dual-timescale drift:**
- Fast drift (half-life ~5 min) — reacts to recent price action
- Slow drift (half-life ~20 min) — captures prevailing trend
- Both must agree with the signal direction before a trade is allowed

**Log-normal probability model:**

For each active market, computes market-specific `p_hat` via the log-normal CDF:

| MarketType | p_hat |
|---|---|
| `Above(K)` | `P(S_T > K) = Phi((ln(S/K) + mu*T) / (sigma*sqrt(T)))` |
| `Below(K)` | `1 - P(S_T > K)` |
| `Between(lo, hi)` | `P(S_T > lo) - P(S_T > hi)` |
| `UpDown` | `P(S_T > open_price)` — treated as `Above(open)` |

Where `S` = current spot, `K` = strike, `mu` = drift/sec, `sigma` = vol/sec, `T` = time to expiry in seconds. Uses logistic approximation for the CDF: `Phi(x) ~ 1/(1 + exp(-1.7x))`.

**Guards:**
- Requires `MIN_TICKS = 30` before emitting signals (vol stability)
- Only emits after 50% of the market window has elapsed
- Throttles to 1 signal/sec/market
- NO-only filter on UpDown markets (empirical: short-term crypto has structural NO bias)

### 4. Decision Engine (`actors/decision.rs`)

Receives signals and market state, decides whether to trade and how much.

**Pipeline:**

1. **Confidence check** — skip if confidence < `min_confidence`
2. **Market price filter** — only trade when midpoint is between 0.35 and 0.65 (edge is most exploitable near 50/50)
3. **Edge computation** — `edge = p_hat - market_price`
4. **Fee-adjusted edge** — `effective_edge = |edge| - fee_rate`; skip if <= 0
5. **Entry gate** — edge must also clear `tau_min + LMSR effective spread`
6. **Sizing** — `min(LMSR optimal size, Kelly size)`
7. **Bankroll cap** — `max_bet_fraction * bankroll` (default: 10%)
8. **Stealth cap** — `2% * volume_24h` to avoid moving the market
9. **Minimum order** — $5 (Polymarket minimum)
10. **Side** — `p_hat > market_price` => YES, else NO

**Fee model:** `fee_rate = 0.25 * (p * (1-p))^2` — peaks at ~1.56% at p=0.50, drops toward zero at extremes.

**Risk controls:**

| Parameter | Default | Purpose |
|---|---|---|
| `kelly_fraction` | 0.25 | Quarter-Kelly for conservative sizing |
| `max_bet_fraction` | 0.10 | Per-trade cap (10% of bankroll) |
| `max_total_exposure` | 0.50 | All open positions (50% of bankroll) |
| `tau_min` | 0.02 | Minimum edge threshold |
| `min_confidence` | 0.10 | Signal confidence floor |

### 5. Executor (`actors/executor.rs`)

Handles order execution and position management.

**Paper mode:**
- Simulates fill with slippage model: `slippage = spread/2 + size/(size+$50k) * 0.02`
- Fill price clamped to [0.01, 0.95] with max 5c slippage above market
- Returns `FillResult { decision_id, fill_price, estimated_slippage }`

**Live mode:**
- Places EIP-712 signed orders via Polymarket CLOB API
- Fetches dynamic fee rate per token before order placement

**Position management:**
- One position per market enforced
- Total exposure checked before each fill (committed capital < `max_total_exposure * bankroll`)
- Slippage tolerance: rejects fills where price slipped > 10% from decision price
- Settles on market resolution: $1 payout if correct, $0 if wrong, minus fees

### 6. Telegram (`actors/telegram.rs`)

Optional. Sends notifications via Telegram Bot API.

**Alert types:**
- **Trade Filled** — market, side, size, fill price, effective edge%
- **Trade Settled** — market, outcome (win/loss icon), P&L, fees, bankroll
- **Periodic Summary** — trades, win rate, total P&L, fees, bankroll, return%
- **Final Session Summary** — same as periodic, sent on graceful shutdown

Rate-limited to 1 msg/sec. Validates HTTP status and Telegram API `ok` field.

### 7. SQLite Writer (`actors/writer.rs`)

Fire-and-forget channel from all actors. Batches 100 events or 500ms (whichever first) into a single transaction.

SQLite pragmas: `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000`.

## Persistence & Restart Recovery

On startup, the bot synchronously reads from SQLite before spawning actors:

| What | Source | Fallback |
|---|---|---|
| Bankroll | `trades.bankroll_after` (last trade) | CLI flag or `config.toml` |
| Open positions | `open_positions` table | Empty (no positions) |
| Signal warm-up | `signal_state` table (if < 1 hour old) | Cold start (30 tick warm-up) |
| Decision ID sequence | `MAX(id)` from `decisions` table | Start at 1 |

At runtime, writes are fire-and-forget via the writer channel:
- `SaveOpenPosition` — on every fill
- `ClearOpenPositions` — on every settlement
- `SaveSignalState` — on graceful shutdown (per-asset vol/drift/slow_drift)

Schema migrations run on startup (idempotent `ALTER TABLE` + `CREATE TABLE IF NOT EXISTS`).

## Data Flow Example

A complete trade lifecycle:

1. **Binance tick** arrives via WebSocket → `SpotPrice { BTC, $97500, ts }`
2. **AssetTracker** updates EWM variance and drift
3. For market "BTC above $98k in 15m": `p_hat = Phi((ln(97500/98000) + drift*T) / (vol*sqrt(T))) = 0.38`
4. **Signal** emitted: `{ p_hat: 0.38, confidence: 0.24 }`
5. **Decision engine**: market midpoint = 0.50, edge = 0.38 - 0.50 = -0.12 (NO side), fee = 0.0156, effective_edge = 0.104
6. Kelly size computed, capped by bankroll/stealth limits
7. **Executor** fills at market price + slippage → `FillResult { fill_price: 0.495, slippage: 0.005 }`
8. Position stored in `open_positions`, Telegram notified
9. Market resolves NO → executor settles: `pnl = size * (1 - entry_price) - fees`
10. Bankroll updated, position cleared, Telegram notified with outcome

## Future Improvements

Ordered by priority:

1. **Realized volatility from tick buffer** — blend ring-buffer realized vol with EWM for faster regime adaptation
2. **Early exit on edge reversal** — sell back via CLOB when signal flips beyond threshold
3. **Binance funding rate signal** — additional directional input from perp funding rates
4. **Backtesting harness** — record/replay mode for offline strategy validation
