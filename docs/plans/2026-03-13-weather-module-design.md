# Weather Module Design

**Date:** 2026-03-13
**Goal:** Add weather temperature market trading to the bot. Additive only — zero changes to crypto code paths.

## Edge Thesis

Polymarket weather markets have 9 temperature buckets per city per day (~20 cities). The **tail buckets** (extreme low/high, priced $0.01–$0.10) are systematically underpriced because most bettors use simple forecasts (AccuWeather). ECMWF ensemble (51 members) + GFS (31 members) = 82 independent atmosphere simulations give us real tail probabilities that the market ignores.

**Validated by:**
- MIT professor: $300 → $66K trading tail buckets priced $0.01–$0.10
- RL bot independently abandoned crypto (3%) and specialized in weather (34%)
- Nature paper: ECMWF ensembles have "almost perfect tail behaviour" for temperature

## Architecture

### New Files (additive only)

```
src/weather/
  mod.rs          — module root, re-exports
  fetcher.rs      — Gamma API: discover weather events, parse buckets, poll order books
  forecast.rs     — Open-Meteo ensemble API: fetch ECMWF + GFS daily max temperature
  signal.rs       — count ensemble members per bucket → p_hat, emit signals for tail edges
  types.rs        — WeatherMarket, Bucket, Forecast, CityConfig
```

### Touched (minimal)

- `src/lib.rs` — add `pub mod weather;`
- `src/main.rs:run_weather()` — fill in stub, wire channels
- `src/db/schema.rs` — add weather-specific tables (forecasts, weather_markets)
- `src/db/queries.rs` — insert functions for weather tables
- `src/main.rs:run_status()` — extend to show weather summary
- `.gitignore` — add `dev_weather.db`

### NOT Touched

- `src/actors/signal.rs` — crypto signal, frozen
- `src/actors/decision.rs` — reused as-is
- `src/actors/executor.rs` — reused as-is
- `src/actors/market_fetcher.rs` — crypto fetcher, frozen
- `src/actors/ingest.rs` — Binance WS, frozen
- `src/flow.rs` — crypto order flow, frozen

### Channel Graph

```
Timer (30 min poll)
  └─► WeatherFetcher (Gamma API tag=weather)
        ├─► forecast.rs (Open-Meteo ensemble per city)
        │     └─► WeatherSignal (count members per bucket)
        │           └─► DecisionInput::Signal ──► existing DecisionActor
        ├─► DecisionInput::Market ──► existing DecisionActor
        ├─► SettleCommand ──► existing Executor
        └─► DbEvent ──► existing Writer
```

## Data Flow — One Poll Cycle (every 30 min)

1. **Discover markets** — `GET gamma-api.polymarket.com/events?tag=weather&closed=false`
   - Parse: city, date, 9 bucket markets, token IDs, order book prices
   - Filter: resolving within 36 hours
   - Map city → (lat, lon, temp_unit, ICAO station)

2. **Fetch forecasts** — `GET ensemble-api.open-meteo.com/v1/ensemble`
   - Params: `daily=temperature_2m_max`, `models=ecmwf_ifs025,gfs025_ensemble`
   - One call per city per resolution date
   - Returns 51 ECMWF + 31 GFS = 82 members of daily max temp

3. **Compute bucket probabilities**
   - Parse bucket boundaries from `groupItemTitle` ("74-75°F" → lo=74, hi=75)
   - Count ensemble members in each bucket → `p_hat = count / 82`
   - Focus on tail buckets (first 2 + last 2) where `market_price < 0.10`

4. **Emit signals for edges**
   - `edge = p_hat - market_price`
   - If `edge > threshold` (e.g. 0.03) → emit Signal to DecisionActor
   - Decision actor applies Kelly sizing + exposure limits (same as crypto)

5. **Execute** — Executor places GTD order via CLOB API (same lifecycle as crypto)

## City Configuration

Hardcoded lookup table (lat/lon/unit/station):

```rust
struct CityConfig {
    name: &'static str,       // "atlanta"
    lat: f64,                  // 33.75
    lon: f64,                  // -84.39
    temp_unit: TempUnit,       // Fahrenheit or Celsius
    icao: &'static str,       // "KATL"
    bucket_width: u8,          // 2 (°F cities) or 1 (°C cities)
}
```

~20 cities. No filtering — trade all cities, maximize tail opportunities.

## Bucket Parsing

From Polymarket `groupItemTitle`:
- `"65°F or below"` → Bucket { lo: None, hi: 65 }
- `"66-67°F"` → Bucket { lo: 66, hi: 67 }
- `"80°F or higher"` → Bucket { lo: 80, hi: None }

Same for °C with 1-degree width.

## Config

New `[weather]` section in config.toml:

```toml
[weather]
poll_interval_secs = 1800       # 30 minutes
max_forecast_horizon_hours = 36 # only trade markets resolving within this
edge_threshold = 0.03           # minimum edge to trade
max_tail_price = 0.10           # only trade buckets priced below this
tail_buckets = 3                # how many buckets from each end count as "tail"
```

## DB Tables (new)

```sql
CREATE TABLE weather_forecasts (
    id INTEGER PRIMARY KEY,
    city TEXT NOT NULL,
    target_date TEXT NOT NULL,
    model TEXT NOT NULL,          -- "ecmwf" or "gfs"
    member INTEGER NOT NULL,
    temp_max REAL NOT NULL,
    fetched_ts INTEGER NOT NULL
);

CREATE TABLE weather_markets (
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
    p_ensemble REAL,             -- our probability from ensemble counting
    edge REAL,
    ts INTEGER NOT NULL
);
```

Existing tables (decisions, trades, open_positions) reused for weather trades — `market_id` format: `WX_{city}_{date}_{bucket}` (e.g. `WX_atlanta_20260314_8`).

## Status Command Extension

```
poly status
=== CRYPTO ===
Trades: 3 | Wins: 1 | Losses: 0 | PnL: +$2.40
Open: 2 positions

=== WEATHER ===
Cities: 18 | Forecasts: 82 members × 18 cities
Tail signals: 4 | Trades: 2 | Wins: 1 | PnL: +$8.50
```

## Development Isolation

- `cargo test` → in-memory temp databases
- `cargo run -- weather --paper --db-path ./dev_weather.db` → local dev database
- Production `~/.polymarket/data.db` untouched until user approves `cargo install`
- `dev_weather.db` added to `.gitignore`

## Future (not in scope)

- Bias correction from forecast-vs-actual historical data (Option B)
- Precipitation markets
- ML model overlay
- RL-based position sizing
