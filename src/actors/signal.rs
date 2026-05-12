use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::flow::FlowTracker;
use crate::types::*;

/// Minimum fraction of window elapsed before emitting signals.
/// At 20% elapsed: allows early momentum entries which win 71% vs 22%
/// for mid-window entries. Early entry = more time for displacement to
/// persist and less chance of mean-reversion killing the position.
const MIN_ELAPSED_PCT: f64 = 0.20;

/// Initial per-second volatility estimate (~2.5% daily for BTC).
const INITIAL_VOL: f64 = 0.00008;

/// Minimum volatility floor — prevents overconfidence during quiet periods.
/// 0.00005 per-second ≈ 33% annualized. Below this, vol estimate is unreliable.
const MIN_VOL: f64 = 0.00005;

/// Minimum valid updates before emitting signals (~30 seconds of data).
const MIN_TICKS: u32 = 30;

/// Cold-start warmup: suppress signals for this many seconds after the first
/// tick. Prevents trading with unreliable drift estimates (fast drift half-life
/// is ~5 min, so 5 min warmup gives at least one full half-life of data).
const WARMUP_SECS: i64 = 300;

/// Safety margin on realized volatility.
/// Set to 1.0 because tail risk is now handled by the Student-t CDF
/// (fat_tail_cdf) instead of crude vol inflation. The old value of 1.3
/// compressed p_hat toward 0.5, causing systematic NO-bias in trending markets.
const VOL_SAFETY_MARGIN: f64 = 1.0;

/// Slow variance lambda divisor: slow_vol_lambda = lambda / SLOW_VOL_FACTOR.
/// Factor of 6 → slow vol half-life is 6× longer than fast vol (~30 min).
const SLOW_VOL_FACTOR: f64 = 6.0;

/// Slow drift lambda divisor: slow_lambda = spot_lambda / SLOW_DRIFT_FACTOR.
/// Factor of 2 → slow drift half-life is 2× longer than fast drift.
/// Fast (spot_lambda=0.00230): half-life ≈ 5 min.
/// Slow (spot_lambda/2):       half-life ≈ 10 min.
/// Requires both timescales to agree before trading — prevents
/// short-term bounces from triggering bets against the prevailing trend.
const SLOW_DRIFT_FACTOR: f64 = 2.0;

// ---------------------------------------------------------------------------
// Fat-tailed probability model (Student-t adjusted log-normal)
// ---------------------------------------------------------------------------

/// Degrees of freedom for Student-t tail adjustment.
/// Lower ν = fatter tails = more conservative at extremes.
/// ν=4 matches empirical crypto return distributions well:
/// - Moderate z-scores (|z| < 2): behaves like normal CDF
/// - Extreme z-scores (|z| > 3): stays further from 0/1, accounting
///   for liquidation cascades and flash crashes
const TAIL_NU: f64 = 4.0;

/// Student-t adjusted CDF: accounts for fat tails in crypto returns.
///
/// Compresses extreme z-scores via `x / sqrt(1 + x²/ν)` before applying
/// the logistic approximation. For moderate x this is nearly identical to
/// the normal CDF; for extreme x it prevents overconfident probabilities
/// near 0.0 or 1.0. This replaces the old VOL_SAFETY_MARGIN=1.3 approach
/// with a principled treatment of tail risk.
#[inline]
fn fat_tail_cdf(x: f64) -> f64 {
    let adjusted = x / (1.0 + x * x / TAIL_NU).sqrt();
    1.0 / (1.0 + (-1.7 * adjusted).exp())
}

