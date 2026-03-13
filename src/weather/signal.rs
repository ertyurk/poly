use crate::weather::types::is_tail;

/// A detected edge on a tail bucket where ensemble probability exceeds
/// the market price by more than the configured threshold.
#[derive(Debug, Clone)]
pub struct TailEdge {
    pub bucket_index: u8,
    pub p_ensemble: f64,
    pub market_price: f64,
    pub edge: f64,
}

impl TailEdge {
    /// The ensemble-derived probability for this bucket.
    pub const fn p_hat(&self) -> f64 {
        self.p_ensemble
    }

    /// Edge as a multiple of market price.  Returns 0.0 when price is zero.
    pub fn relative_edge(&self) -> f64 {
        if self.market_price == 0.0 {
            0.0
        } else {
            self.edge / self.market_price
        }
    }
}

/// Scan tail buckets for mispriced opportunities.
///
/// For each bucket index that `is_tail()` returns `true`:
///   - skip if `market_price > max_tail_price`
///   - compute `edge = ensemble_prob - market_price`
///   - include if `edge > edge_threshold`
pub fn find_tail_edges(
    market_prices: &[f64],
    ensemble_probs: &[f64],
    tail_count: u8,
    max_tail_price: f64,
    edge_threshold: f64,
) -> Vec<TailEdge> {
    let n = market_prices.len().min(ensemble_probs.len());
    let total = n as u8;

    (0..n)
        .filter_map(|i| {
            let idx = i as u8;
            if !is_tail(idx, total, tail_count) {
                return None;
            }
            let mp = market_prices[i];
            if mp > max_tail_price {
                return None;
            }
            let edge = ensemble_probs[i] - mp;
            if edge > edge_threshold {
                Some(TailEdge {
                    bucket_index: idx,
                    p_ensemble: ensemble_probs[i],
                    market_price: mp,
                    edge,
                })
            } else {
                None
            }
        })
        .collect()
}
