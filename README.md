# polymarket-bot

Trading bot for [Polymarket](https://polymarket.com) crypto prediction markets. Connects to Binance for real-time spot prices and Polymarket for real market data, order books, and trade execution. Uses Bayesian signal processing, LMSR pricing, and Kelly-criterion position sizing.

Supports **paper trading** (real data, simulated execution) and **live trading** (real orders via Polymarket CLOB API with EIP-712 signing).

## Usage

```bash
polymarket-bot [OPTIONS]

Options:
    --asset <btc|eth|all>       Asset filter [default: all]
    --window <5m|15m|all>       Window filter [default: all]
    --bankroll <USD>            Starting bankroll (overrides config.toml)
    --paper-trade / --dry-run   Paper mode: real data, simulated execution
    -c, --config <PATH>         Config file [default: config.toml]
```

### Examples

```bash
# Paper trade BTC 5-minute markets with $100
polymarket-bot --paper-trade --asset btc --window 5m --bankroll 100

# Paper trade all crypto markets with $500
polymarket-bot --paper-trade --bankroll 500

# Paper trade ETH only, all windows
polymarket-bot --paper-trade --asset eth

# Live trading (requires API keys in .env)
polymarket-bot --asset btc --bankroll 1000

# Live trading with debug logging
RUST_LOG=polymarket_bot=debug polymarket-bot --bankroll 5000
```

### Paper trade vs live trade

Both modes use **identical data pipelines** — the only difference is order execution:

| Component | Paper mode | Live mode |
|---|---|---|
| Polymarket market discovery (Gamma API) | Real | Real |
| Order books (CLOB API, refreshed every 5s) | Real | Real |
| Binance spot prices (WebSocket) | Real | Real |
| Bayesian signal processing | Real | Real |
| Decision engine (edge, fees, sizing) | Real | Real |
| Market resolution detection | Real | Real |
| **Order execution** | **Simulated locally** | **CLOB API `POST /order`** |
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

# Build optimized release (LTO enabled)
cargo build --release

# Run in paper-trade mode (no API keys needed)
cargo run -- --paper-trade --bankroll 100

# Run tests
cargo test
```

### Live trading setup

1. Copy `.env.example` to `.env`
2. Fill in your Polymarket API credentials:
   ```
   POLYMARKET_API_KEY=your_key
   POLYMARKET_API_SECRET=your_secret_base64
   POLYMARKET_PASSPHRASE=your_passphrase
   PRIVATE_KEY=your_ethereum_private_key_hex
   ```
3. Run without `--paper-trade`:
   ```bash
   cargo run -- --asset btc --bankroll 1000
   ```

## How it works

```
Binance WS ──► Ingest ──► Signal Engine ──► Decision Engine ──► Executor
  (spot prices)              (Bayesian)       (edge/sizing)     (paper or live)
                                                                      │
Polymarket ──► Market Fetcher ──► Signal + Decision                   │
  (Gamma API)   (real markets,      (real prices,                     │
  (CLOB API)     order books)        real fees)                       │
                                                                      ▼
                                                               SQLite Writer
                                                             (batched, WAL mode)
```

**Six actors** run as async tokio tasks, connected by `mpsc` channels:

| Actor | Role |
|---|---|
| **Ingest** | Binance WebSocket for real-time BTC/ETH trades. Reconnects with exponential backoff. |
| **Market Fetcher** | Polls Polymarket Gamma API for crypto prediction markets. Fetches real order books from CLOB API every 5s. Detects market resolution. |
| **Signal** | Per-market Bayesian state. Computes `p_hat` (probability of UP) using decay-weighted log-likelihood ratios. |
| **Decision** | Edge gating: edge must exceed fees + spread. Sizes with half-Kelly capped by LMSR optimal size and stealth constraint (2% of 24h volume). |
| **Executor** | Paper mode: simulates fills at real prices. Live mode: places EIP-712 signed orders on Polymarket CLOB API. |
| **Writer** | Batched SQLite writer. Flushes every 100 events or 500ms. |

## Key math

| Concept | Formula | Source |
|---|---|---|
| LMSR cost | `C(q) = b * ln(Σ e^(q_i/b))` | Hanson 2003 |
| Bayesian update | `log P(UP\|D) = log P(UP) + Σ log P(D_k\|UP)` | Sequential log-space |
| Signal decay | `w_k = exp(-λ * (t - t_k))`, λ = 0.00230 (5-min half-life) | Exponential weighting |
| Half-Kelly | `f = (p_hat - p) / 2(1 - p)` | Kelly criterion / 2 |
| Entry gate | `\|edge\| > τ_min + p(1-p)/b * δ_min` | Edge must clear fees + spread |
| Stealth cap | `size ≤ 0.02 * volume_24h` | Avoid moving the market |

## Configuration

All settings in [`config.toml`](config.toml) with inline documentation. Key sections:

| Section | Controls |
|---|---|
| `[bankroll]` | Starting bankroll (USD) |
| `[strategy]` | Edge threshold, Kelly fraction, volume cap, confidence floor, LMSR liquidity |
| `[strategy.decay]` | Exponential decay rates for signal sources |
| `[binance]` | WebSocket URL and trade streams |
| `[polymarket]` | CLOB, Gamma API endpoints; polling intervals |
| `[writer]` | SQLite batch size and flush interval |

## Database

SQLite with WAL mode, `synchronous = NORMAL`, foreign keys enabled. Seven tables:

| Table | Contents |
|---|---|
| `spot_prices` | Every Binance trade tick |
| `markets` | Discovered Polymarket markets |
| `book_snapshots` | Order book snapshots (bid, ask, midpoint, spread) |
| `signals` | Bayesian signal output (p_hat, confidence) |
| `decisions` | Every trade/skip with edge, fees, sizing |
| `trades` | Executed trades with P&L and bankroll state |
| `config_snapshots` | Config JSON at startup |

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
├── schema.sql
├── .env.example
├── src/
│   ├── main.rs               # CLI parsing, actor wiring, startup
│   ├── cli.rs                 # clap CLI definition
│   ├── config.rs              # TOML config parsing
│   ├── types.rs               # Domain types and channel messages
│   ├── actors/
│   │   ├── ingest.rs          # Binance WebSocket consumer
│   │   ├── market_fetcher.rs  # Polymarket market discovery + order books
│   │   ├── signal.rs          # Bayesian signal engine
│   │   ├── decision.rs        # Edge gating and position sizing
│   │   ├── executor.rs        # Paper/live trade execution
│   │   └── writer.rs          # Batched SQLite writer
│   ├── polymarket/
│   │   ├── client.rs          # Gamma + CLOB API HTTP client
│   │   ├── auth.rs            # HMAC-SHA256 API authentication
│   │   ├── signing.rs         # EIP-712 order signing (Polygon)
│   │   └── types.rs           # API request/response types
│   ├── math/
│   │   ├── lmsr.rs            # LMSR pricing
│   │   ├── bayesian.rs        # Log-space Bayesian updates
│   │   ├── kelly.rs           # Kelly criterion sizing
│   │   └── decay.rs           # Exponential decay weighting
│   └── db/
│       ├── mod.rs             # SQLite init (WAL, foreign keys)
│       ├── schema.rs          # Table creation
│       └── queries.rs         # Insert/update helpers
└── docs/
    └── dashboard-queries.md
```

## Code quality

- `#![forbid(unsafe_code)]`
- Strict clippy: pedantic + nursery warnings, `unwrap`/`expect`/`panic` denied
- Release: `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`, symbols stripped
- Zero warnings
- Hot-path functions `#[inline]`
- `VecDeque` for O(1) observation window management
- `Copy` types on hot-path messages

## License

Private. Not for redistribution.
