# Dashboard Query Cookbook

Queries for building dashboards from the paper-trading bot's SQLite database.
All timestamps are unix microseconds. Connect to `data/bot.db` with any SQLite tool.

**DbPro element types:** Metric, Sparkline, Area Chart, Bar Chart, Pie Chart, Table, Progress, Text, Image, GIF, Map

## Quick Start — Run These First

### Overall Performance — `Metric` (run this first)

Use separate Metric elements for: **Net PnL**, **Win Rate**, **Total Trades**, **Final Bankroll**.

```sql
SELECT
    COUNT(*) AS total_trades,
    SUM(CASE WHEN outcome = 'WIN' THEN 1 ELSE 0 END) AS wins,
    SUM(CASE WHEN outcome = 'LOSS' THEN 1 ELSE 0 END) AS losses,
    ROUND(100.0 * SUM(CASE WHEN outcome = 'WIN' THEN 1 ELSE 0 END) / COUNT(*), 1) AS win_rate_pct,
    ROUND(SUM(pnl), 2) AS net_pnl,
    ROUND(SUM(fee_paid), 2) AS total_fees,
    ROUND(MIN(bankroll_after), 2) AS lowest_bankroll,
    ROUND(MAX(bankroll_after), 2) AS highest_bankroll,
    (SELECT ROUND(bankroll_after, 2) FROM trades ORDER BY resolved_ts DESC LIMIT 1) AS final_bankroll
FROM trades;
```

### Last 10 Trades — `Table`

```sql
SELECT
    t.market_id,
    t.side,
    ROUND(t.size, 2) AS bet_size,
    ROUND(t.entry_price, 3) AS entry_price,
    t.outcome,
    ROUND(t.pnl, 2) AS pnl,
    ROUND(t.fee_paid, 2) AS fee,
    ROUND(t.bankroll_after, 2) AS bankroll,
    datetime(t.resolved_ts / 1000000, 'unixepoch', 'localtime') AS resolved_at
FROM trades t
ORDER BY t.resolved_ts DESC
LIMIT 10;
```

---

## Detailed Dashboard Panels

### 1. Equity Curve — `Area Chart`

Cumulative P&L over time. X: time, Y: bankroll_after.

```sql
SELECT
    datetime(resolved_ts / 1000000, 'unixepoch', 'localtime') AS time,
    bankroll_after,
    SUM(pnl) OVER (ORDER BY resolved_ts) AS cumulative_pnl
FROM trades
ORDER BY resolved_ts;
```

### 2. Win Rate by Asset & Window — `Bar Chart`

```sql
SELECT
    m.asset,
    m.window,
    COUNT(*) AS total,
    SUM(CASE WHEN t.outcome = 'WIN' THEN 1 ELSE 0 END) AS wins,
    ROUND(100.0 * SUM(CASE WHEN t.outcome = 'WIN' THEN 1 ELSE 0 END) / COUNT(*), 1) AS win_pct,
    ROUND(SUM(t.pnl), 2) AS net_pnl
FROM trades t
JOIN markets m ON t.market_id = m.market_id
GROUP BY m.asset, m.window;
```

### 3. Edge vs. Outcome — `Bar Chart`

Did higher-edge trades actually win more often? X: edge_bucket, Y: avg_pnl.

```sql
SELECT
    CASE
        WHEN d.effective_edge < 0.05 THEN '0-5%'
        WHEN d.effective_edge < 0.10 THEN '5-10%'
        WHEN d.effective_edge < 0.20 THEN '10-20%'
        ELSE '20%+'
    END AS edge_bucket,
    COUNT(*) AS trades,
    ROUND(100.0 * SUM(CASE WHEN t.outcome = 'WIN' THEN 1 ELSE 0 END) / COUNT(*), 1) AS win_pct,
    ROUND(AVG(t.pnl), 2) AS avg_pnl
FROM trades t
JOIN decisions d ON t.decision_id = d.id
GROUP BY edge_bucket
ORDER BY edge_bucket;
```

