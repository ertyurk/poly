use polymarket_bot::flow::FlowTracker;

#[test]
fn test_ofi_all_buys() {
    let mut ft = FlowTracker::new();
    let base_ts = 1_000_000_000_000i64;
    for i in 0..10 {
        ft.update(1.0, false, base_ts + i * 100_000);
    }
    let snap = ft.snapshot(base_ts + 10 * 100_000);
    assert!(snap.ofi_10s > 0.9, "OFI should be near +1.0 for all buys, got {}", snap.ofi_10s);
}

#[test]
fn test_ofi_all_sells() {
    let mut ft = FlowTracker::new();
    let base_ts = 1_000_000_000_000i64;
    for i in 0..10 {
        ft.update(1.0, true, base_ts + i * 100_000);
    }
    let snap = ft.snapshot(base_ts + 10 * 100_000);
    assert!(snap.ofi_10s < -0.9, "OFI should be near -1.0 for all sells, got {}", snap.ofi_10s);
}

#[test]
fn test_ofi_balanced() {
    let mut ft = FlowTracker::new();
    let base_ts = 1_000_000_000_000i64;
    for i in 0..10 {
        ft.update(1.0, i % 2 == 0, base_ts + i * 100_000);
    }
    let snap = ft.snapshot(base_ts + 10 * 100_000);
    assert!(snap.ofi_10s.abs() < 0.3, "OFI should be near 0 for balanced flow, got {}", snap.ofi_10s);
}

#[test]
fn test_volume_regime_spike() {
    let mut ft = FlowTracker::new();
    let base_ts = 1_000_000_000_000i64;
    for i in 0..60 {
        ft.update(1.0, false, base_ts + i * 1_000_000);
    }
    let spike_ts = base_ts + 60 * 1_000_000;
    for i in 0..10 {
        ft.update(10.0, false, spike_ts + i * 100_000);
    }
    let snap = ft.snapshot(spike_ts + 10 * 100_000);
    assert!(snap.vol_ratio > 2.0, "Volume ratio should spike above 2.0, got {}", snap.vol_ratio);
}

#[test]
fn test_large_trade_detection() {
    let mut ft = FlowTracker::new();
    let base_ts = 1_000_000_000_000i64;
    for i in 0..100 {
        ft.update(0.1, false, base_ts + i * 100_000);
    }
    ft.update(5.0, false, base_ts + 100 * 100_000);
    let snap = ft.snapshot(base_ts + 101 * 100_000);
    assert!(snap.large_trade, "5.0 should be flagged as large trade vs baseline of 0.1");
}

#[test]
fn test_old_data_evicted() {
    let mut ft = FlowTracker::new();
    let base_ts = 1_000_000_000_000i64;
    for i in 0..5 {
        ft.update(1.0, false, base_ts + i * 100_000);
    }
    let snap = ft.snapshot(base_ts + 60_000_000);
    assert!(snap.ofi_10s.abs() < 0.01, "Old trades should be evicted, got OFI {}", snap.ofi_10s);
}
