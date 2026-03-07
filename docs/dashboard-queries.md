# Dashboard Query Cookbook

Queries for building dashboards from the paper-trading bot's SQLite database. All timestamps are unix microseconds.

## 1. Equity Curve (Line Chart)

Cumulative P&L over time.

```sql
SELECT
    resolved_ts,
    bankroll_after,
    SUM(pnl) OVER (ORDER BY resolved_ts) AS cumulative_pnl
FROM trades
ORDER BY resolved_ts;
```

## 2. Win Rate by Asset & Window (Bar Chart)

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

## 3. Edge vs. Outcome (Scatter / Grouped Bar)

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

## 4. Fee Drag Analysis (Pie Chart)

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

## 5. Regret Analysis — Skipped Trades (Table)

Trades the bot skipped that would have been profitable.

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

## 6. Hourly Performance Heatmap

Which hours (UTC) are most profitable?

```sql
SELECT
    CAST(strftime('%H', t.resolved_ts / 1000000, 'unixepoch') AS INTEGER) AS hour_utc,
    COUNT(*) AS trades,
    ROUND(SUM(t.pnl), 2) AS net_pnl,
    ROUND(100.0 * SUM(CASE WHEN t.outcome = 'WIN' THEN 1 ELSE 0 END) / COUNT(*), 1) AS win_pct
FROM trades t
GROUP BY hour_utc
ORDER BY hour_utc;
```

## 7. Signal Calibration Plot

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

## 8. Kelly Fraction Effectiveness (Bar Chart)

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

## 9. Daily Summary (Table)

```sql
SELECT
    DATE(t.resolved_ts / 1000000, 'unixepoch') AS day,
    COUNT(*) AS trades,
    SUM(CASE WHEN t.outcome = 'WIN' THEN 1 ELSE 0 END) AS wins,
    ROUND(SUM(t.pnl), 2) AS net_pnl,
    ROUND(SUM(t.fee_paid), 2) AS fees_paid,
    ROUND(MIN(t.bankroll_after), 2) AS min_bankroll,
    MAX(t.bankroll_after) AS end_bankroll
FROM trades t
GROUP BY day
ORDER BY day;
```

## 10. Max Drawdown

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

## 11. Trade Duration Distribution

Time from entry to resolution.

```sql
SELECT
    m.window,
    ROUND(AVG((t.resolved_ts - t.entry_ts) / 1000000.0), 1) AS avg_duration_secs,
    ROUND(MIN((t.resolved_ts - t.entry_ts) / 1000000.0), 1) AS min_duration_secs,
    ROUND(MAX((t.resolved_ts - t.entry_ts) / 1000000.0), 1) AS max_duration_secs
FROM trades t
JOIN markets m ON t.market_id = m.market_id
GROUP BY m.window;
```

## Dashboard Panel Summary

| Panel | Chart Type | Query |
|---|---|---|
| Equity curve | Line | #1 |
| Win rate by market | Grouped bar | #2 |
| Edge calibration | Grouped bar | #3 |
| Fee drag | Pie | #4 |
| Regret tracker | Table | #5 |
| Time-of-day heatmap | Heatmap | #6 |
| Signal calibration | Scatter | #7 |
| Position sizing | Bar | #8 |
| Daily P&L | Table | #9 |
| Max drawdown | KPI card | #10 |
| Trade duration | Histogram | #11 |