### 4. Fee Drag Analysis — `Pie Chart`

How much of gross profit is eaten by fees? Slice: window, Value: total_fees.

```sql
SELECT
    m.window,
    ROUND(SUM(t.fee_paid), 2) AS total_fees,
    ROUND(SUM(CASE WHEN t.outcome = 'WIN' THEN t.pnl + t.fee_paid ELSE 0 END), 2) AS gross_profit,
    ROUND(100.0 * SUM(t.fee_paid) / NULLIF(SUM(CASE WHEN t.outcome = 'WIN' THEN t.pnl + t.fee_paid ELSE 0 END), 0), 1) AS fee_pct
FROM trades t
JOIN markets m ON t.market_id = m.market_id
GROUP BY m.window;
```

### 5. Regret Analysis — Skipped Trades — `Table`

How many skipped signals were actually in the right direction?

```sql
SELECT
    d.skip_reason,
    COUNT(*) AS skipped,
    SUM(CASE WHEN m.resolved_side =
        CASE WHEN d.edge > 0 THEN 'YES' ELSE 'NO' END
        THEN 1 ELSE 0 END) AS would_have_won,
    ROUND(AVG(ABS(d.edge)), 3) AS avg_edge
FROM decisions d
JOIN markets m ON d.market_id = m.market_id
WHERE d.action = 'SKIP' AND m.resolved_side IS NOT NULL
GROUP BY d.skip_reason;
```

### 6. Hourly Performance — `Bar Chart`

Which hours (local time) are most profitable? X: hour, Y: net_pnl.

```sql
SELECT
    CAST(strftime('%H', t.resolved_ts / 1000000, 'unixepoch', 'localtime') AS INTEGER) AS hour,
    COUNT(*) AS trades,
    ROUND(SUM(t.pnl), 2) AS net_pnl,
    ROUND(100.0 * SUM(CASE WHEN t.outcome = 'WIN' THEN 1 ELSE 0 END) / COUNT(*), 1) AS win_pct
FROM trades t
GROUP BY hour
ORDER BY hour;
```

### 7. Signal Calibration — `Bar Chart`

Is p_hat well-calibrated? X: predicted_bucket, Y: actual_yes_pct.

```sql
SELECT
    CASE
        WHEN s.p_hat < 0.3 THEN '0-30%'
        WHEN s.p_hat < 0.5 THEN '30-50%'
        WHEN s.p_hat < 0.7 THEN '50-70%'
        ELSE '70-100%'
    END AS predicted_bucket,
    COUNT(*) AS markets,
    ROUND(100.0 * SUM(CASE WHEN m.resolved_side = 'YES' THEN 1 ELSE 0 END) / COUNT(*), 1) AS actual_yes_pct
FROM signals s
JOIN markets m ON s.market_id = m.market_id
WHERE m.resolved_side IS NOT NULL
    AND s.ts = (SELECT MAX(ts) FROM signals s2 WHERE s2.market_id = s.market_id)
GROUP BY predicted_bucket;
```

### 8. Kelly Fraction Effectiveness — `Bar Chart`

Are we sizing positions correctly? X: kelly_bucket, Y: avg_pnl.

```sql
SELECT
    CASE
        WHEN d.kelly_fraction < 0.05 THEN '<5%'
        WHEN d.kelly_fraction < 0.15 THEN '5-15%'
        WHEN d.kelly_fraction < 0.25 THEN '15-25%'
        ELSE '25%+'
    END AS kelly_bucket,
    COUNT(*) AS trades,
    ROUND(SUM(t.pnl), 2) AS net_pnl,
    ROUND(AVG(t.pnl), 2) AS avg_pnl
FROM trades t
JOIN decisions d ON t.decision_id = d.id
GROUP BY kelly_bucket;
```

### 9. Daily Summary — `Table`

