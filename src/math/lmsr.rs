/// Optimal trade size: delta* = b * ln(p_hat / p_market * (1 - p_market) / (1 - p_hat))
#[inline]
pub fn optimal_trade_size(p_hat: f64, p_market: f64, b: f64) -> f64 {
    b * (p_hat / p_market * (1.0 - p_market) / (1.0 - p_hat)).ln()
}

/// Effective spread: c_i = p*(1-p)/b * delta_min
#[inline]
pub fn effective_spread(p: f64, b: f64, delta_min: f64) -> f64 {
    p * (1.0 - p) / b * delta_min
}
