/// Exponential decay weight: exp(-lambda * elapsed_secs).
pub fn weight(lambda: f64, elapsed_secs: f64) -> f64 {
    (-lambda * elapsed_secs).exp()
}

/// Precision-weighted fusion of estimates.
/// Each tuple: (p_hat, variance, weight).
/// Returns sum(w * p_hat / var) / sum(w / var).
/// Falls back to 0.5 if denominator is zero.
pub fn fuse_estimates(estimates: &[(f64, f64, f64)]) -> f64 {
    let (numerator, denominator) = estimates.iter().fold((0.0, 0.0), |(num, den), &(p_hat, var, w)| {
        (num + w * p_hat / var, den + w / var)
    });
    if denominator == 0.0 {
        0.5
    } else {
        numerator / denominator
    }
}