```sql
SELECT
    DATE(t.resolved_ts / 1000000, 'unixepoch', 'localtime') AS day,
    COUNT(*) AS trades,
    SUM(CASE WHEN t.outcome = 'WIN' THEN 1 ELSE 0 END) AS wins,
    ROUND(SUM(t.pnl), 2) AS net_pnl,
    ROUND(SUM(t.fee_paid), 2) AS fees_paid,
    ROUND(MIN(t.bankroll_after), 2) AS min_bankroll,
    ROUND(MAX(t.bankroll_after), 2) AS max_bankroll
FROM trades t
GROUP BY day
ORDER BY day;
```

### 10. Max Drawdown — `Metric`

Largest peak-to-trough decline in bankroll. Show max_drawdown_pct as the value.

```sql
WITH running AS (
    SELECT
        resolved_ts,
        bankroll_after,
        MAX(bankroll_after) OVER (ORDER BY resolved_ts) AS peak
    FROM trades
)
SELECT
    ROUND(MIN(bankroll_after - peak), 2) AS max_drawdown,
    ROUND(100.0 * MIN((bankroll_after - peak) / peak), 2) AS max_drawdown_pct
FROM running;
```

### 11. Trade Duration Distribution — `Bar Chart`

Time from entry to resolution. X: window, Y: avg_duration_secs.

```sql
SELECT
    m.window,
    COUNT(*) AS trades,
    ROUND(AVG((t.resolved_ts - t.entry_ts) / 1000000.0), 1) AS avg_duration_secs,
    ROUND(MIN((t.resolved_ts - t.entry_ts) / 1000000.0), 1) AS min_duration_secs,
    ROUND(MAX((t.resolved_ts - t.entry_ts) / 1000000.0), 1) AS max_duration_secs
FROM trades t
JOIN markets m ON t.market_id = m.market_id
GROUP BY m.window;
```

### 12. Market Resolution Tracker — `Table`

All simulated markets with their outcomes.

```sql
SELECT
    market_id,
    asset,
    window,
    ROUND(open_price, 2) AS open_price,
    resolved_side,
    datetime(open_ts / 1000000, 'unixepoch', 'localtime') AS opened_at,
    datetime(resolution_ts / 1000000, 'unixepoch', 'localtime') AS resolved_at
FROM markets
ORDER BY open_ts DESC
LIMIT 50;
```

### 13. Bet Size Distribution — `Pie Chart`

How much are we betting per trade? Slice: size_bucket, Value: trades.

```sql
SELECT
    CASE
        WHEN t.size < 5 THEN '$0-5'
        WHEN t.size < 10 THEN '$5-10'
        WHEN t.size < 20 THEN '$10-20'
        ELSE '$20+'
    END AS size_bucket,
    COUNT(*) AS trades,
    ROUND(AVG(t.pnl), 2) AS avg_pnl,
    ROUND(SUM(t.pnl), 2) AS total_pnl,
    ROUND(100.0 * SUM(CASE WHEN t.outcome = 'WIN' THEN 1 ELSE 0 END) / COUNT(*), 1) AS win_pct
FROM trades t
GROUP BY size_bucket
ORDER BY size_bucket;
```

### 14. Signal Strength Over Time — `Area Chart`

Watch how the bot's confidence evolved during a market window. X: time, Y: p_hat & confidence.

```sql
-- Replace 'BTC_5m_0001' with an actual market_id
SELECT
    datetime(ts / 1000000, 'unixepoch', 'localtime') AS time,
    ROUND(p_hat, 4) AS p_hat,
    ROUND(confidence, 4) AS confidence,
    n_observations
FROM signals
WHERE market_id = 'BTC_5m_0001'
ORDER BY ts;
```

### 15. BTC vs ETH Comparison — `Bar Chart`

Which asset is the bot better at predicting? X: asset, Y: net_pnl.

