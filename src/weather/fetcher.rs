use crate::weather::types::{Bucket, CityConfig};

// ── Public structs ────────────────────────────────────────────────

/// A Polymarket weather event with its constituent bucket markets.
#[derive(Debug, Clone)]
pub struct WeatherEvent {
    pub event_id: String,
    pub city: String,
    pub target_date: String,
    pub end_date: String,
    pub buckets: Vec<BucketMarket>,
}

/// One bucket market within a weather event.
#[derive(Debug, Clone)]
pub struct BucketMarket {
    pub market_id: String,
    pub label: String,
    pub bucket: Bucket,
    pub threshold: u8,
    pub token_yes: String,
    pub token_no: String,
    pub best_bid: f64,
    pub best_ask: f64,
    pub midpoint: f64,
}

// ── Title / slug parsers ──────────────────────────────────────────

/// Multi-word city names that map to canonical city keys.
static CITY_ALIASES: &[(&str, &str)] = &[
    ("new york city", "nyc"),
    ("são paulo", "sao_paulo"),
    ("sao paulo", "sao_paulo"),
    ("tel aviv", "tel_aviv"),
    ("buenos aires", "buenos_aires"),
];

/// Parse city name from an event title like
/// `"Highest temperature in Atlanta on March 14?"`.
///
/// Returns the canonical lowercase city key if it exists in CITIES.
pub fn city_from_title(title: &str) -> Option<String> {
    // Find " in " and " on " to extract the city portion.
    let lower = title.to_lowercase();
    let in_pos = lower.find(" in ")?;
    let city_start = in_pos + 4;
    let on_pos = lower[city_start..].find(" on ")?;
    let raw_city = &lower[city_start..city_start + on_pos];
    let raw_city = raw_city.trim();

    // Check multi-word aliases first.
    for &(alias, key) in CITY_ALIASES {
        if raw_city == alias {
            return CityConfig::find(key).map(|c| c.name.to_owned());
        }
    }

    // Single-word: just lowercase.
    let key = raw_city.to_lowercase();
    CityConfig::find(&key).map(|c| c.name.to_owned())
}

/// Month name → number.
fn month_number(month: &str) -> Option<u8> {
    match month {
        "january" => Some(1),
        "february" => Some(2),
        "march" => Some(3),
        "april" => Some(4),
        "may" => Some(5),
        "june" => Some(6),
        "july" => Some(7),
        "august" => Some(8),
        "september" => Some(9),
        "october" => Some(10),
        "november" => Some(11),
        "december" => Some(12),
        _ => None,
    }
}

/// Parse a date from a Polymarket event slug.
///
/// Example: `"highest-temperature-in-atlanta-on-march-14-2026"` → `"2026-03-14"`.
pub fn date_from_slug(slug: &str) -> Option<String> {
    // Find "-on-" then expect month-day-year.
    let on_idx = slug.find("-on-")?;
    let after_on = &slug[on_idx + 4..];
    let parts: Vec<&str> = after_on.split('-').collect();

    if parts.len() < 3 {
        return None;
    }

    let month = month_number(parts[0])?;
    let day: u8 = parts[1].parse().ok()?;
    let year: u16 = parts[2].parse().ok()?;

    Some(format!("{year:04}-{month:02}-{day:02}"))
}

// ── Event parsing ─────────────────────────────────────────────────

/// Parse a stringified JSON array of two token IDs.
///
/// Input: `"[\"tok-yes\",\"tok-no\"]"` → `Some(("tok-yes".into(), "tok-no".into()))`.
fn parse_clob_token_ids(raw: &str) -> Option<(String, String)> {
    let arr: Vec<String> = serde_json::from_str(raw).ok()?;
    if arr.len() >= 2 {
        Some((arr[0].clone(), arr[1].clone()))
    } else {
        None
    }
}

/// Parse a single Polymarket weather event JSON object into a `WeatherEvent`.
pub fn parse_weather_event(json: &serde_json::Value) -> Option<WeatherEvent> {
    let event_id = json.get("id")?.as_str()?.to_owned();
    let title = json.get("title")?.as_str()?;
    let slug = json.get("slug")?.as_str()?;
    let end_date = json.get("endDate")?.as_str()?.to_owned();

    let city = city_from_title(title)?;
    let target_date = date_from_slug(slug)?;

    let markets = json.get("markets")?.as_array()?;

    let mut buckets: Vec<BucketMarket> = Vec::new();

    for (idx, m) in markets.iter().enumerate() {
        let market_id = m.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        let label = m.get("groupItemTitle").and_then(|v| v.as_str())?;
        let bucket = Bucket::parse(label)?;

        let clob_raw = m.get("clobTokenIds").and_then(|v| v.as_str())?;
        let (token_yes, token_no) = parse_clob_token_ids(clob_raw)?;

        let best_bid = m
            .get("bestBid")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let best_ask = m
            .get("bestAsk")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);

        // outcomePrices may be a stringified JSON array like "[0.12, 0.88]"
        let midpoint = m
            .get("outcomePrices")
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str::<Vec<f64>>(s).ok())
            .and_then(|arr| arr.first().copied())
            .unwrap_or_else(|| f64::midpoint(best_bid, best_ask));

        buckets.push(BucketMarket {
            market_id,
            label: label.to_owned(),
            bucket,
            threshold: idx as u8,
            token_yes,
            token_no,
            best_bid,
            best_ask,
            midpoint,
        });
    }

    // Sort by threshold (which we'll reassign after sorting by bucket lo value).
    buckets.sort_by(|a, b| {
        let a_val = a.bucket.lo.unwrap_or(f64::NEG_INFINITY);
        let b_val = b.bucket.lo.unwrap_or(f64::NEG_INFINITY);
        a_val.partial_cmp(&b_val).unwrap_or(std::cmp::Ordering::Equal)
    });

    // Reassign threshold indices after sorting.
    for (i, bm) in buckets.iter_mut().enumerate() {
        bm.threshold = i as u8;
        bm.bucket.index = i as u8;
    }

    if buckets.is_empty() {
        return None;
    }

    Some(WeatherEvent {
        event_id,
        city,
        target_date,
        end_date,
        buckets,
    })
}

/// Fetch weather events from the Gamma Events API.
pub async fn fetch_weather_events(
    http: &reqwest::Client,
    gamma_url: &str,
) -> Result<Vec<WeatherEvent>, Box<dyn std::error::Error + Send + Sync>> {
    let resp = http.get(gamma_url).send().await?;
    let json: serde_json::Value = resp.json().await?;

    let events = json
        .as_array()
        .map(|arr| arr.iter().filter_map(parse_weather_event).collect())
        .unwrap_or_default();

    Ok(events)
}
