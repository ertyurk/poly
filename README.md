# Poly

Trading bot for [Polymarket](https://polymarket.com) prediction markets. Supports crypto (BTC/ETH up/down) and weather (temperature) markets.

Uses a log-normal probability model, LMSR pricing, and Kelly-criterion position sizing. Supports **paper trading** (simulated execution) and **live trading** (real orders via Polymarket CLOB API).

> **Disclaimer:** This is a thought experiment repository. We spent some hours experimenting with trading logic — all BTC/ETH markets are being won consistently in paper trade (only execution is imitated, data feeds are correct from Polymarket as well as BinanceWS). However, in reality, since we don't have private network ties to Polymarket's infrastructure, order book fill latency from network roundtrips is the bottleneck. The models and decision logic work; execution in a real-time scenario is the problem. Even though this is Rust with sub-microsecond local compute, you cannot overcome the reality of network infrastructure without co-location or dedicated connectivity. **Use at your own risk. This is not financial advice.**

## Install

```bash
# Install globally as `poly`
cargo install --path .

# Or with private key baked into the binary (no .env needed)
PRIVATE_KEY=your_hex_key cargo install --path .
```

## Quick start

```bash
# Paper trade crypto with $100 bankroll
poly crypto --paper --bankroll 100

# Live trade crypto (requires PRIVATE_KEY)
poly crypto --bankroll 54

# Paper trade BTC 5-minute markets only
poly crypto --paper --bankroll 100 --asset btc --window 5m

# Paper trade weather markets
poly weather --paper --bankroll 50

# Check bot status
poly status

# Launch dashboard in another terminal
poly dashboard

# Reset database for clean state
poly reset-db
```

## Commands

```
poly <COMMAND> [OPTIONS]

Commands:
  crypto      Run crypto prediction market trader
  weather     Run weather prediction market trader
  dashboard   Launch web dashboard (separate terminal)
  status      Show quick database status report
  reset-db    Remove database for clean state

Global options:
  --config <PATH>     Config file [default: ~/.polymarket/config.toml]
  --db-path <PATH>    Database file [default: ~/.polymarket/data.db]
```

### `poly crypto`

```
poly crypto [OPTIONS]

Options:
  --paper-trade, --paper    Simulated execution (no API keys needed)
  --bankroll <USD>          Starting bankroll (overrides config)
  --asset <btc|eth|all>     Asset filter [default: all]
  --window <5m|15m|all>     Window filter [default: all]
```

### `poly weather`

```
poly weather [OPTIONS]

Options:
  --paper-trade, --paper    Simulated execution
  --bankroll <USD>          Starting bankroll
```

### `poly dashboard`

```
poly dashboard [OPTIONS]

Options:
  --host <HOST>    Bind address [default: 127.0.0.1]
  --port <PORT>    Port [default: 3030]
```

Open `http://127.0.0.1:3030` in your browser. Reads from the same DB the trader writes to.

### `poly status`

Quick terminal report — decisions, trades, P&L, open positions, skip reasons.

### `poly reset-db`

Removes `data.db`, `data.db-wal`, `data.db-shm` for a clean start.

## Data directory

Everything lives in `~/.polymarket/`:

```
~/.polymarket/
├── config.toml    # auto-created on first run with defaults
├── data.db        # SQLite database
└── .env           # private key (optional, runtime fallback)
```

Override with `--config` or `--db-path` flags.

## Private key

Three-tier fallback for the Polymarket signing key:

1. **Compile-time** — `PRIVATE_KEY=xxx cargo install --path .` (baked into binary)
2. **`.env` file** — `~/.polymarket/.env` or project-local `.env`
3. **Environment variable** — `PRIVATE_KEY=xxx poly crypto`

Paper mode needs no key.

## Paper vs live

Both modes use identical data pipelines — only order execution differs:

| Component                       | Paper         | Live         |
| ------------------------------- | ------------- | ------------ |
| Polymarket market discovery     | Real          | Real         |
| Order books (CLOB API)          | Real          | Real         |
| Binance spot prices (WebSocket) | Real          | Real         |
| Signal model + decision engine  | Real          | Real         |
| **Order execution**             | **Simulated** | **CLOB API** |
| API key required                | No            | Yes          |

## How it works

```
Binance WS ──► Ingest ──► Signal ──► Decision ──► Executor
                            │           │             │
Polymarket ──► Fetcher ─────┘           │             ├──► Telegram
  (Gamma API)                           │             │
  (CLOB API)                            │             ▼
                                        │        SQLite Writer
                                        │
                              Open-Meteo ──► Weather Signal (coming soon)
```

Seven async actors connected by `mpsc` channels:

| Actor              | Role                                                |
| ------------------ | --------------------------------------------------- |
| **Ingest**         | Binance WebSocket for real-time spot ticks          |
| **Market Fetcher** | Polymarket market discovery + order books           |
| **Signal**         | Log-normal CDF probability model                    |
| **Decision**       | Edge gating + quarter-Kelly sizing                  |
| **Executor**       | Paper fills or live CLOB orders (GTD→FOK lifecycle) |
| **Writer**         | Batched SQLite (100 events / 500ms)                 |
| **Telegram**       | Trade alerts + periodic summaries                   |

## Configuration

Config lives at `~/.polymarket/config.toml` (auto-created on first run).

Key settings:

| Setting                       | Default | Description                |
| ----------------------------- | ------- | -------------------------- |
| `strategy.tau_min`            | 0.03    | Minimum edge threshold     |
| `strategy.kelly_fraction`     | 0.25    | Quarter-Kelly sizing       |
| `strategy.max_bet_fraction`   | 0.10    | Max 10% bankroll per trade |
| `strategy.max_total_exposure` | 0.50    | Max 50% total exposure     |
| `execution.gtd_expiry_secs`   | 7       | GTD order timeout          |
| `execution.fok_price_bump`    | 0.03    | FOK fallback price bump    |
| `telegram.enabled`            | true    | Enable/disable alerts      |

See `config.toml` for all options with inline docs.

## Database

SQLite with WAL mode. Ten tables tracking spot prices, markets, signals, decisions, trades, and positions.

```bash
# Quick P&L check
poly status

# Manual query
sqlite3 ~/.polymarket/data.db \
  "SELECT count(*) as trades, sum(pnl) as pnl FROM trades"
```

### Restart resilience

On startup, the bot restores: bankroll, open positions, signal warm-up state, and decision ID sequence.

## Code quality

- `#![forbid(unsafe_code)]`
- Strict clippy: pedantic + nursery, `unwrap`/`expect`/`panic` denied
- Release: LTO, `codegen-units = 1`, symbols stripped
- All tests: `cargo test`

## License

MIT
