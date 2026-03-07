use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::math::{bayesian, decay};
use crate::types::*;

/// Tracks Bayesian state for one market window
pub struct MarketWindow {
    pub market_id: String,
    lambda: f64,
    observations: Vec<(f64, f64)>, // (log_likelihood_ratio, elapsed_secs from window start)
    count: u32,
}

impl MarketWindow {
    pub fn new(market_id: String, lambda: f64) -> Self {
        Self {
            market_id,
            lambda,
            observations: Vec::with_capacity(300),
            count: 0,
        }
    }

    /// Update with a new price observation.
    /// `ret` = price return since window open.
    /// `vol` = estimated volatility at this timescale.
    /// `elapsed` = seconds since window opened.
    pub fn update(&mut self, ret: f64, vol: f64, elapsed: f64) {
        let p_up = bayesian::probability_from_return(ret, vol);
        let p_down = 1.0 - p_up;

        let ll_ratio = p_up.ln() - p_down.ln();
        self.observations.push((ll_ratio, elapsed));
        self.count += 1;

        if self.observations.len() > 300 {
            self.observations.remove(0);
        }
    }

    /// Current probability estimate for UP, using decay-weighted observations.
    pub fn p_hat(&self) -> f64 {
        if self.observations.is_empty() {
            return 0.5;
        }

        let latest_time = self.observations.last().map(|o| o.1).unwrap_or(0.0);
        let mut weighted_ll_sum = 0.0;

        for &(ll_ratio, obs_time) in &self.observations {
            let age = (latest_time - obs_time).max(0.0);
            let w = decay::weight(self.lambda, age);
            weighted_ll_sum += w * ll_ratio;
        }

        // Normalize: p_up = sigmoid(weighted_ll_sum) via binary normalization
        let log_up = weighted_ll_sum; // relative to log_down = 0
        let log_down = 0.0;
        let (p_up, _) = bayesian::normalize_binary(log_up, log_down);
        p_up
    }

    /// Confidence: distance from 0.5, scaled to [0, 1].
    pub fn confidence(&self) -> f64 {
        (self.p_hat() - 0.5).abs() * 2.0
    }

    pub fn n_observations(&self) -> u32 {
        self.count
    }
}

pub struct SignalActor {
    config: Config,
}

impl SignalActor {
    pub fn new(config: Config) -> Self {
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
        let vol: f64 = 0.003; // placeholder volatility estimate

        // Bayesian state per market
        let mut windows: HashMap<String, MarketWindow> = HashMap::new();
        // Track open prices per market
        let mut open_prices: HashMap<String, f64> = HashMap::new();
        // Track which asset each market tracks
        let mut market_assets: HashMap<String, Asset> = HashMap::new();
        // Track latest spot prices per asset
        let mut latest_spot: HashMap<Asset, f64> = HashMap::new();
        // Track market open timestamps
        let mut open_ts_map: HashMap<String, TsMicros> = HashMap::new();

        loop {
            tokio::select! {
                Some(market) = market_rx.recv() => {
                    let market_id = market.market_id.clone();
                    market_assets.insert(market_id.clone(), market.asset);
                    open_ts_map.insert(market_id.clone(), market.open_ts);

                    if let Some(open_price) = market.open_price {
                        open_prices.insert(market_id.clone(), open_price);
                    }

                    windows
                        .entry(market_id)
                        .or_insert_with_key(|id| MarketWindow::new(id.clone(), lambda));
                }

                Some(spot) = spot_rx.recv() => {
                    latest_spot.insert(spot.asset, spot.price);

                    // Update all market windows that track this asset
                    let matching_markets: Vec<String> = market_assets
                        .iter()
                        .filter(|(_, &a)| a == spot.asset)
                        .map(|(id, _)| id.clone())
                        .collect();

                    for market_id in matching_markets {
                        let open_price = match open_prices.get(&market_id) {
                            Some(&p) => p,
                            None => continue,
                        };

                        let ret = (spot.price - open_price) / open_price;
                        let open_ts = open_ts_map.get(&market_id).copied().unwrap_or(spot.ts);
                        let elapsed = ((spot.ts - open_ts) as f64) / 1_000_000.0; // micros to secs

                        if let Some(window) = windows.get_mut(&market_id) {
                            window.update(ret, vol, elapsed);

                            let sig = Signal {
                                market_id: market_id.clone(),
                                p_hat: window.p_hat(),
                                confidence: window.confidence(),
                                prior: 0.5,
                                n_observations: window.n_observations(),
                                ts: spot.ts,
                            };

                            let _ = db_tx.try_send(DbEvent::Signal(sig.clone()));
                            let _ = signal_tx.try_send(sig);
                        }
                    }
                }

                _ = shutdown.changed() => {
                    tracing::info!("signal actor shutting down");
                    return;
                }
            }
        }
    }
}