```sql
SELECT
    m.asset,
    COUNT(*) AS trades,
    SUM(CASE WHEN t.outcome = 'WIN' THEN 1 ELSE 0 END) AS wins,
    ROUND(100.0 * SUM(CASE WHEN t.outcome = 'WIN' THEN 1 ELSE 0 END) / COUNT(*), 1) AS win_pct,
    ROUND(SUM(t.pnl), 2) AS net_pnl,
    ROUND(AVG(t.size), 2) AS avg_bet_size,
    ROUND(SUM(t.fee_paid), 2) AS total_fees
FROM trades t
JOIN markets m ON t.market_id = m.market_id
GROUP BY m.asset;
```

### 16. PnL Sparkline — `Sparkline`

Per-trade PnL as a compact sparkline.

```sql
SELECT ROUND(pnl, 2) AS pnl
FROM trades
ORDER BY resolved_ts;
```

### 17. Win Rate Progress — `Progress`

Visual progress bar of win rate (0-100%).

```sql
SELECT
    ROUND(100.0 * SUM(CASE WHEN outcome = 'WIN' THEN 1 ELSE 0 END) / COUNT(*), 1) AS win_rate_pct
FROM trades;
```

### 18. Exposure Usage — `Progress`

How much of max_total_exposure is currently used.

```sql
SELECT
    ROUND(100.0 * COALESCE(SUM(t_open.size), 0) /
        (SELECT bankroll_after FROM trades ORDER BY resolved_ts DESC LIMIT 1) / 0.50, 1) AS exposure_pct
FROM trades t_open
JOIN markets m ON t_open.market_id = m.market_id
WHERE m.resolved_side IS NULL;
```

### 19. Bankroll Sparkline — `Sparkline`

Bankroll over time as a compact sparkline.

```sql
SELECT ROUND(bankroll_after, 2) AS bankroll
FROM trades
ORDER BY resolved_ts;
```

### 20. Decision Funnel — `Bar Chart`

How many signals become trades? X: stage, Y: count.

```sql
SELECT 'Signals' AS stage, COUNT(DISTINCT market_id) AS cnt FROM signals
UNION ALL
SELECT 'Decisions', COUNT(*) FROM decisions WHERE action = 'TRADE'
UNION ALL
SELECT 'Trades', COUNT(*) FROM trades;
```

---

## Dashboard Panel Summary

| # | Panel | DbPro Type | What it answers |
|---|---|---|---|
| — | Overall Performance | **Metric** (x4) | Am I making or losing money? |
| — | Last 10 Trades | **Table** | What just happened? |
| 1 | Equity curve | **Area Chart** | How is my bankroll trending? |
| 2 | Win rate by market | **Bar Chart** | Which asset/window combo works best? |
| 3 | Edge calibration | **Bar Chart** | Do higher-edge trades actually win more? |
| 4 | Fee drag | **Pie Chart** | How much are fees costing me? |
| 5 | Regret tracker | **Table** | Am I skipping good trades? |
| 6 | Hourly performance | **Bar Chart** | When should I run the bot? |
| 7 | Signal calibration | **Bar Chart** | Is the model accurate? |
| 8 | Position sizing | **Bar Chart** | Is Kelly sizing working? |
| 9 | Daily P&L | **Table** | Day-by-day breakdown |
| 10 | Max drawdown | **Metric** | Worst peak-to-trough decline |
| 11 | Trade duration | **Bar Chart** | How long do positions live? |
| 12 | Market tracker | **Table** | All markets and their outcomes |
| 13 | Bet sizes | **Pie Chart** | Am I betting too much/little? |
| 14 | Signal deep-dive | **Area Chart** | How did confidence evolve in one market? |
| 15 | BTC vs ETH | **Bar Chart** | Which asset am I better at? |
| 16 | PnL sparkline | **Sparkline** | Quick per-trade PnL trend |
| 17 | Win rate progress | **Progress** | Visual win rate gauge |
| 18 | Exposure usage | **Progress** | How much risk budget is used? |
| 19 | Bankroll sparkline | **Sparkline** | Quick bankroll trend |
| 20 | Decision funnel | **Bar Chart** | How many signals become trades? |
