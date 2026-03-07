use polymarket_bot::actors::signal::MarketWindow;

#[test]
fn test_market_window_initial_prior() {
    let w = MarketWindow::new(0.00230);
    assert!((w.p_hat() - 0.5).abs() < 1e-10);
}

#[test]
fn test_market_window_update_positive() {
    let mut w = MarketWindow::new(0.00230);
    w.update(0.002, 0.005, 0.0);
    assert!(w.p_hat() > 0.5);
}

#[test]
fn test_market_window_update_negative() {
    let mut w = MarketWindow::new(0.00230);
    w.update(-0.002, 0.005, 0.0);
    assert!(w.p_hat() < 0.5);
}

#[test]
fn test_market_window_multiple_updates_converge() {
    let mut w = MarketWindow::new(0.00230);
    for _ in 0..10 {
        w.update(0.001, 0.005, 0.0);
    }
    assert!(w.p_hat() > 0.7);
}

#[test]
fn test_market_window_decay_reduces_old_signal() {
    let mut w = MarketWindow::new(0.00230);
    w.update(0.003, 0.005, 0.0);
    let p_early = w.p_hat();
    w.update(0.0, 0.005, 600.0); // neutral observation 10 min later
    let p_late = w.p_hat();
    assert!(p_late < p_early);
}

#[test]
fn test_market_window_observation_count() {
    let mut w = MarketWindow::new(0.00230);
    assert_eq!(w.n_observations(), 0);
    w.update(0.001, 0.005, 0.0);
    w.update(0.001, 0.005, 1.0);
    assert_eq!(w.n_observations(), 2);
}
