#![allow(clippy::unwrap_used)]

use polymarket_bot::weather::fetcher::{city_from_title, date_from_slug, parse_weather_event};

// ── city_from_title ───────────────────────────────────────────────

#[test]
fn city_atlanta() {
    assert_eq!(
        city_from_title("Highest temperature in Atlanta on March 14?").as_deref(),
        Some("atlanta")
    );
}

#[test]
fn city_nyc() {
    assert_eq!(
        city_from_title("Highest temperature in New York City on March 14?").as_deref(),
        Some("nyc")
    );
}

#[test]
fn city_sao_paulo() {
    assert_eq!(
        city_from_title("Highest temperature in São Paulo on March 14?").as_deref(),
        Some("sao_paulo")
    );
}

#[test]
fn city_sao_paulo_ascii() {
    assert_eq!(
        city_from_title("Highest temperature in Sao Paulo on March 14?").as_deref(),
        Some("sao_paulo")
    );
}

#[test]
fn city_tel_aviv() {
    assert_eq!(
        city_from_title("Highest temperature in Tel Aviv on March 14?").as_deref(),
        Some("tel_aviv")
    );
}

#[test]
fn city_buenos_aires() {
    assert_eq!(
        city_from_title("Highest temperature in Buenos Aires on March 14?").as_deref(),
        Some("buenos_aires")
    );
}

#[test]
fn city_unknown() {
    assert!(city_from_title("Highest temperature in Atlantis on March 14?").is_none());
}

// ── date_from_slug ────────────────────────────────────────────────

#[test]
fn date_basic() {
    assert_eq!(
        date_from_slug("highest-temperature-in-atlanta-on-march-14-2026").as_deref(),
        Some("2026-03-14")
    );
}

#[test]
fn date_december() {
    assert_eq!(
        date_from_slug("highest-temperature-in-london-on-december-25-2026").as_deref(),
        Some("2026-12-25")
    );
}

#[test]
fn date_no_on() {
    assert!(date_from_slug("highest-temperature-in-atlanta-march-14-2026").is_none());
}

// ── parse_weather_event ───────────────────────────────────────────

#[test]
fn parse_two_bucket_event() {
    let json: serde_json::Value = serde_json::from_str(
        r#"{
            "id": "evt-123",
            "title": "Highest temperature in Atlanta on March 14?",
            "slug": "highest-temperature-in-atlanta-on-march-14-2026",
            "endDate": "2026-03-15T00:00:00Z",
            "markets": [
                {
                    "id": "mkt-2",
                    "groupItemTitle": "74-75°F",
                    "clobTokenIds": "[\"tok-yes-2\",\"tok-no-2\"]",
                    "bestBid": 0.10,
                    "bestAsk": 0.15,
                    "outcomePrices": "[0.12, 0.88]"
                },
                {
                    "id": "mkt-1",
                    "groupItemTitle": "65°F or below",
                    "clobTokenIds": "[\"tok-yes-1\",\"tok-no-1\"]",
                    "bestBid": 0.05,
                    "bestAsk": 0.10,
                    "outcomePrices": "[0.07, 0.93]"
                }
            ]
        }"#,
    )
    .unwrap();

    let event = parse_weather_event(&json).unwrap();
    assert_eq!(event.event_id, "evt-123");
    assert_eq!(event.city, "atlanta");
    assert_eq!(event.target_date, "2026-03-14");
    assert_eq!(event.end_date, "2026-03-15T00:00:00Z");
    assert_eq!(event.buckets.len(), 2);

    // Buckets should be sorted by threshold (bucket index).
    // "65°F or below" parses as lo=None,hi=65 → lower tail, should come first.
    assert_eq!(event.buckets[0].label, "65°F or below");
    assert_eq!(event.buckets[0].token_yes, "tok-yes-1");
    assert_eq!(event.buckets[0].token_no, "tok-no-1");
    assert!((event.buckets[0].best_bid - 0.05).abs() < f64::EPSILON);

    assert_eq!(event.buckets[1].label, "74-75°F");
    assert_eq!(event.buckets[1].token_yes, "tok-yes-2");
    assert_eq!(event.buckets[1].token_no, "tok-no-2");
}
