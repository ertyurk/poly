# polymarket-bot

Trading bot for [Polymarket](https://polymarket.com) crypto prediction markets. Connects to Binance for real-time spot prices and Polymarket for real market data, order books, and trade execution. Uses a log-normal probability model, LMSR pricing, and Kelly-criterion position sizing.

Supports **paper trading** (real data, simulated execution) and **live trading** (real orders via Polymarket CLOB API with EIP-712 signing).

## Usage

```bash
cargo run -- [OPTIONS]

Options:
    --asset <btc|eth|all>       Asset filter [default: all]
    --window <5m|15m|all>       Window filter [default: all]
    --bankroll <USD>            Starting bankroll (overrides config.toml)
    --paper-trade / --dry-run   Paper mode: real data, simulated execution
    --config <PATH>             Config file [default: config.toml]
```

### Examples

```bash
# Paper trade BTC 5-minute markets with $100
cargo run -- --paper-trade --asset btc --window 5m --bankroll 100

# Paper trade all crypto markets with $500
cargo run -- --paper-trade --bankroll 500

# Paper trade ETH only, all windows
cargo run -- --paper-trade --asset eth

# Live trading (requires API keys in .env)
cargo run -- --asset btc --bankroll 1000

# Live trading with debug logging
RUST_LOG=polymarket_bot=debug cargo run -- --bankroll 5000
```

### Paper trade vs live trade

Both modes use **identical data pipelines** — the only difference is order execution:

| Component | Paper mode | Live mode |
|---|---|---|
| Polymarket market discovery (Gamma API) | Real | Real |
| Order books (CLOB API, refreshed every 5s) | Real | Real |
| Binance spot prices (WebSocket) | Real | Real |
| Log-normal signal model | Real | Real |
| Decision engine (edge, fees, sizing) | Real | Real |
| Market resolution detection | Real | Real |
| Position & bankroll persistence | Real | Real |
| Telegram notifications | Real | Real |
| **Order execution** | **Simulated (with slippage)** | **CLOB API `POST /order`** |
| API keys required | No | Yes |

Paper mode gives you accurate P&L tracking against real market conditions without risking capital.

## Quick start

### Prerequisites

- Rust 1.75+ (edition 2021)
- Internet connection (Binance WebSocket + Polymarket API)

### Build and run

```bash
# Build
cargo build

# Build optimized release (full LTO enabled)
cargo build --release

# Run in paper-trade mode (no API keys needed)
cargo run -- --paper-trade --bankroll 100

# Run tests
cargo test
```

### Environment variables

The bot loads `.env` from the current directory automatically via `dotenvy`. Paper mode needs no env vars. Live mode requires:

```bash
# .env (in project root)
POLYMARKET_API_KEY=your_key
POLYMARKET_API_SECRET=your_secret_base64
POLYMARKET_PASSPHRASE=your_passphrase
PRIVATE_KEY=your_ethereum_private_key_hex
```

See `.env.example` for a template.

### Live trading setup

See [`docs/live-trading-setup.md`](docs/live-trading-setup.md) for the full step-by-step guide (API keys, wallet private key, EIP-712 signing, Telegram).

Quick version:
1. Copy `.env.example` to `.env`
2. Fill in your Polymarket API credentials + private key
3. Run without `--paper-trade`:
   ```bash
   cargo run -- --asset btc --bankroll 1000
   ```

## How it works

See [`docs/architecture.md`](docs/architecture.md) for the full system design, signal model math, decision pipeline, and data flow.

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

**Seven actors** run as async tokio tasks, connected by `mpsc` channels:

| Actor | Role |
|---|---|
| **Ingest** | Binance WebSocket for real-time BTC/ETH trades. Reconnects with exponential backoff. |
| **Market Fetcher** | Polls Polymarket Gamma API for crypto prediction markets. Fetches real order books from CLOB API every 5s. Detects market resolution. Parses market type (Above/Below/Between/UpDown) from question text. |
| **Signal** | Per-asset `AssetTracker` estimates realized volatility and drift from Binance ticks (EWM with dual-timescale drift). Computes `p_hat = P(S_T > K)` via log-normal CDF per `MarketType`. Requires 30+ ticks warm-up. |
| **Decision** | Edge gating: edge must exceed fees. Sizes with quarter-Kelly. Per-trade cap 10% of bankroll, total exposure cap 50%. |
| **Executor** | Paper mode: simulates fills with slippage model. Live mode: places EIP-712 signed orders. Positions persist across restarts. |
| **Writer** | Batched SQLite writer. Flushes every 100 events or 500ms. Fire-and-forget channel. |
| **Telegram** | Trade fill + settlement alerts. Periodic and shutdown summaries. Rate-limited (1 msg/sec). |

## Key math

| Concept | Formula | Source |
|---|---|---|
| Log-normal CDF | `p_hat = Φ((ln(S/K) + μT) / (σ√T))` | Black-Scholes-style |
| EWM variance | `σ² += λ(r² - σ²)` with dual-timescale drift (fast ~5min, slow ~20min) | Exponential moving avg |
| LMSR cost | `C(q) = b * ln(Σ e^(q_i/b))` | Hanson 2003 |
| Quarter-Kelly | `f = kelly_fraction * (p_hat - p) / (1 - p)` | Kelly criterion / 4 |
| Entry gate | `effective_edge = \|p_hat - market_price\| - fee_rate` | Edge must clear fees |
| Stealth cap | `size ≤ 0.02 * volume_24h` | Avoid moving the market |
| Paper slippage | `slippage = spread/2 + size/(size + $50k) * 0.02` | Linear impact model |

## Configuration

All settings in [`config.toml`](config.toml) with inline documentation. Key sections:

| Section | Controls |
|---|---|
| `[bankroll]` | Starting bankroll (USD) |
| `[strategy]` | Edge threshold, Kelly fraction, volume cap, confidence floor, LMSR liquidity, max exposure |
| `[binance]` | WebSocket URL and trade streams |
| `[polymarket]` | CLOB, Gamma API endpoints; polling intervals |
| `[writer]` | SQLite batch size and flush interval |
| `[telegram]` | Bot token, chat ID, summary interval (optional) |

## Database

SQLite with WAL mode, `synchronous = NORMAL`, foreign keys enabled. Idempotent schema migrations on startup. Ten tables:

| Table | Contents |
|---|---|
| `spot_prices` | Every Binance trade tick |
| `markets` | Discovered Polymarket markets (with resolution tracking) |
| `book_snapshots` | Order book snapshots (bid, ask, midpoint, spread) |
| `signals` | Signal output (p_hat, confidence, prior, n_observations) |
| `decisions` | Every trade/skip with edge, fees, sizing |
| `trades` | Executed trades with P&L, bankroll state, estimated slippage |
| `config_snapshots` | Config JSON at startup |
| `open_positions` | Persisted open positions (survive restarts) |
| `signal_state` | Warm-up state for `AssetTracker` (vol, drift, slow_drift) |

### Restart resilience

On startup, the bot:
1. Restores bankroll from the last trade's `bankroll_after` (or config default)
2. Loads open positions from `open_positions` table into executor
3. Restores signal warm-up state (`AssetTracker` variance/drift) if saved within the last hour
4. Resumes `next_decision_id` from `MAX(id)` in decisions table

Position writes and signal state saves are **fire-and-forget** via the writer channel — no blocking on the hot path.

Query results anytime:

```bash
sqlite3 data/bot.db "SELECT COUNT(*) as trades, \
  SUM(CASE WHEN outcome='WIN' THEN 1 ELSE 0 END) as wins, \
  printf('\$%.2f', SUM(pnl)) as pnl, \
  printf('\$%.2f', (SELECT bankroll_after FROM trades ORDER BY resolved_ts DESC LIMIT 1)) as bankroll \
  FROM trades;"
```

See [`docs/dashboard-queries.md`](docs/dashboard-queries.md) for ready-to-use SQL queries.

## Project structure

```
polymarket-bot/
├── Cargo.toml
├── config.toml
├── rustfmt.toml
├── schema.sql
├── .env.example
├── src/
│   ├── main.rs               # CLI parsing, actor wiring, startup + restore
│   ├── cli.rs                 # clap CLI definition
│   ├── config.rs              # TOML config parsing
│   ├── types.rs               # Domain types and channel messages
│   ├── actors/
│   │   ├── ingest.rs          # Binance WebSocket consumer
│   │   ├── market_fetcher.rs  # Polymarket market discovery + order books
│   │   ├── signal.rs          # Log-normal signal engine (AssetTracker)
│   │   ├── decision.rs        # Edge gating and position sizing
│   │   ├── executor.rs        # Paper/live trade execution + position persistence
│   │   ├── telegram.rs        # Telegram alerts + periodic summaries
│   │   └── writer.rs          # Batched SQLite writer
│   ├── polymarket/
│   │   ├── client.rs          # Gamma + CLOB API HTTP client
│   │   ├── auth.rs            # HMAC-SHA256 API authentication
│   │   ├── signing.rs         # EIP-712 order signing (Polygon)
│   │   └── types.rs           # API request/response types
│   ├── math/
│   │   ├── lmsr.rs            # LMSR pricing
│   │   └── kelly.rs           # Kelly criterion sizing
│   └── db/
│       ├── mod.rs             # SQLite init (WAL, foreign keys)
│       ├── schema.rs          # Table creation + migrations
│       └── queries.rs         # Insert/update/restore helpers
└── docs/
    ├── architecture.md
    ├── live-trading-setup.md
    └── dashboard-queries.md
```

## Telegram notifications

Optional. Add to `config.toml`:

```toml
[telegram]
bot_token = "123456:ABC-DEF..."   # from @BotFather
chat_id = "your_chat_id"          # from @userinfobot
summary_interval_mins = 60
```

You'll receive:
- **Trade Filled** — on every position entry (market, side, size, price, edge%)
- **Trade Settled** — on market resolution (outcome, P&L, fees, bankroll)
- **Periodic Summary** — every N minutes (win/loss/rate, total P&L, bankroll)
- **Final Session Summary** — on graceful shutdown (Ctrl+C)

## Code quality

- `#![forbid(unsafe_code)]`
- Strict clippy: pedantic + nursery warnings, `unwrap`/`expect`/`panic` denied
- Release: `opt-level = 3`, full LTO, `codegen-units = 1`, symbols stripped
- Zero warnings, `rustfmt` enforced (`max_width = 100`)
- Hot-path functions `#[inline]`
- `VecDeque` for O(1) observation window management
- `Copy` types on hot-path messages

## License

Private. Not for redistribution.
