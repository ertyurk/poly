use std::collections::{HashMap, VecDeque};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::math::{bayesian, decay};
use crate::types::*;

/// Minimum fraction of window elapsed before emitting signals.
/// Trading later = more information = higher accuracy.
/// At 65% elapsed in a 5-min window (1.75min remaining), a 0.1% BTC move
/// gives p_hat ≈ 0.90, yielding high-confidence directional bets.
const MIN_ELAPSED_PCT: f64 = 0.65;

/// Maximum number of observations to retain per market window.
const MAX_OBSERVATIONS: usize = 300;

/// Initial per-second volatility estimate (~2.5% daily for BTC).
const INITIAL_VOL: f64 = 0.00008;

/// Minimum volatility floor to prevent division by near-zero.
const MIN_VOL: f64 = 0.00002;

/// Minimum valid updates before emitting signals (~30 seconds of data).
const MIN_TICKS: u32 = 30;

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

struct AssetTracker {
    last_price: f64,
    last_ts: TsMicros,
    /// Count of valid updates (spaced ≥ 0.1s apart) — NOT raw ticks.
    valid_ticks: u32,
    /// Exponentially-weighted per-second variance estimate.
    variance: f64,
    /// Exponentially-weighted per-second drift estimate.
    drift: f64,
    lambda: f64,
}

impl AssetTracker {
    fn new(lambda: f64) -> Self {
        Self {
            last_price: 0.0,
            last_ts: 0,
            valid_ticks: 0,
            variance: INITIAL_VOL * INITIAL_VOL,
            drift: 0.0,
            lambda,
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

            let alpha = (1.0 - (-self.lambda * dt_secs).exp()).clamp(0.001, 0.5);
            self.variance = (1.0 - alpha) * self.variance + alpha * var_sample;
            self.drift = (1.0 - alpha) * self.drift + alpha * ret_per_sec;

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

    fn current_price(&self) -> f64 {
        self.last_price
    }
}

// ---------------------------------------------------------------------------
// MarketWindow — Bayesian momentum accumulator for UpDown markets
// ---------------------------------------------------------------------------

/// Tracks Bayesian state for one market window.
///
/// Accumulates decay-weighted log-likelihood ratios from consecutive price
/// observations, building conviction in a directional trend. Well-suited for
/// "Up or Down" markets where the question is about directional momentum.
pub struct MarketWindow {
    lambda: f64,
    observations: VecDeque<(f64, f64)>, // (log_likelihood_ratio, elapsed_secs)
    count: u32,
}

impl MarketWindow {
    pub fn new(lambda: f64) -> Self {
        Self {
            lambda,
            observations: VecDeque::with_capacity(MAX_OBSERVATIONS),
            count: 0,
        }
    }

    /// Update with a new price observation.
    /// `ret` = price return since window open.
    /// `vol` = estimated volatility at this timescale.
    /// `elapsed` = seconds since window opened.
    pub fn update(&mut self, ret: f64, vol: f64, elapsed: f64) {
        let p_up = bayesian::probability_from_return(ret, vol).clamp(1e-10, 1.0 - 1e-10);
        let p_down = 1.0 - p_up;

        let ll_ratio = p_up.ln() - p_down.ln();
        self.observations.push_back((ll_ratio, elapsed));
        self.count += 1;

        if self.observations.len() > MAX_OBSERVATIONS {
            self.observations.pop_front();
        }
    }

    /// Current probability estimate for UP, using decay-weighted observations.
    pub fn p_hat(&self) -> f64 {
        if self.observations.is_empty() {
            return 0.5;
        }

        let latest_time = self.observations.back().map_or(0.0, |o| o.1);
        let mut weighted_ll_sum = 0.0;

        for &(ll_ratio, obs_time) in &self.observations {
            let age = (latest_time - obs_time).max(0.0);
            let w = decay::weight(self.lambda, age);
            weighted_ll_sum += w * ll_ratio;
        }

        let (p_up, _) = bayesian::normalize_binary(weighted_ll_sum, 0.0);
        p_up
    }

    #[inline]
    pub fn confidence(&self) -> f64 {
        (self.p_hat() - 0.5).abs() * 2.0
    }

    #[inline]
    pub const fn n_observations(&self) -> u32 {
        self.count
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
}

impl SignalActor {
    pub const fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn run(
        &self,
        mut spot_rx: mpsc::Receiver<SpotPrice>,
        mut market_rx: mpsc::Receiver<MarketState>,
        signal_tx: mpsc::Sender<Signal>,
        db_tx: mpsc::Sender<DbEvent>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let lambda = self.config.strategy.decay.spot_lambda;

        // Per-asset volatility & drift tracking
        let mut asset_trackers: HashMap<Asset, AssetTracker> = HashMap::new();
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
                    tracing::info!("signal actor shutting down");
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

                    let vol = tracker.vol_per_sec();

                    // Emit signals for all markets tracking this asset
                    for (market_id, meta) in &market_meta {
                        if meta.asset != spot.asset {
                            continue;
                        }
                        if spot.ts >= meta.resolution_ts {
                            continue;
                        }

                        // Capture open spot price EARLY — before the elapsed
                        // filter so we measure displacement from actual open,
                        // not from when we start emitting signals.
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
                        let p_hat = compute_p_hat_lognormal(
                            tracker.current_price(),
                            meta.market_type,
                            open_spot,
                            vol,
                            tracker.drift_per_sec(),
                            time_to_expiry,
                        );
                        let n_obs = tracker.valid_ticks;

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
