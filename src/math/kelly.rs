/// Full Kelly criterion: f* = (p_hat - p_market) / (1 - p_market).
/// Returns 0.0 if there is no edge (p_hat <= p_market) or p_market >= 1.0.
#[inline]
pub fn full_kelly(p_hat: f64, p_market: f64) -> f64 {
    let edge = p_hat - p_market;
    if edge <= 0.0 || p_market >= 1.0 {
        return 0.0;
    }
    edge / (1.0 - p_market)
}

/// Fractional Kelly: full_kelly * fraction.
#[inline]
pub fn fractional_kelly(p_hat: f64, p_market: f64, fraction: f64) -> f64 {
    full_kelly(p_hat, p_market) * fraction
}

/// Position size in currency units.
/// size = fractional_kelly(p_hat, p_market, kelly_fraction_config) * bankroll
#[inline]
pub fn position_size(p_hat: f64, p_market: f64, kelly_fraction_config: f64, bankroll: f64) -> f64 {
    fractional_kelly(p_hat, p_market, kelly_fraction_config) * bankroll
}
