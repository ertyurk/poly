# Future Ideas — Evaluated but Not Yet Implemented

**Date:** 2026-03-12
**Status:** Backlog — requires paper testing before any live deployment

---

## 1. Copy Trading Module

**Source:** Branch `claude/review-trading-autoresearch-BIxhf`

### Concept
Follow proven Polymarket wallets instead of generating our own signals.
Poll the Data API for trades by top wallets, mirror entries proportionally.

### Why It's Interesting
- Addresses the execution gap differently — ride the leader's proven fill ability
- Top wallet: `0x1979ae6B7E6534dE9c4539D0c205E582cA637C9D` — +$1.07M PnL,
  24,600+ crypto trades, ~60% win rate
- Doesn't require our signal model at all

### Key Risks
- **Latency drag**: 60% win rate with 2% fees = razor-thin margin. If copy
  latency costs 2-3% of win rate, we're breakeven or negative.
- **Adverse selection**: by the time we copy, the edge may be gone
- **Leader drawdown**: single wallet can have bad months

### Architecture (from branch plan)
- New `CopyTraderActor` — polls `GET /trades?maker={wallet}` every 5-10s
- Dedup by trade ID, freshness gate (skip if >60s old)
- Proportional sizing: `(our_bankroll / leader_est_bankroll) * leader_size`
- Mirror exits when leader closes positions
- Config: `[copy_trading]` section with wallet list, poll interval, consensus filter

### Prerequisites
- Paper test for 1-2 weeks tracking the top wallet
- Measure copy latency and win rate degradation
- Build as a separate mode (`--copy` flag), not replacing momentum bot

### Full Design
See `docs/plans/2026-03-11-copy-trading-plan.md` on branch
`origin/claude/review-trading-autoresearch-BIxhf`

---

## 2. Weather Market Support

**Source:** Branch `claude/review-trading-bot-post-NfPDY`

### Concept
Trade Polymarket temperature prediction markets using NWS (National Weather
Service) observation data as the signal source.

### Why It's Interesting
- **Structural edge**: NWS data IS the resolution source for these markets.
  If the market misprices relative to the latest observation, the edge is real.
- **No execution speed race**: NWS updates every 15-60 min (not milliseconds)
- **Diversification**: uncorrelated with crypto momentum strategy

### Key Risks
- **Market efficiency**: other bots likely already use NWS data
- **Low volume**: weather markets may be too thin for meaningful position sizes
- **Untested model**: normal CDF with sqrt(time) uncertainty scaling is
  plausible but unvalidated

### Architecture (from branch)
- New `WeatherActor` — polls NWS stations for temperature observations
- New `MarketType` variants: `TempAbove(strike)`, `TempBelow(strike)`,
  `TempBetween(lo, hi)`
- Normal CDF probability model: `σ = base_uncertainty * sqrt(hours_out / 6)`
- Market discovery via keyword matching on question text ("temperature", "°F")
- Config: `[weather]` section with station mappings and uncertainty parameter

### Prerequisites
- Paper test for 1 week on NYC/Chicago weather markets
- Measure actual edge: are markets mispriced enough after fees?
- Validate fill rates on weather market order books

---

## 3. Early Exit on Edge Reversal

**Source:** Branch `claude/review-trading-bot-post-NfPDY` (feature proposal)

### Concept
Monitor open positions and sell back via CLOB when the edge reverses beyond
a threshold, rather than holding until resolution.

### Why It's Interesting
- Cuts average loss per trade by ~30-50% in backtests
- Currently we hold losing positions until they resolve at $0

### Key Risks
- **Whipsaw**: exit on a temporary reversal, miss the recovery
- **Execution complexity**: selling back requires CLOB sell orders (new path)
- **Fee drag**: entry + exit fees on a losing trade = double the cost

### Prerequisites
- Analyze historical trades: what % of losses showed early warning signals?
- Paper test: does exiting at -5% edge improve overall P&L?

---

## 4. Realized Volatility from Tick Buffer

**Source:** Branch `claude/review-trading-bot-post-NfPDY` (feature proposal)

### Concept
Compute realized vol directly from recent ticks in a ring buffer, blend with
EWMA estimate for faster adaptation to regime changes.

### Why It's Interesting
- EWMA vol lags during sudden regime changes (liquidation cascades)
- Blended vol (60% realized + 40% EWMA) would be more responsive
- Better vol = better p_hat = better signal quality

### Prerequisites
- Compare realized vs EWMA vol on historical tick data
- Measure impact on p_hat accuracy

---

## Priority Order

1. **Fill rate improvement** (in progress) — highest ROI, proven signal just needs execution
2. **Weather markets** — structural edge, uncorrelated, moderate implementation effort
3. **Copy trading** — interesting but latency risk needs careful paper testing
4. **Early exit** — nice-to-have, depends on loss analysis
5. **Realized vol** — incremental improvement, low priority
