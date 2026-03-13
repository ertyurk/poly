#![allow(clippy::unwrap_used)]

use polymarket_bot::db;

#[test]
fn weather_tables_are_created() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();

    let count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='table' AND name IN ('weather_forecasts', 'weather_markets')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "both weather tables should exist");
}

#[test]
fn insert_weather_forecast_roundtrip() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();

    db::queries::insert_weather_forecast(
        &conn, "atlanta", "2026-03-14", "ecmwf_ifs025", 1, 72.5, 1_000_000,
    )
    .unwrap();

    let (city, temp): (String, f64) = conn
        .query_row(
            "SELECT city, temp_max FROM weather_forecasts LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(city, "atlanta");
    assert!((temp - 72.5).abs() < f64::EPSILON);
}

#[test]
fn insert_weather_market_roundtrip() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();

    db::queries::insert_weather_market(
        &conn,
        "evt-123",
        "atlanta",
        "2026-03-14",
        0,
        "65\u{00b0}F or below",
        None,
        Some(65.0),
        "tok-yes-1",
        "tok-no-1",
        Some(0.05),
        Some(0.10),
        Some(0.07),
        Some(0.12),
        Some(0.05),
        1_000_000,
    )
    .unwrap();

    let (event_id, bucket_label): (String, String) = conn
        .query_row(
            "SELECT event_id, bucket_label FROM weather_markets LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(event_id, "evt-123");
    assert!(bucket_label.contains("65"));
}