/// Compute p_hat using the log-normal model (for strike-based markets).
fn compute_p_hat_lognormal(
    current_price: f64,
    market_type: MarketType,
    open_spot_price: f64,
    vol_per_sec: f64,
    drift_per_sec: f64,
    time_to_expiry_secs: f64,
) -> f64 {
    if time_to_expiry_secs <= 0.0 {
        return 0.5;
    }

    let sigma_t = vol_per_sec * time_to_expiry_secs.sqrt();
    if sigma_t < 1e-12 {
        return 0.5;
    }

    let drift_t =
        (drift_per_sec - 0.5 * vol_per_sec * vol_per_sec) * time_to_expiry_secs;

    match market_type {
        MarketType::Above(strike) => {
            if strike <= 0.0 {
                return 1.0;
            }
            let d = ((current_price / strike).ln() + drift_t) / sigma_t;
            fat_tail_cdf(d)
        }
        MarketType::Below(strike) => {
            if strike <= 0.0 {
                return 0.0;
            }
            let d = ((current_price / strike).ln() + drift_t) / sigma_t;
            1.0 - fat_tail_cdf(d)
        }
        MarketType::Between(lo, hi) => {
            if lo <= 0.0 || hi <= 0.0 {
                return 0.5;
            }
            let d_lo = ((current_price / lo).ln() + drift_t) / sigma_t;
            let d_hi = ((current_price / hi).ln() + drift_t) / sigma_t;
            (fat_tail_cdf(d_lo) - fat_tail_cdf(d_hi)).clamp(0.0, 1.0)
        }
        MarketType::UpDown => {
            // UpDown = "price went up from open" = Above(open_spot_price)
            if open_spot_price <= 0.0 {
                return 0.5;
            }
            let d = ((current_price / open_spot_price).ln() + drift_t) / sigma_t;
            fat_tail_cdf(d)
        }
    }
}

// ---------------------------------------------------------------------------
// AssetTracker — estimates realized vol and drift from tick stream
// ---------------------------------------------------------------------------

pub struct AssetTracker {
    last_price: f64,
    last_ts: TsMicros,
    /// Timestamp of the first tick — used for cold-start warmup.
    first_ts: TsMicros,
    /// Count of valid updates (spaced ≥ 0.1s apart) — NOT raw ticks.
    valid_ticks: u32,
    /// Exponentially-weighted per-second variance estimate.
    variance: f64,
    /// Slow variance EWM (~30 min half-life) for vol regime detection.
    slow_variance: f64,
    /// Fast drift: EWM per-second drift (half-life ≈ 5 min).
    drift: f64,
    /// Slow drift: EWM per-second drift (half-life ≈ 20 min).
    /// Used as trend confirmation — both must agree before trading.
    slow_drift: f64,
    lambda: f64,
    slow_lambda: f64,
    /// Circular buffer of recent log returns for variance ratio test.
    return_buf: std::collections::VecDeque<f64>,
}

impl AssetTracker {
    pub fn new(lambda: f64) -> Self {
        let slow_lambda = lambda / SLOW_DRIFT_FACTOR;
        Self {
            last_price: 0.0,
            last_ts: 0,
            first_ts: 0,
            valid_ticks: 0,
            variance: INITIAL_VOL * INITIAL_VOL,
            slow_variance: INITIAL_VOL * INITIAL_VOL,
            drift: 0.0,
            slow_drift: 0.0,
            lambda,
            slow_lambda,
            return_buf: std::collections::VecDeque::with_capacity(64),
        }
    }

