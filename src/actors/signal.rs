use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::types::*;

/// Minimum fraction of window elapsed before emitting signals.
/// At 50% elapsed: enough data for directional signal with less time
/// for reversals. Market may still have edge available.
const MIN_ELAPSED_PCT: f64 = 0.50;

/// Initial per-second volatility estimate (~2.5% daily for BTC).
const INITIAL_VOL: f64 = 0.00008;

/// Minimum volatility floor to prevent division by near-zero.
const MIN_VOL: f64 = 0.00002;

/// Minimum valid updates before emitting signals (~30 seconds of data).
const MIN_TICKS: u32 = 30;

/// Safety margin on realized volatility to prevent overconfidence.
/// EWM variance from noisy tick data tends to underestimate true vol;
/// this multiplier inflates σ so the model produces more conservative
/// probability estimates (smaller z-scores → p_hat closer to 0.5).
const VOL_SAFETY_MARGIN: f64 = 1.3;

/// Slow drift lambda divisor: slow_lambda = spot_lambda / SLOW_DRIFT_FACTOR.
/// Factor of 4 → slow drift half-life is 4× longer than fast drift.
/// Fast (spot_lambda=0.00230): half-life ≈ 5 min.
/// Slow (spot_lambda/4):       half-life ≈ 20 min.
/// Requires both timescales to agree before trading — prevents
/// short-term bounces from triggering bets against the prevailing trend.
const SLOW_DRIFT_FACTOR: f64 = 4.0;

// ---------------------------------------------------------------------------
// Log-normal probability model (for Above/Below/Between markets)
// ---------------------------------------------------------------------------

/// Logistic approximation of standard normal CDF: Φ(x) ≈ 1/(1 + exp(-1.7x))
#[inline]
fn normal_cdf_approx(x: f64) -> f64 {
    1.0 / (1.0 + (-1.7 * x).exp())
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

    let drift_t = drift_per_sec * time_to_expiry_secs;

    match market_type {
        MarketType::Above(strike) => {
            if strike <= 0.0 {
                return 1.0;
            }
            let d = ((current_price / strike).ln() + drift_t) / sigma_t;
            normal_cdf_approx(d)
        }
        MarketType::Below(strike) => {
            if strike <= 0.0 {
                return 0.0;
            }
            let d = ((current_price / strike).ln() + drift_t) / sigma_t;
            1.0 - normal_cdf_approx(d)
        }
        MarketType::Between(lo, hi) => {
            if lo <= 0.0 || hi <= 0.0 {
                return 0.5;
            }
            let d_lo = ((current_price / lo).ln() + drift_t) / sigma_t;
            let d_hi = ((current_price / hi).ln() + drift_t) / sigma_t;
            (normal_cdf_approx(d_lo) - normal_cdf_approx(d_hi)).clamp(0.0, 1.0)
        }
        MarketType::UpDown => {
            // UpDown = "price went up from open" = Above(open_spot_price)
            if open_spot_price <= 0.0 {
                return 0.5;
            }
            let d = ((current_price / open_spot_price).ln() + drift_t) / sigma_t;
            normal_cdf_approx(d)
        }
    }
}

// ---------------------------------------------------------------------------
// AssetTracker — estimates realized vol and drift from tick stream
// ---------------------------------------------------------------------------

pub struct AssetTracker {
    last_price: f64,
    last_ts: TsMicros,
    /// Count of valid updates (spaced ≥ 0.1s apart) — NOT raw ticks.
    valid_ticks: u32,
    /// Exponentially-weighted per-second variance estimate.
    variance: f64,
    /// Fast drift: EWM per-second drift (half-life ≈ 5 min).
    drift: f64,
    /// Slow drift: EWM per-second drift (half-life ≈ 20 min).
    /// Used as trend confirmation — both must agree before trading.
    slow_drift: f64,
    lambda: f64,
    slow_lambda: f64,
}

impl AssetTracker {
    fn new(lambda: f64) -> Self {
        let slow_lambda = lambda / SLOW_DRIFT_FACTOR;
        Self {
            last_price: 0.0,
            last_ts: 0,
            valid_ticks: 0,
            variance: INITIAL_VOL * INITIAL_VOL,
            drift: 0.0,
            slow_drift: 0.0,
            lambda,
            slow_lambda,
        }
    }

    /// Update with a new price tick. Returns true if enough data for signals.
    fn update(&mut self, price: f64, ts: TsMicros) -> bool {
        if self.valid_ticks == 0 {
            self.last_price = price;
            self.last_ts = ts;
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
            let slow_alpha =
                (1.0 - (-self.slow_lambda * dt_secs).exp()).clamp(0.001, 0.5);
            self.slow_drift =
                (1.0 - slow_alpha) * self.slow_drift + slow_alpha * ret_per_sec;

            self.last_price = price;
            self.last_ts = ts;
            self.valid_ticks += 1;
        }

        self.valid_ticks >= MIN_TICKS
    }

    fn vol_per_sec(&self) -> f64 {
        self.variance.sqrt().max(MIN_VOL)
    }

    fn drift_per_sec(&self) -> f64 {
        self.drift
    }

    fn slow_drift_per_sec(&self) -> f64 {
        self.slow_drift
    }

    fn current_price(&self) -> f64 {
        self.last_price
    }

