# Fill Rate Improvement — GTD→FOK Execution Design

**Date:** 2026-03-12
**Status:** Approved
**Goal:** Raise live fill rate from 18% to ~70-80%

---

## Problem

Live trading (March 2026): 82% of signals never execute.
- FOK orders require full fill at exact price — fails on thin books ($2-10K depth)
- Paper model assumes $50K depth → unrealistic 100% fill rate
- Signal quality is proven (7/7 paper, 100% win rate v8) — execution is the bottleneck

## Solution: GTD→FOK Two-Phase Execution

### Phase 1: GTD Maker Order (0-15s)

Post a **GTD limit order** at our target price with **15-second auto-expiry**.

- Order rests on the book as a maker order
- **0% maker fee** (vs 2% taker fee on FOK)
- If a counterparty crosses our price within 15s → fill at maker rate
- If not filled → order auto-expires (no orphaned orders, ever)

### Phase 2: FOK Taker Fallback (15s+)

If GTD expires unfilled AND the signal is still valid:

- Re-check the current order book
- Send **FOK at best_ask + 1 cent** (YES) or **best_bid - 1 cent** (NO)
- This crosses the spread aggressively to guarantee fill
- 2% taker fee, but the edge still covers it
- If the book has moved too far (>10% slippage from original signal), skip entirely

### Order Lifecycle

```
Signal arrives
    │
    ▼
┌─────────────────────────────┐
│ Place GTD order              │
│ price = best_ask (YES)       │
│       = 1-best_bid (NO)      │
│ expiry = now + 15s           │
│ Record order_id, market_id   │
└──────────┬──────────────────┘
           │
    ┌──────┴──────┐
    │ Poll status  │ (every 3s)
    │ via order_id │
    └──────┬──────┘
           │
    ┌──────┴──────────────────┐
    │                         │
    ▼                         ▼
MATCHED                   EXPIRED/LIVE after 15s
  │                           │
  ▼                           ▼
Record position         Check signal freshness
                              │
                    ┌─────────┴─────────┐
                    │                   │
                    ▼                   ▼
              Signal still         Signal stale
              valid (< 20s)        (> 20s old)
                    │                   │
                    ▼                   ▼
              Re-check book         Skip (log)
                    │
                    ▼
              FOK at aggressive
              price (+1¢)
                    │
              ┌─────┴─────┐
              │           │
              ▼           ▼
          MATCHED      REJECTED
              │           │
              ▼           ▼
          Record pos    Log rejection
```

## Safety Layers (5 deep)

### 1. GTD Auto-Expiry (primary)
- Every order has a hard expiration timestamp baked into the signed order
- Even if our process crashes, the CLOB will expire the order
- No orphaned orders, ever — this is enforced by Polymarket's matching engine

### 2. Single Order Per Market
- The existing `pending` HashSet in DecisionActor prevents duplicate signals
- The executor checks `positions` for duplicate entries before placing
- New: `active_orders` HashMap tracks in-flight GTD orders — no second order
  while one is resting

### 3. Cancel-All on Shutdown
- On Ctrl+C / SIGTERM: call `cancel_all_orders()` before process exit
- On panic: tokio runtime drop triggers shutdown signal → cancel-all fires
- Belt-and-suspenders with GTD expiry (even if cancel-all fails, orders expire)

### 4. Market Resolution Guard
- Don't place orders on markets resolving within 60 seconds
- Prevents placing orders that could fill on a market that's about to settle
  with an unknown outcome

### 5. Stale Signal Rejection
- Signal has a `ts` field — reject if signal is >20 seconds old by FOK phase
- Prevents acting on outdated information after the GTD wait period
- Re-validate the order book before FOK: reject if slippage > 10%

## Changes Required

### `src/polymarket/live_trader.rs`
- Add `place_gtd_order(token_id, side, price, size, expiry_secs)` method
  - Uses `OrderType::GTD` with `expiration = Utc::now() + Duration::seconds(expiry_secs)`
- Add `check_order_status(order_id)` method (poll for MATCHED/LIVE/CANCELED)
- Add `cancel_order(order_id)` method
- Add `cancel_all_orders()` method

### `src/actors/executor.rs`
- New `ActiveOrder` struct: `{ order_id, market_id, side, price, size, placed_at, signal_ts }`
- New `active_orders: HashMap<String, ActiveOrder>` (market_id → active order)
- New order lifecycle loop:
  1. On TradeDecision: place GTD, store in active_orders
  2. Every 3s: poll active order statuses
  3. On MATCHED: remove from active_orders, create OpenPosition
  4. On expired (15s elapsed): remove, attempt FOK fallback if signal fresh
  5. On FOK MATCHED: create OpenPosition
  6. On FOK rejected: log, done
- Shutdown: `cancel_all_orders()` before breaking the loop

### `src/actors/decision.rs`
- No changes to decision logic — it still emits TradeDecision as before
- The executor handles the GTD→FOK lifecycle independently

### `src/main.rs`
- Pass `LiveTrader` reference to executor (already done)
- Add shutdown hook for cancel-all

### `config.toml`
```toml
[execution]
# GTD order expiry in seconds (Phase 1 duration)
gtd_expiry_secs = 15
# Maximum signal age for FOK fallback (seconds)
max_signal_age_secs = 20
# Price aggression for FOK fallback (cents above ask / below bid)
fok_price_bump_cents = 0.01
# Minimum seconds before market resolution to place an order
min_time_before_resolution_secs = 60
```

## Expected Impact

| Metric | Current (FOK-only) | Projected (GTD→FOK) |
|--------|-------------------|---------------------|
| Fill rate | 18% | 70-80% |
| Avg fee per fill | 2% (taker) | ~1% (blended maker+taker) |
| Signals executed per hour | ~1-2 | ~5-8 |
| Expected profit per signal | ~$1.08 | ~$3.72 |

## Paper Mode

Paper mode is unaffected — it continues to simulate instant fills. The GTD→FOK
lifecycle only runs in Live mode. This means paper results remain a ceiling
estimate (not a floor), which is the conservative assumption.
