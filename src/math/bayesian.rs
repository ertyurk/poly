/// Compute log-posterior by summing log-prior and log-likelihoods.
pub fn log_posterior(log_prior: f64, log_likelihoods: &[f64]) -> f64 {
    log_prior + log_likelihoods.iter().sum::<f64>()
}

/// Logistic approximation: 1 / (1 + exp(-1.7 * z)) where z = ret / volatility.
pub fn probability_from_return(ret: f64, volatility: f64) -> f64 {
    let z = ret / volatility;
    1.0 / (1.0 + (-1.7 * z).exp())
}

/// Normalize two log-posteriors to probabilities.
/// Uses log-sum-exp trick for numerical stability.
pub fn normalize_binary(log_up: f64, log_down: f64) -> (f64, f64) {
    let max_log = log_up.max(log_down);
    let exp_up = (log_up - max_log).exp();
    let exp_down = (log_down - max_log).exp();
    let total = exp_up + exp_down;
    (exp_up / total, exp_down / total)
}
