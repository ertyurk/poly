# Dashboard Query Cookbook

Queries for building dashboards from the paper-trading bot's SQLite database.
All timestamps are unix microseconds. Connect to `data/bot.db` with any SQLite tool.

## Quick Start — Run These First

### Overall Performance (run this first)

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

### Last 10 Trades

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

### 1. Equity Curve (Line Chart)

Cumulative P&L over time.

```sql
SELECT
    datetime(resolved_ts / 1000000, 'unixepoch', 'localtime') AS time,
    bankroll_after,
    SUM(pnl) OVER (ORDER BY resolved_ts) AS cumulative_pnl
FROM trades
ORDER BY resolved_ts;
```

### 2. Win Rate by Asset & Window (Bar Chart)

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

### 3. Edge vs. Outcome (Scatter / Grouped Bar)

Did higher-edge trades actually win more often?

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

### 4. Fee Drag Analysis (Pie Chart)

How much of gross profit is eaten by fees?

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

### 5. Regret Analysis — Skipped Trades (Table)

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

### 6. Hourly Performance Heatmap

Which hours (local time) are most profitable?

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

### 7. Signal Calibration Plot

Is p_hat well-calibrated? (predicted probability vs. actual outcome rate)

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

### 8. Kelly Fraction Effectiveness (Bar Chart)

Are we sizing positions correctly?

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

### 9. Daily Summary (Table)

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

### 10. Max Drawdown

Largest peak-to-trough decline in bankroll.

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

### 11. Trade Duration Distribution

Time from entry to resolution.

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

### 12. Market Resolution Tracker

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

### 13. Bet Size Distribution

How much are we betting per trade?

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

### 14. Signal Strength Over Time (for a specific market)

Watch how the bot's confidence evolved during a market window.

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

### 15. BTC vs ETH Comparison

Which asset is the bot better at predicting?

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

---

## Dashboard Panel Summary

| # | Panel | Chart Type | What it answers |
|---|---|---|---|
| — | Overall Performance | KPI cards | Am I making or losing money? |
| — | Last 10 Trades | Table | What just happened? |
| 1 | Equity curve | Line | How is my bankroll trending? |
| 2 | Win rate by market | Grouped bar | Which asset/window combo works best? |
| 3 | Edge calibration | Grouped bar | Do higher-edge trades actually win more? |
| 4 | Fee drag | Pie | How much are fees costing me? |
| 5 | Regret tracker | Table | Am I skipping good trades? |
| 6 | Time-of-day heatmap | Heatmap | When should I run the bot? |
| 7 | Signal calibration | Scatter | Is the Bayesian model accurate? |
| 8 | Position sizing | Bar | Is Kelly sizing working? |
| 9 | Daily P&L | Table | Day-by-day breakdown |
| 10 | Max drawdown | KPI card | Worst losing streak |
| 11 | Trade duration | Histogram | How long do positions live? |
| 12 | Market tracker | Table | All markets and their outcomes |
| 13 | Bet sizes | Bar | Am I betting too much/little? |
| 14 | Signal deep-dive | Line | How did confidence evolve in one market? |
| 15 | BTC vs ETH | Grouped bar | Which asset am I better at? |