    /// Update with a new price tick. Returns true if enough data for signals.
    /// Requires both MIN_TICKS valid updates AND WARMUP_SECS elapsed since
    /// first tick to prevent trading with unreliable drift estimates.
    pub fn update(&mut self, price: f64, ts: TsMicros) -> bool {
        if self.valid_ticks == 0 {
            self.last_price = price;
            self.last_ts = ts;
            self.first_ts = ts;
            self.valid_ticks = 1;
            return false;
        }

        let dt_secs = ((ts - self.last_ts) as f64) / 1_000_000.0;

        // Only count updates with meaningful time gaps (≥ 0.1s, < 60s)
        if dt_secs >= 0.1 && dt_secs < 60.0 {
            let log_ret = (price / self.last_price).ln();
            let var_sample = log_ret * log_ret / dt_secs;
            let ret_per_sec = log_ret / dt_secs;

            // Fast EWM update (reactive to recent moves)
            let alpha = (1.0 - (-self.lambda * dt_secs).exp()).clamp(0.001, 0.5);
            self.variance = (1.0 - alpha) * self.variance + alpha * var_sample;
            self.drift = (1.0 - alpha) * self.drift + alpha * ret_per_sec;

            // Slow EWM update (captures prevailing trend)
            let slow_alpha = (1.0 - (-self.slow_lambda * dt_secs).exp()).clamp(0.001, 0.5);
            self.slow_drift = (1.0 - slow_alpha) * self.slow_drift + slow_alpha * ret_per_sec;

            // Slow variance EWM (~30 min half-life)
            let slow_vol_lambda = self.lambda / SLOW_VOL_FACTOR;
            let slow_vol_alpha = (1.0 - (-slow_vol_lambda * dt_secs).exp()).clamp(0.001, 0.5);
            self.slow_variance =
                (1.0 - slow_vol_alpha) * self.slow_variance + slow_vol_alpha * var_sample;

            // Store log return for variance ratio test
            self.return_buf.push_back(log_ret);
            if self.return_buf.len() > 64 {
                self.return_buf.pop_front();
            }

            self.last_price = price;
            self.last_ts = ts;
            self.valid_ticks += 1;
        }

        let warmup_elapsed =
            (ts - self.first_ts) >= WARMUP_SECS * 1_000_000;
        self.valid_ticks >= MIN_TICKS && warmup_elapsed
    }

    fn vol_per_sec(&self) -> f64 {
        self.variance.sqrt().max(MIN_VOL)
    }

    fn slow_vol_per_sec(&self) -> f64 {
        self.slow_variance.sqrt().max(MIN_VOL)
    }

    fn drift_per_sec(&self) -> f64 {
        self.drift
    }

    fn slow_drift_per_sec(&self) -> f64 {
        self.slow_drift
    }

    fn vol_regime(&self) -> f64 {
        let fast = self.variance.sqrt().max(MIN_VOL);
        let slow = self.slow_variance.sqrt().max(MIN_VOL);
        fast / slow
    }

    /// Variance ratio VR(q): Var(q-period) / (q * Var(1-period)).
    /// VR < 1.0 = mean-reverting, VR > 1.0 = trending.
    fn variance_ratio(&self, q: usize) -> Option<f64> {
        let n = self.return_buf.len();
        if n < q * 2 {
            return None;
        }
        let buf: &std::collections::VecDeque<f64> = &self.return_buf;
        // Var of 1-period returns
        let mean: f64 = buf.iter().sum::<f64>() / n as f64;
        let var1: f64 =
            buf.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        if var1 < 1e-20 {
            return None;
        }
        // Var of q-period returns (sum of q consecutive 1-period returns)
        let num_q = n - q + 1;
        let mut q_returns = Vec::with_capacity(num_q);
        for i in 0..num_q {
            let s: f64 = (0..q).map(|j| buf[i + j]).sum();
            q_returns.push(s);
        }
        let q_mean: f64 = q_returns.iter().sum::<f64>() / num_q as f64;
        let var_q: f64 = q_returns
            .iter()
            .map(|r| (r - q_mean).powi(2))
            .sum::<f64>()
            / (num_q - 1).max(1) as f64;
        Some(var_q / (q as f64 * var1))
    }

    fn current_price(&self) -> f64 {
        self.last_price
    }

    /// Restore vol/tick state from a previous session.
    /// Drift is reset to zero and warmup re-applies from the restore timestamp,
    /// since stale drift needs time to rebuild from live ticks.
    pub fn restore(
        last_price: f64,
        last_ts: TsMicros,
        valid_ticks: u32,
        variance: f64,
        _drift: f64,
        _slow_drift: f64,
        lambda: f64,
        slow_variance: f64,
    ) -> Self {
        let slow_lambda = lambda / SLOW_DRIFT_FACTOR;
        Self {
            last_price,
            last_ts,
            // Restored trackers still need warmup — drift is zeroed on
            // restore and needs time to rebuild from live ticks.
            first_ts: last_ts,
            valid_ticks,
            variance,
            slow_variance,
            drift: 0.0,
            slow_drift: 0.0,
            lambda,
            slow_lambda,
            return_buf: std::collections::VecDeque::with_capacity(64),
        }
    }

