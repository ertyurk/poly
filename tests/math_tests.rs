use approx::assert_relative_eq;

#[test]
fn test_lmsr_optimal_trade_size() {
    let size = polymarket_bot::math::lmsr::optimal_trade_size(0.70, 0.50, 100_000.0);
    assert_relative_eq!(size, 84_730.0, epsilon = 100.0);
}

#[test]
fn test_lmsr_effective_spread() {
    let spread = polymarket_bot::math::lmsr::effective_spread(0.5, 100_000.0, 1.0);
    assert_relative_eq!(spread, 0.0000025, epsilon = 1e-7);
}

#[test]
fn test_full_kelly_even_odds() {
    let f = polymarket_bot::math::kelly::full_kelly(0.60, 0.50);
    assert_relative_eq!(f, 0.20, epsilon = 1e-10);
}

#[test]
fn test_fractional_kelly() {
    let f = polymarket_bot::math::kelly::fractional_kelly(0.60, 0.50, 0.5);
    assert_relative_eq!(f, 0.10, epsilon = 1e-10);
}

#[test]
fn test_kelly_no_edge_returns_zero() {
    let f = polymarket_bot::math::kelly::full_kelly(0.50, 0.50);
    assert_relative_eq!(f, 0.0, epsilon = 1e-10);
}

#[test]
fn test_kelly_negative_edge_returns_zero() {
    let f = polymarket_bot::math::kelly::full_kelly(0.40, 0.50);
    assert_relative_eq!(f, 0.0, epsilon = 1e-10);
}

#[test]
fn test_position_size() {
    let size = polymarket_bot::math::kelly::position_size(0.60, 0.50, 0.5, 100_000.0);
    assert_relative_eq!(size, 10_000.0, epsilon = 1.0);
}
