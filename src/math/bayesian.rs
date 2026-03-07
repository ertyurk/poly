/// Logistic approximation scale factor (approximates probit function).
const LOGISTIC_SCALE: f64 = 1.7;

/// Logistic approximation: 1 / (1 + exp(-1.7 * z)) where z = ret / volatility.
/// Returns 0.5 if volatility is zero or negative.
#[inline]
pub fn probability_from_return(ret: f64, volatility: f64) -> f64 {
    if volatility <= 0.0 {
        return 0.5;
    }
    let z = ret / volatility;
    1.0 / (1.0 + (-LOGISTIC_SCALE * z).exp())
}

/// Normalize two log-posteriors to probabilities.
/// Uses log-sum-exp trick for numerical stability.
#[inline]
pub fn normalize_binary(log_up: f64, log_down: f64) -> (f64, f64) {
    let max_log = log_up.max(log_down);
    let exp_up = (log_up - max_log).exp();
    let exp_down = (log_down - max_log).exp();
    let total = exp_up + exp_down;
    (exp_up / total, exp_down / total)
}
