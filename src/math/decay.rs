/// Exponential decay weight: exp(-lambda * elapsed_secs).
#[inline]
pub fn weight(lambda: f64, elapsed_secs: f64) -> f64 {
    (-lambda * elapsed_secs).exp()
}
