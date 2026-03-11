use crate::types::TsMicros;

/// Rolling window durations in microseconds.
const WINDOW_10S: i64 = 10_000_000;
const WINDOW_30S: i64 = 30_000_000;

/// EWM lambda for baseline volume (half-life ~5 minutes).
const VOL_BASELINE_LAMBDA: f64 = 0.00230;

/// Minimum total volume to compute meaningful OFI (avoid div-by-zero).
const MIN_VOL_FOR_OFI: f64 = 0.001;

/// Large trade threshold: trade is "large" if qty > this multiple of EWM average size.
const LARGE_TRADE_MULTIPLIER: f64 = 5.0;

/// Snapshot of flow state at a point in time.
#[derive(Debug, Clone, Copy)]
pub struct FlowSnapshot {
    /// Order Flow Imbalance over 10s window: [-1.0, +1.0].
    /// Positive = net buy pressure, negative = net sell pressure.
    pub ofi_10s: f64,
    /// Volume ratio: recent 30s volume / EWM baseline. >2.0 = "hot" regime.
    pub vol_ratio: f64,
    /// True if the most recent trade was unusually large.
    pub large_trade: bool,
}

/// Single recorded trade in the ring buffer.
#[derive(Debug, Clone, Copy)]
struct TickRecord {
    ts: TsMicros,
    qty: f64,
    /// +qty for aggressive buy, -qty for aggressive sell.
    signed_qty: f64,
}

/// Per-asset flow tracker using a ring buffer for rolling windows.
///
/// Performance: O(1) amortized per update. The ring buffer is fixed-size
/// and old entries are lazily evicted during snapshot computation.
pub struct FlowTracker {
    /// Ring buffer of recent trades. Fixed capacity, overwrites oldest.
    buf: Vec<TickRecord>,
    /// Write position in ring buffer.
    write_pos: usize,
    /// Number of valid entries (up to buf capacity).
    count: usize,
    /// EWM baseline volume per second (smoothed).
    baseline_vol_per_sec: f64,
    /// EWM average trade size (for large trade detection).
    avg_trade_size: f64,
    /// Last update timestamp.
    last_ts: TsMicros,
    /// Whether the most recent trade was flagged as large.
    last_large: bool,
}

impl FlowTracker {
    /// Ring buffer capacity. 2048 trades ≈ 30-60 seconds of BTC at typical rates.
    const CAPACITY: usize = 2048;

    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            write_pos: 0,
            count: 0,
            baseline_vol_per_sec: 0.0,
            avg_trade_size: 0.0,
            last_ts: 0,
            last_large: false,
        }
    }

    /// Record a new trade.
    /// `qty`: trade quantity in base asset units.
    /// `buyer_is_maker`: if true, the sell side was aggressor (= sell pressure).
    pub fn update(&mut self, qty: f64, buyer_is_maker: bool, ts: TsMicros) {
        let signed = if buyer_is_maker { -qty } else { qty };

        // Large trade detection: compare to EWM average
        if self.avg_trade_size > 0.0 {
            self.last_large = qty > self.avg_trade_size * LARGE_TRADE_MULTIPLIER;
        }

        // Update EWM average trade size
        if self.last_ts > 0 {
            let dt = ((ts - self.last_ts) as f64) / 1_000_000.0;
            if dt > 0.0 && dt < 60.0 {
                let alpha = (1.0 - (-VOL_BASELINE_LAMBDA * dt).exp()).clamp(0.001, 0.5);
                self.avg_trade_size = (1.0 - alpha) * self.avg_trade_size + alpha * qty;
                self.baseline_vol_per_sec =
                    (1.0 - alpha) * self.baseline_vol_per_sec + alpha * (qty / dt);
            }
        } else {
            self.avg_trade_size = qty;
            self.baseline_vol_per_sec = qty;
        }

        // Write to ring buffer
        let record = TickRecord { ts, qty, signed_qty: signed };
        if self.buf.len() < Self::CAPACITY {
            self.buf.push(record);
            self.write_pos = self.buf.len();
        } else {
            let pos = self.write_pos % Self::CAPACITY;
            self.buf[pos] = record;
            self.write_pos = pos + 1;
        }
        self.count = self.count.saturating_add(1).min(Self::CAPACITY);
        self.last_ts = ts;
    }

    /// Compute a snapshot of current flow state.
    pub fn snapshot(&self, now: TsMicros) -> FlowSnapshot {
        let cutoff_10s = now - WINDOW_10S;
        let cutoff_30s = now - WINDOW_30S;

        let mut buy_10 = 0.0f64;
        let mut sell_10 = 0.0f64;
        let mut total_30 = 0.0f64;

        for i in 0..self.count {
            let idx = if self.buf.len() < Self::CAPACITY {
                i
            } else {
                (self.write_pos + i) % Self::CAPACITY
            };
            let rec = &self.buf[idx];

            if rec.ts >= cutoff_30s {
                total_30 += rec.qty;

                if rec.ts >= cutoff_10s {
                    if rec.signed_qty > 0.0 {
                        buy_10 += rec.qty;
                    } else {
                        sell_10 += rec.qty;
                    }
                }
            }
        }

        let total_10 = buy_10 + sell_10;
        let ofi_10s = if total_10 > MIN_VOL_FOR_OFI {
            (buy_10 - sell_10) / total_10
        } else {
            0.0
        };
        let vol_30s_per_sec = total_30 / (WINDOW_30S as f64 / 1_000_000.0);
        let vol_ratio = if self.baseline_vol_per_sec > MIN_VOL_FOR_OFI {
            vol_30s_per_sec / self.baseline_vol_per_sec
        } else {
            1.0
        };

        FlowSnapshot {
            ofi_10s,
            vol_ratio,
            large_trade: self.last_large,
        }
    }
}
