#![allow(clippy::unwrap_used)]

use polymarket_bot::weather::types::{is_tail, weather_market_id, Bucket, CityConfig, TempUnit};

// ── Bucket parsing: Fahrenheit ──────────────────────────────────────

#[test]
fn parse_fahrenheit_bounded() {
    let b = Bucket::parse("74-75°F").unwrap();
    assert_eq!(b.lo, Some(74.0));
    assert_eq!(b.hi, Some(75.0));
    assert_eq!(b.unit, TempUnit::Fahrenheit);
    assert_eq!(b.label, "74-75°F");
}

#[test]
fn parse_fahrenheit_lower_tail() {
    let b = Bucket::parse("65°F or below").unwrap();
    assert_eq!(b.lo, None);
    assert_eq!(b.hi, Some(65.0));
    assert_eq!(b.unit, TempUnit::Fahrenheit);
}

#[test]
fn parse_fahrenheit_upper_tail() {
    let b = Bucket::parse("80°F or higher").unwrap();
    assert_eq!(b.lo, Some(80.0));
    assert_eq!(b.hi, None);
    assert_eq!(b.unit, TempUnit::Fahrenheit);
}

// ── Bucket parsing: Celsius ─────────────────────────────────────────

#[test]
fn parse_celsius_bounded() {
    let b = Bucket::parse("10°C").unwrap();
    assert_eq!(b.lo, Some(10.0));
    assert_eq!(b.hi, Some(10.0));
    assert_eq!(b.unit, TempUnit::Celsius);
    assert_eq!(b.label, "10°C");
}

#[test]
fn parse_celsius_lower_tail() {
    let b = Bucket::parse("6°C or below").unwrap();
    assert_eq!(b.lo, None);
    assert_eq!(b.hi, Some(6.0));
    assert_eq!(b.unit, TempUnit::Celsius);
}

#[test]
fn parse_celsius_upper_tail() {
    let b = Bucket::parse("14°C or higher").unwrap();
    assert_eq!(b.lo, Some(14.0));
    assert_eq!(b.hi, None);
    assert_eq!(b.unit, TempUnit::Celsius);
}

#[test]
fn parse_negative_celsius_single() {
    let b = Bucket::parse("-3°C").unwrap();
    assert_eq!(b.lo, Some(-3.0));
    assert_eq!(b.hi, Some(-3.0));
    assert_eq!(b.unit, TempUnit::Celsius);
}

#[test]
fn parse_negative_celsius_lower_tail() {
    let b = Bucket::parse("-4°C or below").unwrap();
    assert_eq!(b.lo, None);
    assert_eq!(b.hi, Some(-4.0));
    assert_eq!(b.unit, TempUnit::Celsius);
}

#[test]
fn parse_invalid_returns_none() {
    assert!(Bucket::parse("garbage").is_none());
    assert!(Bucket::parse("").is_none());
}

// ── Bucket::contains ────────────────────────────────────────────────

#[test]
fn contains_bounded_fahrenheit() {
    let b = Bucket::parse("74-75°F").unwrap();
    // Bounded °F: [lo, lo+2)
    assert!(b.contains(74.0));
    assert!(b.contains(75.5));
    assert!(!b.contains(76.0)); // lo+2 = 76, exclusive
    assert!(!b.contains(73.9));
}

#[test]
fn contains_bounded_celsius() {
    let b = Bucket::parse("10°C").unwrap();
    // Bounded °C: [lo, lo+1)
    assert!(b.contains(10.0));
    assert!(b.contains(10.5));
    assert!(!b.contains(11.0)); // lo+1 = 11, exclusive
    assert!(!b.contains(9.9));
}

#[test]
fn contains_lower_tail_fahrenheit() {
    let b = Bucket::parse("65°F or below").unwrap();
    // Lower tail: (-inf, hi + bucket_width) → (-inf, 67)
    assert!(b.contains(60.0));
    assert!(b.contains(66.9));
    assert!(!b.contains(67.0));
    assert!(b.contains(-100.0));
}

#[test]
fn contains_lower_tail_celsius() {
    let b = Bucket::parse("6°C or below").unwrap();
    // Lower tail: (-inf, hi + bucket_width) → (-inf, 7)
    assert!(b.contains(5.0));
    assert!(b.contains(6.9));
    assert!(!b.contains(7.0));
}

#[test]
fn contains_upper_tail() {
    let b = Bucket::parse("80°F or higher").unwrap();
    // Upper tail: [lo, +inf)
    assert!(b.contains(80.0));
    assert!(b.contains(999.0));
    assert!(!b.contains(79.9));
}

// ── CityConfig ──────────────────────────────────────────────────────

#[test]
fn city_lookup_atlanta() {
    let city = CityConfig::find("atlanta").unwrap();
    assert_eq!(city.name, "atlanta");
    assert_eq!(city.temp_unit, TempUnit::Fahrenheit);
    assert_eq!(city.icao, "KATL");
    assert!((city.lat - 33.749).abs() < 0.01);
    assert!((city.lon - (-84.388)).abs() < 0.01);
}

#[test]
fn city_lookup_london() {
    let city = CityConfig::find("london").unwrap();
    assert_eq!(city.name, "london");
    assert_eq!(city.temp_unit, TempUnit::Celsius);
    assert_eq!(city.icao, "EGLC");
}

#[test]
fn city_lookup_unknown_returns_none() {
    assert!(CityConfig::find("atlantis").is_none());
}

#[test]
fn all_twenty_cities_exist() {
    let names = [
        "atlanta",
        "chicago",
        "miami",
        "nyc",
        "dallas",
        "seattle",
        "london",
        "paris",
        "tokyo",
        "seoul",
        "toronto",
        "shanghai",
        "ankara",
        "tel_aviv",
        "munich",
        "singapore",
        "sao_paulo",
        "buenos_aires",
        "wellington",
        "lucknow",
    ];
    for name in &names {
        assert!(CityConfig::find(name).is_some(), "missing city: {name}");
    }
}

// ── is_tail ─────────────────────────────────────────────────────────

#[test]
fn is_tail_lower_indices() {
    // tail_count=2, total=9 → lower tail: 0,1; upper tail: 7,8
    assert!(is_tail(0, 9, 2));
    assert!(is_tail(1, 9, 2));
}

#[test]
fn is_tail_middle_indices_are_not_tail() {
    assert!(!is_tail(4, 9, 2));
    assert!(!is_tail(5, 9, 2));
}

#[test]
fn is_tail_upper_indices() {
    assert!(is_tail(7, 9, 2));
    assert!(is_tail(8, 9, 2));
}

#[test]
fn is_tail_boundary() {
    // index 2 should NOT be tail with tail_count=2
    assert!(!is_tail(2, 9, 2));
    // index 6 should NOT be tail with tail_count=2, total=9
    assert!(!is_tail(6, 9, 2));
}

// ── weather_market_id ───────────────────────────────────────────────

#[test]
fn market_id_format() {
    let id = weather_market_id("atlanta", "2026-03-15", 3);
    assert_eq!(id, "WX_atlanta_2026-03-15_3");
}
