/// LMSR cost function: C(q) = b * ln(sum(e^(q_i/b)))
/// Uses log-sum-exp trick for numerical stability.
pub fn cost(quantities: &[f64], b: f64) -> f64 {
    let max_q = quantities.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let sum_exp: f64 = quantities.iter().map(|&q| ((q - max_q) / b).exp()).sum();
    b * (sum_exp.ln() + max_q / b)
}

/// Softmax price function: p_i = e^(q_i/b) / sum(e^(q_j/b))
/// Uses log-sum-exp trick for numerical stability.
pub fn prices(quantities: &[f64], b: f64) -> Vec<f64> {
    let max_q = quantities.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let exps: Vec<f64> = quantities.iter().map(|&q| ((q - max_q) / b).exp()).collect();
    let sum_exp: f64 = exps.iter().sum();
    exps.iter().map(|&e| e / sum_exp).collect()
}

/// Cost of buying `delta` units of `outcome`.
/// trade_cost = C(q') - C(q) where q' has q[outcome] += delta.
pub fn trade_cost(quantities: &[f64], outcome: usize, delta: f64, b: f64) -> f64 {
    let cost_before = cost(quantities, b);
    let mut new_quantities = quantities.to_vec();
    new_quantities[outcome] += delta;
    let cost_after = cost(&new_quantities, b);
    cost_after - cost_before
}

/// Optimal trade size: delta* = b * ln(p_hat / p_market * (1 - p_market) / (1 - p_hat))
pub fn optimal_trade_size(p_hat: f64, p_market: f64, b: f64) -> f64 {
    b * (p_hat / p_market * (1.0 - p_market) / (1.0 - p_hat)).ln()
}

/// Effective spread: c_i = p*(1-p)/b * delta_min
pub fn effective_spread(p: f64, b: f64, delta_min: f64) -> f64 {
    p * (1.0 - p) / b * delta_min
}
