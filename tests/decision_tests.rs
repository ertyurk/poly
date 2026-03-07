use polymarket_bot::actors::decision::*;
use polymarket_bot::types::*;

#[test]
fn test_compute_edge() {
    let edge = compute_edge(0.65, 0.50);
    assert!((edge - 0.15).abs() < 1e-10);
}

#[test]
fn test_compute_edge_negative() {
    let edge = compute_edge(0.40, 0.50);
    assert!((edge - (-0.10)).abs() < 1e-10);
}

#[test]
fn test_fee_at_50_percent_15m() {
    let fee = lookup_fee(0.50, Window::FifteenMin, &default_fee_schedule_15m());
    assert!(fee > 0.03 && fee < 0.04);
}

#[test]
fn test_fee_at_extremes_lower() {
    let fee = lookup_fee(0.05, Window::FifteenMin, &default_fee_schedule_15m());
    assert!(fee < 0.02);
}

#[test]
fn test_effective_edge_positive() {
    let eff = effective_edge(0.15, 0.02);
    assert!((eff - 0.13).abs() < 1e-10);
}

#[test]
fn test_effective_edge_fee_exceeds() {
    let eff = effective_edge(0.02, 0.0315);
    assert!(eff < 0.0);
}

#[test]
fn test_entry_gate_passes() {
    let passes = check_entry_gate(0.15, 0.05, 0.5, 100_000.0, 1.0);
    assert!(passes);
}

#[test]
fn test_entry_gate_fails_small_edge() {
    let passes = check_entry_gate(0.03, 0.05, 0.5, 100_000.0, 1.0);
    assert!(!passes);
}

#[test]
fn test_stealth_cap() {
    let capped = apply_stealth_cap(5000.0, 50_000.0, 0.02);
    assert!((capped - 1000.0).abs() < 1e-10);
}

#[test]
fn test_stealth_cap_no_change() {
    let capped = apply_stealth_cap(500.0, 50_000.0, 0.02);
    assert!((capped - 500.0).abs() < 1e-10);
}

#[test]
fn test_decide_skip_low_confidence() {
    let result = decide(
        0.65, 0.50, 0.01, 0.05, 100_000.0, 0.5, 100_000.0, 50_000.0, 0.02, 0.60, 0.30, "mkt-1",
    );
    assert!(result.is_err());
}

#[test]
fn test_decide_trade_succeeds() {
    let result = decide(
        0.65, 0.50, 0.01, 0.05, 100_000.0, 0.5, 100_000.0, 50_000.0, 0.02, 0.20, 0.30, "mkt-1",
    );
    assert!(result.is_ok());
    let dec = result.unwrap();
    assert_eq!(dec.side, Side::Yes);
    assert!(dec.size > 0.0);
}
