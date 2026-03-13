use polymarket_bot::actors::decision::*;
use polymarket_bot::actors::signal::AssetTracker;
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
        0.65, 0.50, 0.05, 100_000.0, 0.5, 100_000.0, 50_000.0, 0.02, 0.10, 0.60, 0.30, "mkt-1",
        0.48, 0.52, "", 0.50,
    );
    assert!(result.is_err());
}

#[test]
fn test_decide_trade_succeeds() {
    let result = decide(
        0.65, 0.50, 0.05, 100_000.0, 0.5, 100_000.0, 50_000.0, 0.02, 0.10, 0.20, 0.30, "mkt-1",
        0.48, 0.52, "", 0.95,
    );
    assert!(result.is_ok());
    let dec = result.unwrap();
    assert_eq!(dec.side, Side::Yes);
    assert!(dec.size_usd > 0.0);
}

// ---------------------------------------------------------------------------
// Cold-start warmup tests
// ---------------------------------------------------------------------------

/// AssetTracker must NOT report ready during the warmup period (5 min),
/// even if it has received enough ticks.
#[test]
fn test_asset_tracker_blocks_during_warmup() {
    let lambda = 0.00230;
    let mut tracker = AssetTracker::new(lambda);
    let base_ts: i64 = 1_000_000_000_000; // arbitrary start

    // Feed 100 ticks at 1-second intervals (well above MIN_TICKS=30)
    for i in 0..100 {
        let ts = base_ts + i * 1_000_000; // 1 second apart
        let price = 80_000.0 + (i as f64) * 0.1;
        let ready = tracker.update(price, ts);
        // Should NOT be ready — only 100s elapsed, warmup is 300s
        assert!(!ready, "tracker should not be ready at tick {i} (warmup)");
    }
}

/// AssetTracker reports ready AFTER warmup period has elapsed.
#[test]
fn test_asset_tracker_ready_after_warmup() {
    let lambda = 0.00230;
    let mut tracker = AssetTracker::new(lambda);
    let base_ts: i64 = 1_000_000_000_000;

    // Feed ticks for 301 seconds (past 300s warmup) at 1s intervals
    let mut ready = false;
    for i in 0..=301 {
        let ts = base_ts + i * 1_000_000;
        let price = 80_000.0 + (i as f64) * 0.1;
        ready = tracker.update(price, ts);
    }
    assert!(ready, "tracker should be ready after 301s (past warmup)");
}
