#![allow(clippy::unwrap_used)]

use polymarket_bot::weather::forecast::{
    bucket_probabilities, build_open_meteo_url, parse_ensemble_temps,
};
use polymarket_bot::weather::types::{Bucket, CityConfig};

// ── build_open_meteo_url ──────────────────────────────────────────

#[test]
fn url_fahrenheit_includes_temp_unit() {
    let city = CityConfig::find("atlanta").unwrap();
    let url = build_open_meteo_url(city, "2026-03-15");
    assert!(url.contains("latitude=33.749"));
    assert!(url.contains("longitude=-84.388"));
    assert!(url.contains("start_date=2026-03-15"));
    assert!(url.contains("end_date=2026-03-15"));
    assert!(url.contains("&temperature_unit=fahrenheit"));
    assert!(url.contains("models=ecmwf_ifs025,gfs025_ensemble"));
}

#[test]
fn url_celsius_omits_temp_unit() {
    let city = CityConfig::find("london").unwrap();
    let url = build_open_meteo_url(city, "2026-03-15");
    assert!(url.contains("latitude=51.508"));
    assert!(!url.contains("temperature_unit"));
}

// ── parse_ensemble_temps ──────────────────────────────────────────

#[test]
fn parse_three_members() {
    let json: serde_json::Value = serde_json::from_str(
        r#"{
            "daily": {
                "time": ["2026-03-15"],
                "temperature_2m_max_member01": [72.5],
                "temperature_2m_max_member02": [74.0],
                "temperature_2m_max_member03": [71.0]
            }
        }"#,
    )
    .unwrap();

    let temps = parse_ensemble_temps(&json).unwrap();
    assert_eq!(temps.len(), 3);
    assert!((temps[0] - 72.5).abs() < f64::EPSILON);
    assert!((temps[1] - 74.0).abs() < f64::EPSILON);
    assert!((temps[2] - 71.0).abs() < f64::EPSILON);
}

#[test]
fn parse_no_members_returns_none() {
    let json: serde_json::Value = serde_json::from_str(
        r#"{
            "daily": {
                "time": ["2026-03-15"]
            }
        }"#,
    )
    .unwrap();

    assert!(parse_ensemble_temps(&json).is_none());
}

// ── bucket_probabilities ──────────────────────────────────────────

#[test]
fn bucket_probs_sum_to_one() {
    // 9 Atlanta-style Fahrenheit buckets covering [65..82+)
    let labels = [
        "65°F or below",
        "66-67°F",
        "68-69°F",
        "70-71°F",
        "72-73°F",
        "74-75°F",
        "76-77°F",
        "78-79°F",
        "80°F or higher",
    ];
    let buckets: Vec<Bucket> = labels.iter().filter_map(|l| Bucket::parse(l)).collect();
    assert_eq!(buckets.len(), 9);

    // 10 ensemble temps — each lands in exactly one bucket.
    // Lower tail (-inf, 67), 66-67=[66,68), 68-69=[68,70), ...
    // Use temps that don't overlap: 64 is in lower-tail only (< 66),
    // 67.0 is in 66-67 only (>=66, <68, and >=67 so NOT < 67 tail).
    let temps = vec![64.0, 67.0, 68.0, 70.0, 72.0, 74.5, 76.0, 78.5, 80.0, 85.0];

    let probs = bucket_probabilities(&buckets, &temps);
    assert_eq!(probs.len(), 9);

    let sum: f64 = probs.iter().sum();
    assert!(
        (sum - 1.0).abs() < 1e-9,
        "probabilities should sum to 1.0, got {sum}"
    );

    // First bucket (65°F or below): 64.0 falls in → 1/10
    assert!((probs[0] - 0.1).abs() < 1e-9);
    // Last bucket (80°F or higher): 80.0, 85.0 → 2/10
    assert!((probs[8] - 0.2).abs() < 1e-9);
}
