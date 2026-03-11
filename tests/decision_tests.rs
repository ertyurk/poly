use polymarket_bot::actors::decision::*;
use polymarket_bot::types::*;

#[test]
fn test_polymarket_fee_at_50_percent() {
    // fee = 0.25 * (0.5 * 0.5)^2 = 0.25 * 0.0625 = 0.015625
    let fee = polymarket_fee_rate(0.50);
    assert!((fee - 0.015625).abs() < 1e-10);
}

#[test]
fn test_polymarket_fee_at_extremes() {
    // At p=0.05: fee = 0.25 * (0.05*0.95)^2 = 0.25 * 0.002256 ≈ 0.000564
    let fee = polymarket_fee_rate(0.05);
    assert!(fee < 0.001);
    // At p=0.95 should be symmetric
    let fee95 = polymarket_fee_rate(0.95);
    assert!((fee - fee95).abs() < 1e-10);
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
        0.65, 0.50, 0.05, 100_000.0, 0.5, 100_000.0, 50_000.0, 0.02, 0.10, 0.60, 0.30,
        "mkt-1", 0.48, 0.52, "", 0.50,
    );
    assert!(result.is_err());
}

#[test]
fn test_decide_trade_succeeds() {
    let result = decide(
        0.65, 0.50, 0.05, 100_000.0, 0.5, 100_000.0, 50_000.0, 0.02, 0.10, 0.20, 0.30,
        "mkt-1", 0.48, 0.52, "", 0.95,
    );
    assert!(result.is_ok());
    let dec = result.unwrap();
    assert_eq!(dec.side, Side::Yes);
    assert!(dec.size_usd > 0.0);
}
