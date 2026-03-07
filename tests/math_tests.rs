use approx::assert_relative_eq;

#[test]
fn test_lmsr_cost_binary_equal_quantities() {
    let cost = polymarket_bot::math::lmsr::cost(&[0.0, 0.0], 100_000.0);
    assert_relative_eq!(cost, 69_314.718, epsilon = 1.0);
}

#[test]
fn test_lmsr_price_equal_quantities() {
    let prices = polymarket_bot::math::lmsr::prices(&[0.0, 0.0], 100_000.0);
    assert_relative_eq!(prices[0], 0.5, epsilon = 1e-10);
    assert_relative_eq!(prices[1], 0.5, epsilon = 1e-10);
}

#[test]
fn test_lmsr_prices_sum_to_one() {
    let prices = polymarket_bot::math::lmsr::prices(&[1000.0, 500.0], 100_000.0);
    assert_relative_eq!(prices[0] + prices[1], 1.0, epsilon = 1e-10);
}

#[test]
fn test_lmsr_trade_cost() {
    let cost = polymarket_bot::math::lmsr::trade_cost(&[0.0, 0.0], 0, 1000.0, 100_000.0);
    assert!(cost > 0.0);
}

#[test]
fn test_lmsr_optimal_trade_size() {
    let size = polymarket_bot::math::lmsr::optimal_trade_size(0.70, 0.50, 100_000.0);
    assert_relative_eq!(size, 84_730.0, epsilon = 100.0);
}

#[test]
fn test_bayesian_log_update_no_data() {
    let log_prior = (0.5_f64).ln();
    let posterior = polymarket_bot::math::bayesian::log_posterior(log_prior, &[]);
    assert_relative_eq!(posterior.exp(), 0.5, epsilon = 1e-10);
}

#[test]
fn test_bayesian_log_update_positive_evidence() {
    let log_prior = (0.5_f64).ln();
    let log_likelihoods = vec![0.1, 0.2, 0.1];
    let posterior = polymarket_bot::math::bayesian::log_posterior(log_prior, &log_likelihoods);
    assert!(posterior.exp() > 0.5);
}

#[test]
fn test_bayesian_probability_from_return() {
    let p = polymarket_bot::math::bayesian::probability_from_return(0.001, 0.005);
    assert!(p > 0.5);
    let p = polymarket_bot::math::bayesian::probability_from_return(-0.001, 0.005);
    assert!(p < 0.5);
}

#[test]
fn test_full_kelly_even_odds() {
    let f = polymarket_bot::math::kelly::full_kelly(0.60, 0.50);
    assert_relative_eq!(f, 0.20, epsilon = 1e-10);
}

#[test]
fn test_half_kelly() {
    let f = polymarket_bot::math::kelly::half_kelly(0.60, 0.50);
    assert_relative_eq!(f, 0.10, epsilon = 1e-10);
}

#[test]
fn test_kelly_no_edge_returns_zero() {
    let f = polymarket_bot::math::kelly::half_kelly(0.50, 0.50);
    assert_relative_eq!(f, 0.0, epsilon = 1e-10);
}

#[test]
fn test_kelly_negative_edge_returns_zero() {
    let f = polymarket_bot::math::kelly::half_kelly(0.40, 0.50);
    assert_relative_eq!(f, 0.0, epsilon = 1e-10);
}

#[test]
fn test_position_size() {
    let size = polymarket_bot::math::kelly::position_size(0.60, 0.50, 0.5, 100_000.0);
    assert_relative_eq!(size, 10_000.0, epsilon = 1.0);
}

#[test]
fn test_decay_weight_at_zero() {
    let w = polymarket_bot::math::decay::weight(0.00230, 0.0);
    assert_relative_eq!(w, 1.0, epsilon = 1e-10);
}

#[test]
fn test_decay_weight_at_half_life() {
    let half_life = (2.0_f64).ln() / 0.00230;
    let w = polymarket_bot::math::decay::weight(0.00230, half_life);
    assert_relative_eq!(w, 0.5, epsilon = 1e-3);
}

#[test]
fn test_decay_weight_at_one_hour() {
    let w = polymarket_bot::math::decay::weight(0.00230, 3600.0);
    assert!(w < 0.001);
}
