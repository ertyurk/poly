#![allow(clippy::unwrap_used)]

use polymarket_bot::weather::signal::{find_tail_edges, TailEdge};

// ── TailEdge methods ──────────────────────────────────────────────

#[test]
fn tail_edge_p_hat() {
    let edge = TailEdge {
        bucket_index: 0,
        p_ensemble: 0.25,
        market_price: 0.10,
        edge: 0.15,
    };
    assert!((edge.p_hat() - 0.25).abs() < f64::EPSILON);
}

#[test]
fn tail_edge_relative_edge() {
    let edge = TailEdge {
        bucket_index: 0,
        p_ensemble: 0.25,
        market_price: 0.10,
        edge: 0.15,
    };
    // relative_edge = 0.15 / 0.10 = 1.5
    assert!((edge.relative_edge() - 1.5).abs() < 1e-9);
}

#[test]
fn tail_edge_relative_edge_zero_price() {
    let edge = TailEdge {
        bucket_index: 0,
        p_ensemble: 0.25,
        market_price: 0.0,
        edge: 0.15,
    };
    assert!((edge.relative_edge()).abs() < f64::EPSILON);
}

// ── find_tail_edges ───────────────────────────────────────────────

#[test]
fn basic_one_edge_found() {
    // 5 buckets, tail_count=1 → tails are indices 0 and 4.
    // Bucket 0: ensemble=0.20, market=0.05, edge=0.15 > threshold → included
    // Bucket 4: ensemble=0.10, market=0.08, edge=0.02 < threshold → excluded
    let market_prices = [0.05, 0.25, 0.20, 0.20, 0.08];
    let ensemble_probs = [0.20, 0.25, 0.20, 0.25, 0.10];

    let edges = find_tail_edges(&market_prices, &ensemble_probs, 1, 0.15, 0.05);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].bucket_index, 0);
    assert!((edges[0].edge - 0.15).abs() < 1e-9);
}

#[test]
fn no_cheap_buckets() {
    // All tails priced above max_tail_price → no edges.
    let market_prices = [0.50, 0.30, 0.20];
    let ensemble_probs = [0.40, 0.30, 0.30];
    let edges = find_tail_edges(&market_prices, &ensemble_probs, 1, 0.15, 0.01);
    assert!(edges.is_empty());
}

#[test]
fn multiple_edges() {
    // 9 buckets, tail_count=2 → tails: 0,1,7,8
    let market_prices = [0.04, 0.05, 0.15, 0.20, 0.25, 0.15, 0.10, 0.04, 0.03];
    let ensemble_probs = [0.10, 0.12, 0.13, 0.15, 0.20, 0.13, 0.10, 0.10, 0.08];

    let edges = find_tail_edges(&market_prices, &ensemble_probs, 2, 0.10, 0.03);
    // Bucket 0: edge=0.10-0.04=0.06 > 0.03, price 0.04 <= 0.10 ✓
    // Bucket 1: edge=0.12-0.05=0.07 > 0.03, price 0.05 <= 0.10 ✓
    // Bucket 7: edge=0.10-0.04=0.06 > 0.03, price 0.04 <= 0.10 ✓
    // Bucket 8: edge=0.08-0.03=0.05 > 0.03, price 0.03 <= 0.10 ✓
    assert_eq!(edges.len(), 4);
    assert_eq!(edges[0].bucket_index, 0);
    assert_eq!(edges[1].bucket_index, 1);
    assert_eq!(edges[2].bucket_index, 7);
    assert_eq!(edges[3].bucket_index, 8);
}
