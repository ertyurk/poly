# Local Profitability Dashboard

A small local web app for monitoring the bot in real time from SQLite: account state, fees, open positions, spot prices, and historical profitability buckets.

## Run it

```bash
cargo run --bin dashboard -- --db data/bot.db --port 3030
```

Then open `http://127.0.0.1:3030`.

## Architecture

The dashboard is intentionally simple and local-first:

### 1. Local HTTP server — `src/bin/dashboard.rs`

- Starts a tiny Rust server on `127.0.0.1`
- Serves the embedded HTML app from `assets/dashboard.html`
- Exposes a JSON endpoint at `/api/bootstrap`
- Reloading the page re-reads the current SQLite database

This keeps the workflow dead simple: no Node, no bundler, no frontend toolchain.

### 2. Data loader — `src/dashboard.rs`

Builds one denormalized payload from SQLite:

- `trades`: settled trades joined with markets, decisions, and book metadata
- `open_positions`: current open book with mark-to-market estimates from the latest YES book
- `spot_points`: recent BTC/ETH spot ticks for live price cards and charts
- `fill_rejections`: execution failures grouped by reason
- `skips`: optional regret analysis data
- `filters`: unique assets/windows/sides/outcomes for drill-down views

It also infers the starting bankroll, the current realized bankroll, and an estimated total equity figure.

### 3. UI — `assets/dashboard.html`

A single self-contained HTML file with inline CSS and vanilla JavaScript.

The frontend:

- loads the full payload once
- applies filters entirely in the browser
- recomputes metrics instantly
- renders charts/tables without external libraries

This makes it easy to tweak the UI or add new filters without introducing a heavy stack.

## Current filters

The dashboard currently supports:

- asset
- window
- side
- auto-refresh on/off
- refresh interval

These are enough to answer the most important monitoring questions quickly:

- What is my realized bankroll right now?
- What is my estimated total equity with open positions marked?
- Which open positions are hurting or helping?
- Which asset/window/side buckets are actually paying?

## Current panels

- live account metrics (realized bankroll, estimated equity, fees, open exposure)
- auto-refreshing spot cards and spot charts
- realized equity curve
- fee accumulation chart
- P&L by asset / window / side
- open positions mark-to-market table
- recent activity feed
- execution rejection summary
- recent settled trades table

## Important note on historical data quality

The current bot appears to have historical rows where `trades.decision_id` is not always a reliable foreign key to `decisions.id`.

The dashboard works around this by:

- using the decision row by `id` when available
- otherwise falling back to the nearest `TRADE` decision for the same `market_id`

That makes the dashboard usable on existing data, but the underlying bot data linkage should still be cleaned up.

## Good next extensions

If you want to push this further, the next best additions are:

1. add `confidence` and `p_hat` to the dashboard payload
2. add spread-at-decision and fee-at-decision explicitly to the database
3. add presets like `strict`, `balanced`, `aggressive`
4. add a saved-filter URL state so your views are shareable
5. add a bucket-level recommendation engine that highlights the highest-EV cuts automatically
