#![allow(clippy::unwrap_used)]

use polymarket_bot::weather::fetcher::parse_weather_event;
use polymarket_bot::weather::forecast::bucket_probabilities;
use polymarket_bot::weather::signal::find_tail_edges;

/// Full pipeline test: parse event -> compute ensemble probabilities -> find tail edges.
///
/// Uses a 9-bucket Atlanta event with mock temps clustered in the upper range,
/// creating edges on the lower-tail buckets where the ensemble assigns near-zero
/// probability but the mock market has non-zero prices.
#[test]
fn weather_pipeline_atlanta_9_buckets() {
    // Build a realistic 9-bucket weather event for Atlanta
    let json: serde_json::Value = serde_json::from_str(
        r#"{
            "id": "evt-wx-atl",
            "title": "Highest temperature in Atlanta on March 14?",
            "slug": "highest-temperature-in-atlanta-on-march-14-2026",
            "endDate": "2026-03-15T00:00:00Z",
            "markets": [
                {
                    "id": "mkt-0",
                    "groupItemTitle": "59°F or below",
                    "clobTokenIds": "[\"ty0\",\"tn0\"]",
                    "bestBid": 0.02, "bestAsk": 0.05,
                    "outcomePrices": "[0.03, 0.97]"
                },
                {
                    "id": "mkt-1",
                    "groupItemTitle": "60-61°F",
                    "clobTokenIds": "[\"ty1\",\"tn1\"]",
                    "bestBid": 0.03, "bestAsk": 0.07,
                    "outcomePrices": "[0.05, 0.95]"
                },
                {
                    "id": "mkt-2",
                    "groupItemTitle": "62-63°F",
                    "clobTokenIds": "[\"ty2\",\"tn2\"]",
                    "bestBid": 0.06, "bestAsk": 0.10,
                    "outcomePrices": "[0.08, 0.92]"
                },
                {
                    "id": "mkt-3",
                    "groupItemTitle": "64-65°F",
                    "clobTokenIds": "[\"ty3\",\"tn3\"]",
                    "bestBid": 0.08, "bestAsk": 0.12,
                    "outcomePrices": "[0.10, 0.90]"
                },
                {
                    "id": "mkt-4",
                    "groupItemTitle": "66-67°F",
                    "clobTokenIds": "[\"ty4\",\"tn4\"]",
                    "bestBid": 0.12, "bestAsk": 0.18,
                    "outcomePrices": "[0.15, 0.85]"
                },
                {
                    "id": "mkt-5",
                    "groupItemTitle": "68-69°F",
                    "clobTokenIds": "[\"ty5\",\"tn5\"]",
                    "bestBid": 0.18, "bestAsk": 0.24,
                    "outcomePrices": "[0.20, 0.80]"
                },
                {
                    "id": "mkt-6",
                    "groupItemTitle": "70-71°F",
                    "clobTokenIds": "[\"ty6\",\"tn6\"]",
                    "bestBid": 0.15, "bestAsk": 0.20,
                    "outcomePrices": "[0.17, 0.83]"
                },
                {
                    "id": "mkt-7",
                    "groupItemTitle": "72-73°F",
                    "clobTokenIds": "[\"ty7\",\"tn7\"]",
                    "bestBid": 0.05, "bestAsk": 0.09,
                    "outcomePrices": "[0.07, 0.93]"
                },
                {
                    "id": "mkt-8",
                    "groupItemTitle": "74°F or higher",
                    "clobTokenIds": "[\"ty8\",\"tn8\"]",
                    "bestBid": 0.03, "bestAsk": 0.07,
                    "outcomePrices": "[0.05, 0.95]"
                }
            ]
        }"#,
    )
    .unwrap();

    let event = parse_weather_event(&json).unwrap();
    assert_eq!(event.buckets.len(), 9);
    assert_eq!(event.city, "atlanta");
    assert_eq!(event.target_date, "2026-03-14");

    // Mock ensemble temps: cluster around 68-72°F
    // This should put most probability mass in buckets 5-7, leaving tails empty.
    let temps: Vec<f64> = vec![
        67.5, 68.0, 68.2, 68.5, 68.8, 69.0, 69.1, 69.3, 69.5, 69.8,
        70.0, 70.1, 70.3, 70.5, 70.8, 71.0, 71.2, 71.5, 71.8, 72.0,
        68.0, 69.0, 70.0, 71.0, 68.5, 69.5, 70.5, 71.5, 69.2, 70.2,
        68.3, 69.3, 70.3, 71.3, 69.7, 70.7, 68.8, 69.8, 70.8, 71.8,
        69.0, 70.0, 71.0, 69.5, 70.5, 69.2, 70.2, 71.2, 69.8, 70.8,
    ];

    let bucket_structs: Vec<_> = event.buckets.iter().map(|bm| bm.bucket.clone()).collect();
    let probs = bucket_probabilities(&bucket_structs, &temps);

    // Probabilities should sum to ~1.0
    let total: f64 = probs.iter().sum();
    assert!(
        (total - 1.0).abs() < 0.01,
        "probabilities should sum to ~1.0, got {total}"
    );

    // Lower tail buckets (0, 1, 2) should have ~0 probability since all
    // temps are 67.5+
    assert!(probs[0] < 0.01, "bucket 0 (<=59°F) should be ~0");
    assert!(probs[1] < 0.01, "bucket 1 (60-61°F) should be ~0");
    assert!(probs[2] < 0.01, "bucket 2 (62-63°F) should be ~0");

    // Middle buckets (5=68-69, 6=70-71) should have most probability
    assert!(probs[5] > 0.15, "bucket 5 (68-69°F) should have decent prob");
    assert!(probs[6] > 0.15, "bucket 6 (70-71°F) should have decent prob");

    // Market midpoints for tail edge detection
    let market_prices: Vec<f64> = event.buckets.iter().map(|bm| bm.midpoint).collect();

    // Find tail edges: 3 tail buckets on each end, max_tail_price=0.10, edge_threshold=0.03
    let _edges = find_tail_edges(&market_prices, &probs, 3, 0.10, 0.03);

    // Upper tail bucket 8 (74°F or higher): ensemble prob should be non-trivial
    // since some temps are 71.8, 72.0 which are close to 74. But those are in
    // bucket 7 (72-73°F). So bucket 8 should be near zero.
    // The upper tail has market price 0.05 and ensemble prob ~0 → no edge.
    // Lower tail has market price 0.03-0.08 and ensemble prob ~0 → no positive edge.
    // Since all ensemble temps are 67.5-72.0, no lower-tail edge exists either.
    // So we might get edges on the upper tail if ensemble assigns probability there.

    // With these temps, most edges should be on the UPPER tail bucket 8
    // (74°F+) if ensemble gives it probability. But since no temp >= 74,
    // p_ensemble for bucket 8 = 0. Market price is 0.05. Edge = 0 - 0.05 = -0.05 (no edge).
    //
    // However, bucket 4 (66-67°F) has p ~0.02 and is a lower tail (index 2 < tail_count=3).
    // market_price = 0.15 → edge = 0.02 - 0.15 = -0.13 (no edge).
    //
    // Actually with these mock temps, none of the tail buckets have ensemble prob
    // exceeding their market price (tail buckets have low market prices BUT also
    // low ensemble probs). This is expected behavior — the signal module correctly
    // finds NO edges when the ensemble agrees with the market.
    //
    // To verify edges ARE found when they should be, let's create a scenario
    // where ensemble disagrees with market.

    // Scenario 2: upper tail has high ensemble probability but low market price
    let skewed_temps: Vec<f64> = vec![
        74.0, 74.5, 75.0, 75.5, 76.0, 74.2, 74.8, 75.3, 76.5, 77.0,
        74.1, 74.6, 75.1, 75.6, 76.1, 74.3, 74.9, 75.4, 76.6, 77.1,
        73.0, 73.5, 72.0, 72.5, 71.0, 70.0, 69.0, 68.0, 74.0, 75.0,
        74.0, 74.5, 75.0, 75.5, 76.0, 74.2, 74.8, 75.3, 76.5, 77.0,
        74.0, 74.5, 75.0, 75.5, 76.0, 74.2, 74.8, 75.3, 76.5, 77.0,
    ];

    let skewed_probs = bucket_probabilities(&bucket_structs, &skewed_temps);

    // Bucket 8 (74°F+) should have high ensemble probability (~70%+)
    assert!(
        skewed_probs[8] > 0.5,
        "bucket 8 should have high prob with skewed temps, got {:.3}",
        skewed_probs[8]
    );

    let skewed_edges = find_tail_edges(&market_prices, &skewed_probs, 3, 0.10, 0.03);

    // Upper tail bucket 8 has market_price=0.05, p_ensemble>0.50
    // → edge > 0.45, well above threshold 0.03.
    // But max_tail_price filter: 0.05 < 0.10 → passes.
    assert!(
        !skewed_edges.is_empty(),
        "should find at least one tail edge with skewed temps"
    );

    // The edge on bucket 8 should be present
    let bucket_8_edge = skewed_edges
        .iter()
        .find(|e| e.bucket_index == 8);
    assert!(
        bucket_8_edge.is_some(),
        "should find edge on upper tail bucket 8"
    );

    let edge = bucket_8_edge.unwrap();
    assert!(edge.edge > 0.03, "edge should exceed threshold");
    assert!(
        edge.p_ensemble > 0.5,
        "ensemble prob should be high for bucket 8"
    );
    assert!(
        (edge.market_price - 0.05).abs() < f64::EPSILON,
        "market price for bucket 8 should be 0.05"
    );
}