    /// Accessors for warm-up persistence (keep fields private).
    pub fn state_for_persist(&self) -> (f64, TsMicros, u32, f64, f64, f64, f64, f64) {
        (
            self.last_price,
            self.last_ts,
            self.valid_ticks,
            self.variance,
            self.drift,
            self.slow_drift,
            self.lambda,
            self.slow_variance,
        )
    }
}

// ---------------------------------------------------------------------------
// Market metadata
// ---------------------------------------------------------------------------

struct MarketMeta {
    asset: Asset,
    market_type: MarketType,
    resolution_ts: TsMicros,
    open_ts: TsMicros,
}

// ---------------------------------------------------------------------------
// SignalActor
// ---------------------------------------------------------------------------

pub struct SignalActor {
    config: Config,
    initial_trackers: HashMap<Asset, AssetTracker>,
}

impl SignalActor {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            initial_trackers: HashMap::new(),
        }
    }

    /// Pre-load warm-up state from a previous session.
    pub fn with_warm_state(mut self, trackers: HashMap<Asset, AssetTracker>) -> Self {
        self.initial_trackers = trackers;
        self
    }

    pub async fn run(
        mut self,
        mut spot_rx: mpsc::Receiver<SpotTick>,
        mut market_rx: mpsc::Receiver<MarketState>,
        signal_tx: mpsc::Sender<Signal>,
        db_tx: mpsc::Sender<DbEvent>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let lambda = self.config.strategy.decay.spot_lambda;

        // Per-asset volatility & drift tracking — drain warm-up state if available
        let mut asset_trackers: HashMap<Asset, AssetTracker> =
            self.initial_trackers.drain().collect();
        for (asset, tracker) in &asset_trackers {
            let vol = tracker.vol_per_sec();
            tracing::debug!(
                asset = %asset,
                valid_ticks = tracker.state_for_persist().2,
                vol = vol,
                "restored signal state from previous session"
            );
        }
        // Per-asset flow tracking (order flow imbalance, volume regime)
        let mut flow_trackers: HashMap<Asset, FlowTracker> = HashMap::new();

        // Per-market metadata
        let mut market_meta: HashMap<String, MarketMeta> = HashMap::new();
        // Per-market spot open prices (first spot price seen after market discovery)
        let mut open_spot_prices: HashMap<String, f64> = HashMap::new();
        // Throttle: max 1 signal per second per market
        let mut last_signal_ts: HashMap<String, TsMicros> = HashMap::new();

        loop {
            tokio::select! {
                biased;

                _ = shutdown.changed() => {
                    tracing::debug!("signal actor shutting down — persisting state");
                    for (asset, tracker) in &asset_trackers {
                        let (last_price, last_ts, valid_ticks, variance, drift, slow_drift, lam, slow_var) =
                            tracker.state_for_persist();
                        if valid_ticks > 0 {
                            let _ = db_tx.try_send(DbEvent::SaveSignalState {
                                asset: asset.to_string(),
                                last_price,
                                last_ts,
                                valid_ticks,
                                variance,
                                drift,
                                slow_drift,
                                lambda: lam,
                                slow_variance: slow_var,
                            });
                        }
                    }
                    return;
                }

                Some(market) = market_rx.recv() => {
                    let market_id = market.market_id.clone();
                    market_meta.insert(market_id, MarketMeta {
                        asset: market.asset,
                        market_type: market.market_type,
                        resolution_ts: market.resolution_ts,
                        open_ts: market.open_ts,
                    });
                }

                Some(tick) = spot_rx.recv() => {
                    // Update per-asset tracker (vol/drift estimation)
                    let tracker = asset_trackers
                        .entry(tick.asset)
                        .or_insert_with(|| AssetTracker::new(lambda));

                    let ready = tracker.update(tick.price, tick.ts);
                    if !ready {
                        continue;
                    }

                    // Update per-asset flow tracker (OFI, volume regime)
                    let flow = flow_trackers
                        .entry(tick.asset)
                        .or_insert_with(FlowTracker::new);
                    flow.update(tick.qty, tick.buyer_is_maker, tick.ts);

                    // Use max(fast, slow) vol to prevent overconfidence during
                    // quiet periods. Fast vol (~5 min half-life) drops quickly
                    // during lulls; slow vol (~30 min half-life) is more stable.
                    let vol = tracker.vol_per_sec()
                        .max(tracker.slow_vol_per_sec())
                        * VOL_SAFETY_MARGIN;

                    // Extract values from tracker before releasing mutable borrow
                    // (needed so we can immutably borrow asset_trackers for cross-asset)
                    let current_price = tracker.current_price();
                    let drift = tracker.drift_per_sec();
                    let slow_drift = tracker.slow_drift_per_sec();
                    let n_obs = tracker.state_for_persist().2;
                    let vol_regime = tracker.vol_regime();
                    let vr = tracker.variance_ratio(16);

                    // Get flow snapshot (release mutable borrow on flow_trackers)
                    let flow_snap = if let Some(ft) = flow_trackers.get(&tick.asset) {
                        ft.snapshot(tick.ts)
                    } else {
                        crate::flow::FlowSnapshot {
                            ofi_10s: 0.0,
                            vol_ratio: 1.0,
                            large_trade: false,
                        }
                    };

                    // Emit signals for all markets tracking this asset
                    for (market_id, meta) in &market_meta {
                        if meta.asset != tick.asset {
                            continue;
                        }
                        if tick.ts >= meta.resolution_ts {
                            continue;
                        }

                        // Capture open spot price — must be AFTER the market's
                        // open_ts so we use the same reference the market uses.
                        // If the current spot tick is before market open, skip.
                        if tick.ts < meta.open_ts {
                            continue;
                        }
                        let open_spot = *open_spot_prices
                            .entry(market_id.clone())
                            .or_insert(tick.price);

                        // Only emit signals after enough of the window has elapsed.
                        // UpDown (short window): 20% elapsed for momentum accuracy.
                        // Strike (daily): 5% — signal is valid once warmup completes,
                        // and waiting 20% of a 24h window (4.8h) wastes edge.
                        let total_duration =
                            (meta.resolution_ts - meta.open_ts) as f64;
                        let elapsed_us =
                            (tick.ts - meta.open_ts) as f64;
                        let min_elapsed = if matches!(
                            meta.market_type,
                            MarketType::UpDown
                        ) {
                            MIN_ELAPSED_PCT
                        } else {
                            0.05
                        };
                        if total_duration > 0.0
                            && (elapsed_us / total_duration) < min_elapsed
                        {
                            continue;
                        }

                        // Throttle: 1 signal/sec for UpDown, 1/30sec for strike.
                        // Strike markets change slowly — no need to flood the
                        // decision actor with 1/sec for 24h markets.
                        let throttle_us = if matches!(
                            meta.market_type,
                            MarketType::UpDown
                        ) {
                            1_000_000 // 1 second
                        } else {
                            30_000_000 // 30 seconds
                        };
                        let prev = last_signal_ts.get(market_id).copied().unwrap_or(0);
                        if (tick.ts - prev) < throttle_us {
                            continue;
                        }
                        last_signal_ts.insert(market_id.clone(), tick.ts);

                        let time_to_expiry =
                            ((meta.resolution_ts - tick.ts) as f64) / 1_000_000.0;

                        // Unified log-normal model for all market types.
                        // For UpDown: treats as Above(open_spot) — natural time decay.
                        let p_hat = compute_p_hat_lognormal(
                            current_price,
                            meta.market_type,
                            open_spot,
                            vol,
                            drift,
                            time_to_expiry,
                        );

                        // Dual-timescale drift alignment: require BOTH fast
                        // drift (~5 min) AND slow drift (~20 min) to agree
                        // with the signal direction. Prevents short-term
                        // bounces from triggering bets against the prevailing
                        // trend (e.g., YES bets during a broader downtrend).
                        //
                        // Task 7: Relax slow drift requirement when OFI
                        // confirms the signal direction with strong flow.
                        let signal_says_up = p_hat > 0.5;
                        let fast_agrees = (drift > 0.0) == signal_says_up;
                        let slow_agrees = (slow_drift > 0.0) == signal_says_up;

                        let ofi_agrees = if signal_says_up {
                            flow_snap.ofi_10s > 0.5
                        } else {
                            flow_snap.ofi_10s < -0.5
                        };
                        let flow_override = ofi_agrees && flow_snap.vol_ratio > 1.5;

                        // Extreme conviction override: skip slow drift for
                        // very strong signals where waiting costs the edge.
                        let extreme_signal = p_hat < 0.15 || p_hat > 0.85;

                        if !fast_agrees
                            || (!slow_agrees && !flow_override && !extreme_signal)
                        {
                            tracing::debug!(
                                market = %market_id,
                                p_hat = format_args!("{p_hat:.4}"),
                                drift = format_args!("{drift:.2e}"),
                                slow_drift = format_args!("{slow_drift:.2e}"),
                                ofi_10s = format_args!("{:.2}", flow_snap.ofi_10s),
                                vol_ratio = format_args!("{:.2}", flow_snap.vol_ratio),
                                fast_agrees,
                                slow_agrees,
                                flow_override,
                                "signal blocked by drift alignment"
                            );
                            continue;
                        }

                        // Compute elapsed_pct for this market
                        let elapsed_pct = if total_duration > 0.0 {
                            elapsed_us / total_duration
                        } else {
                            0.0
                        };

                        // Cross-asset lead-lag signal (Task 5)
                        let cross_asset_signal = {
                            let other_asset = match meta.asset {
                                Asset::BTC => Asset::ETH,
                                Asset::ETH => Asset::BTC,
                            };
                            if let Some(other_tracker) =
                                asset_trackers.get(&other_asset)
                            {
                                let other_drift = other_tracker.drift_per_sec();
                                (other_drift / (INITIAL_VOL * 0.5)).tanh()
                            } else {
                                0.0
                            }
                        };

                        let confidence = (p_hat - 0.5).abs() * 2.0;

                        // Raw displacement from open price (%).
                        // Positive = spot above open, negative = below.
                        let displacement_pct = if open_spot > 0.0 {
                            (current_price / open_spot - 1.0) * 100.0
                        } else {
                            0.0
                        };

                        let sig = Signal {
                            market_id: market_id.clone(),
                            p_hat,
                            confidence,
                            prior: 0.5,
                            n_observations: n_obs,
                            ts: tick.ts,
                            ofi_10s: flow_snap.ofi_10s,
                            vol_ratio: flow_snap.vol_ratio,
                            large_trade: flow_snap.large_trade,
                            vol_regime,
                            cross_asset_signal,
                            elapsed_pct,
                            displacement_pct,
                            variance_ratio: vr,
                        };

                        tracing::debug!(
                            market = %market_id,
                            p_hat = format_args!("{p_hat:.4}"),
                            confidence = format_args!("{confidence:.4}"),
                            drift = format_args!("{drift:.2e}"),
                            vol_regime = format_args!("{vol_regime:.2}"),
                            ofi_10s = format_args!("{:.2}", flow_snap.ofi_10s),
                            "signal emitted"
                        );
                        let _ = db_tx.try_send(DbEvent::Signal(sig.clone()));
                        if let Err(e) = signal_tx.try_send(sig) {
                            tracing::warn!("signal channel full/closed: {e}");
                        }
                    }
                }
            }
        }
    }
}