    /// Restore state from a previous session to avoid cold-start warm-up.
    pub fn restore(
        last_price: f64,
        last_ts: TsMicros,
        valid_ticks: u32,
        variance: f64,
        drift: f64,
        slow_drift: f64,
        lambda: f64,
    ) -> Self {
        let slow_lambda = lambda / SLOW_DRIFT_FACTOR;
        Self {
            last_price,
            last_ts,
            valid_ticks,
            variance,
            drift,
            slow_drift,
            lambda,
            slow_lambda,
        }
    }

    /// Accessors for warm-up persistence (keep fields private).
    pub fn state_for_persist(&self) -> (f64, TsMicros, u32, f64, f64, f64, f64) {
        (
            self.last_price,
            self.last_ts,
            self.valid_ticks,
            self.variance,
            self.drift,
            self.slow_drift,
            self.lambda,
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
        mut spot_rx: mpsc::Receiver<SpotPrice>,
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
            tracing::info!(
                asset = %asset,
                valid_ticks = tracker.state_for_persist().2,
                vol = vol,
                "restored signal state from previous session"
            );
        }
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
                    tracing::info!("signal actor shutting down — persisting state");
                    for (asset, tracker) in &asset_trackers {
                        let (last_price, last_ts, valid_ticks, variance, drift, slow_drift, lam) =
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

                Some(spot) = spot_rx.recv() => {
                    // Update per-asset tracker (vol/drift estimation)
                    let tracker = asset_trackers
                        .entry(spot.asset)
                        .or_insert_with(|| AssetTracker::new(lambda));

                    let ready = tracker.update(spot.price, spot.ts);
                    if !ready {
                        continue;
                    }

                    // Safety margin on vol prevents overconfidence from noisy
                    // EWM variance estimates. Higher vol → wider σ√T →
                    // smaller z-score → more conservative p_hat.
                    let vol = tracker.vol_per_sec() * VOL_SAFETY_MARGIN;

                    // Emit signals for all markets tracking this asset
                    for (market_id, meta) in &market_meta {
                        if meta.asset != spot.asset {
                            continue;
                        }
                        if spot.ts >= meta.resolution_ts {
                            continue;
                        }

                        // Capture open spot price — must be AFTER the market's
                        // open_ts so we use the same reference the market uses.
                        // If the current spot tick is before market open, skip.
                        if spot.ts < meta.open_ts {
                            continue;
                        }
                        let open_spot = *open_spot_prices
                            .entry(market_id.clone())
                            .or_insert(spot.price);

                        // Only emit signals after enough of the window has elapsed.
                        // Trading later = more data = higher accuracy.
                        let total_duration =
                            (meta.resolution_ts - meta.open_ts) as f64;
                        let elapsed_us =
                            (spot.ts - meta.open_ts) as f64;
                        if total_duration > 0.0
                            && (elapsed_us / total_duration) < MIN_ELAPSED_PCT
                        {
                            continue;
                        }

                        // Throttle: 1 signal/sec/market
                        let prev = last_signal_ts.get(market_id).copied().unwrap_or(0);
                        if (spot.ts - prev) < 1_000_000 {
                            continue;
                        }
                        last_signal_ts.insert(market_id.clone(), spot.ts);

                        let time_to_expiry =
                            ((meta.resolution_ts - spot.ts) as f64) / 1_000_000.0;

                        // Unified log-normal model for all market types.
                        // For UpDown: treats as Above(open_spot) — natural time decay.
                        let drift = tracker.drift_per_sec();
                        let p_hat = compute_p_hat_lognormal(
                            tracker.current_price(),
                            meta.market_type,
                            open_spot,
                            vol,
                            drift,
                            time_to_expiry,
                        );
                        let n_obs = tracker.state_for_persist().2;

                        // Dual-timescale drift alignment: require BOTH fast
                        // drift (~5 min) AND slow drift (~20 min) to agree
                        // with the signal direction. Prevents short-term
                        // bounces from triggering bets against the prevailing
                        // trend (e.g., YES bets during a broader downtrend).
                        let signal_says_up = p_hat > 0.5;
                        let slow_drift = tracker.slow_drift_per_sec();
                        let fast_agrees = (drift > 0.0) == signal_says_up;
                        let slow_agrees = (slow_drift > 0.0) == signal_says_up;
                        if !fast_agrees || !slow_agrees {
                            continue;
                        }

                        // NO-only for UpDown markets: empirical results across
                        // 39 trades show NO=19/19 (100%) vs YES=0/20 (0%).
                        // Short-term crypto UpDown markets have structural
                        // NO bias: random walk + mean reversion + bid-ask
                        // bounce make "stay near/below open" more likely than
                        // "sustained move above open" in 5–60 min windows.
                        // YES bets are disabled until the model can reliably
                        // distinguish genuine breakouts from noise bounces.
                        if signal_says_up
                            && matches!(meta.market_type, MarketType::UpDown)
                        {
                            continue;
                        }

                        let confidence = (p_hat - 0.5).abs() * 2.0;

                        let sig = Signal {
                            market_id: market_id.clone(),
                            p_hat,
                            confidence,
                            prior: 0.5,
                            n_observations: n_obs,
                            ts: spot.ts,
                        };

                        let _ = db_tx.try_send(DbEvent::Signal(sig.clone()));
                        let _ = signal_tx.try_send(sig);
                    }
                }
            }
        }
    }
}
