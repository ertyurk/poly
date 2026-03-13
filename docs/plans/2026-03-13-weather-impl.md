# Weather Module Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add weather temperature market trading via ECMWF/GFS ensemble counting. Additive only — zero changes to crypto code.

**Architecture:** New `src/weather/` module with fetcher (Gamma API), forecast (Open-Meteo), and signal (bucket counting). Reuses existing DecisionActor and Executor unchanged. Isolated dev DB via `--db-path ./dev_weather.db`.

**Tech Stack:** reqwest (HTTP), serde (JSON), existing rusqlite, tokio channels

**CRITICAL CONSTRAINT:** Crypto code paths are FROZEN. The live bot is running with active bets. Do NOT modify: `src/actors/`, `src/flow.rs`, `src/polymarket/`, `src/types.rs`, `src/math/`. Only touch `src/main.rs` (run_weather stub + run_status), `src/lib.rs` (mod declaration), `src/db/` (new tables via migration), `schema.sql`, `.gitignore`, `src/config.rs` (new Weather section), `Cargo.toml` (no new deps needed — reqwest already present).

---

### Task 1: Weather Types

**Files:**
- Create: `src/weather/types.rs`
- Create: `src/weather/mod.rs`
- Modify: `src/lib.rs:1-13` (add `pub mod weather;`)
- Test: `tests/weather_types_tests.rs`

**Step 1: Write failing test**

```rust
// tests/weather_types_tests.rs
use polymarket_bot::weather::types::*;

#[test]
fn parse_fahrenheit_bucket_bounded() {
    let b = Bucket::parse("74-75°F").unwrap();
    assert_eq!(b.lo, Some(74.0));
    assert_eq!(b.hi, Some(75.0));
    assert_eq!(b.unit, TempUnit::Fahrenheit);
}

#[test]
fn parse_fahrenheit_bucket_lower_tail() {
    let b = Bucket::parse("65°F or below").unwrap();
    assert_eq!(b.lo, None);
    assert_eq!(b.hi, Some(65.0));
}

#[test]
fn parse_fahrenheit_bucket_upper_tail() {
    let b = Bucket::parse("80°F or higher").unwrap();
    assert_eq!(b.lo, Some(80.0));
    assert_eq!(b.hi, None);
}

#[test]
fn parse_celsius_bucket_bounded() {
    let b = Bucket::parse("10°C").unwrap();
    assert_eq!(b.lo, Some(10.0));
    assert_eq!(b.hi, Some(10.0));
}

#[test]
fn parse_celsius_bucket_lower_tail() {
    let b = Bucket::parse("6°C or below").unwrap();
    assert_eq!(b.lo, None);
    assert_eq!(b.hi, Some(6.0));
}

#[test]
fn parse_celsius_bucket_upper_tail() {
    let b = Bucket::parse("14°C or higher").unwrap();
    assert_eq!(b.lo, Some(14.0));
    assert_eq!(b.hi, None);
}

#[test]
fn bucket_contains_temp() {
    let b = Bucket::parse("74-75°F").unwrap();
    assert!(b.contains(74.5));
    assert!(b.contains(74.0));
    assert!(b.contains(75.99)); // 2°F bucket: [74, 76)
    assert!(!b.contains(73.9));
    assert!(!b.contains(76.0));
}

#[test]
fn lower_tail_contains() {
    let b = Bucket::parse("65°F or below").unwrap();
    assert!(b.contains(60.0));
    assert!(b.contains(65.99));
    assert!(!b.contains(66.0));
}

#[test]
fn upper_tail_contains() {
    let b = Bucket::parse("80°F or higher").unwrap();
    assert!(b.contains(80.0));
    assert!(b.contains(99.0));
    assert!(!b.contains(79.9));
}

#[test]
fn city_config_lookup() {
    let city = CityConfig::find("atlanta").unwrap();
    assert!((city.lat - 33.75).abs() < 0.1);
    assert_eq!(city.temp_unit, TempUnit::Fahrenheit);
    assert_eq!(city.icao, "KATL");
}

#[test]
fn city_config_celsius() {
    let city = CityConfig::find("london").unwrap();
    assert_eq!(city.temp_unit, TempUnit::Celsius);
    assert_eq!(city.icao, "EGLC");
}

#[test]
fn is_tail_bucket() {
    // 9 buckets: indices 0,1 are low tail; 7,8 are high tail (with tail_count=2)
    assert!(is_tail(0, 9, 2));
    assert!(is_tail(1, 9, 2));
    assert!(!is_tail(2, 9, 2));
    assert!(!is_tail(6, 9, 2));
    assert!(is_tail(7, 9, 2));
    assert!(is_tail(8, 9, 2));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test weather_types_tests 2>&1 | head -20`
Expected: FAIL — module `weather` not found

**Step 3: Write minimal implementation**

`src/weather/mod.rs`:
```rust
pub mod types;
```

`src/weather/types.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TempUnit {
    Fahrenheit,
    Celsius,
}

#[derive(Debug, Clone)]
pub struct Bucket {
    pub index: u8,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    pub unit: TempUnit,
    pub label: String,
}

impl Bucket {
    /// Parse Polymarket groupItemTitle: "74-75°F", "65°F or below", "14°C or higher", "10°C"
    pub fn parse(label: &str) -> Option<Self> {
        let unit = if label.contains("°F") {
            TempUnit::Fahrenheit
        } else if label.contains("°C") {
            TempUnit::Celsius
        } else {
            return None;
        };
        let suffix = if unit == TempUnit::Fahrenheit { "°F" } else { "°C" };
        let bucket_width = if unit == TempUnit::Fahrenheit { 2.0 } else { 1.0 };

        if label.contains("or below") {
            let num_str = label.split(suffix).next()?.trim();
            let hi: f64 = num_str.parse().ok()?;
            Some(Self { index: 0, lo: None, hi: Some(hi), unit, label: label.to_string() })
        } else if label.contains("or higher") {
            let num_str = label.split(suffix).next()?.trim();
            let lo: f64 = num_str.parse().ok()?;
            Some(Self { index: 0, lo: Some(lo), hi: None, unit, label: label.to_string() })
        } else if label.contains('-') {
            let num_part = label.split(suffix).next()?.trim();
            let mut parts = num_part.split('-');
            let lo: f64 = parts.next()?.parse().ok()?;
            let hi: f64 = parts.next()?.parse().ok()?;
            Some(Self { index: 0, lo: Some(lo), hi: Some(hi), unit, label: label.to_string() })
        } else {
            // Single degree: "10°C" → lo=10, hi=10
            let num_str = label.split(suffix).next()?.trim();
            let val: f64 = num_str.parse().ok()?;
            Some(Self { index: 0, lo: Some(val), hi: Some(val), unit, label: label.to_string() })
        }
    }

    /// Check if a temperature falls in this bucket.
    /// Bounded: [lo, lo + bucket_width). Lower tail: (-inf, hi + bucket_width). Upper tail: [lo, +inf).
    pub fn contains(&self, temp: f64) -> bool {
        let bucket_width = match self.unit {
            TempUnit::Fahrenheit => 2.0,
            TempUnit::Celsius => 1.0,
        };
        match (self.lo, self.hi) {
            (None, Some(hi)) => temp < hi + bucket_width, // lower tail: "65°F or below" means < 66
            (Some(lo), None) => temp >= lo,                // upper tail: "80°F or higher"
            (Some(lo), Some(_)) => temp >= lo && temp < lo + bucket_width,
            (None, None) => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CityConfig {
    pub name: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub temp_unit: TempUnit,
    pub icao: &'static str,
}

impl CityConfig {
    pub fn find(name: &str) -> Option<&'static CityConfig> {
        let lower = name.to_lowercase();
        CITIES.iter().find(|c| c.name == lower)
    }
}

pub fn is_tail(bucket_index: u8, total_buckets: u8, tail_count: u8) -> bool {
    bucket_index < tail_count || bucket_index >= total_buckets - tail_count
}

/// Weather market ID format: WX_{city}_{date}_{bucket_index}
pub fn weather_market_id(city: &str, date: &str, bucket_index: u8) -> String {
    format!("WX_{city}_{date}_{bucket_index}")
}

/// All supported cities with coordinates and weather station codes.
pub static CITIES: &[CityConfig] = &[
    CityConfig { name: "atlanta", lat: 33.749, lon: -84.388, temp_unit: TempUnit::Fahrenheit, icao: "KATL" },
    CityConfig { name: "chicago", lat: 41.878, lon: -87.630, temp_unit: TempUnit::Fahrenheit, icao: "KORD" },
    CityConfig { name: "miami", lat: 25.762, lon: -80.192, temp_unit: TempUnit::Fahrenheit, icao: "KMIA" },
    CityConfig { name: "nyc", lat: 40.713, lon: -74.006, temp_unit: TempUnit::Fahrenheit, icao: "KLGA" },
    CityConfig { name: "dallas", lat: 32.777, lon: -96.797, temp_unit: TempUnit::Fahrenheit, icao: "KDFW" },
    CityConfig { name: "seattle", lat: 47.606, lon: -122.332, temp_unit: TempUnit::Fahrenheit, icao: "KSEA" },
    CityConfig { name: "london", lat: 51.508, lon: -0.076, temp_unit: TempUnit::Celsius, icao: "EGLC" },
    CityConfig { name: "paris", lat: 48.857, lon: 2.352, temp_unit: TempUnit::Celsius, icao: "LFPG" },
    CityConfig { name: "tokyo", lat: 35.676, lon: 139.650, temp_unit: TempUnit::Celsius, icao: "RJTT" },
    CityConfig { name: "seoul", lat: 37.567, lon: 126.978, temp_unit: TempUnit::Celsius, icao: "RKSI" },
    CityConfig { name: "toronto", lat: 43.653, lon: -79.383, temp_unit: TempUnit::Celsius, icao: "CYYZ" },
    CityConfig { name: "shanghai", lat: 31.230, lon: 121.474, temp_unit: TempUnit::Celsius, icao: "ZSSS" },
    CityConfig { name: "ankara", lat: 39.934, lon: 32.860, temp_unit: TempUnit::Celsius, icao: "LTAC" },
    CityConfig { name: "tel_aviv", lat: 32.084, lon: 34.782, temp_unit: TempUnit::Celsius, icao: "LLBG" },
    CityConfig { name: "munich", lat: 48.137, lon: 11.576, temp_unit: TempUnit::Celsius, icao: "EDDM" },
    CityConfig { name: "singapore", lat: 1.352, lon: 103.820, temp_unit: TempUnit::Celsius, icao: "WSSS" },
    CityConfig { name: "sao_paulo", lat: -23.550, lon: -46.633, temp_unit: TempUnit::Celsius, icao: "SBGR" },
    CityConfig { name: "buenos_aires", lat: -34.604, lon: -58.382, temp_unit: TempUnit::Celsius, icao: "SAEZ" },
    CityConfig { name: "wellington", lat: -41.287, lon: 174.776, temp_unit: TempUnit::Celsius, icao: "NZWN" },
    CityConfig { name: "lucknow", lat: 26.850, lon: 80.950, temp_unit: TempUnit::Celsius, icao: "VILK" },
];
```

Add to `src/lib.rs`:
```rust
pub mod weather;
```

**Step 4: Run test to verify it passes**

Run: `cargo test --test weather_types_tests 2>&1`
Expected: all 12 tests PASS

**Step 5: Verify crypto tests untouched**

Run: `cargo test 2>&1 | tail -30`
Expected: all existing tests still PASS

**Step 6: Commit**

```bash
git add src/weather/ src/lib.rs tests/weather_types_tests.rs
git commit -m "feat(weather): add types — bucket parsing, city config, tail detection"
```

---

### Task 2: Open-Meteo Forecast Client

**Files:**
- Create: `src/weather/forecast.rs`
- Modify: `src/weather/mod.rs` (add `pub mod forecast;`)
- Test: `tests/weather_forecast_tests.rs`

**Step 1: Write failing test**

```rust
// tests/weather_forecast_tests.rs
use polymarket_bot::weather::forecast::*;
use polymarket_bot::weather::types::*;

#[test]
fn parse_ensemble_response_ecmwf() {
    // Simulated Open-Meteo response with 3 members (real has 50)
    let json = serde_json::json!({
        "latitude": 33.75,
        "longitude": -84.5,
        "daily": {
            "time": ["2026-03-14"],
            "temperature_2m_max_member01": [76.2],
            "temperature_2m_max_member02": [74.8],
            "temperature_2m_max_member03": [81.3]
        }
    });
    let temps = parse_ensemble_temps(&json).unwrap();
    assert_eq!(temps.len(), 3);
    assert!((temps[0] - 76.2).abs() < 0.01);
    assert!((temps[1] - 74.8).abs() < 0.01);
    assert!((temps[2] - 81.3).abs() < 0.01);
}

#[test]
fn compute_bucket_probs_from_ensemble() {
    // 10 ensemble members, 9 buckets for Atlanta (°F, 2° width)
    // Buckets: ≤65, 66-67, 68-69, 70-71, 72-73, 74-75, 76-77, 78-79, ≥80
    let buckets = vec![
        Bucket::parse("65°F or below").unwrap(),
        Bucket::parse("66-67°F").unwrap(),
        Bucket::parse("68-69°F").unwrap(),
        Bucket::parse("70-71°F").unwrap(),
        Bucket::parse("72-73°F").unwrap(),
        Bucket::parse("74-75°F").unwrap(),
        Bucket::parse("76-77°F").unwrap(),
        Bucket::parse("78-79°F").unwrap(),
        Bucket::parse("80°F or higher").unwrap(),
    ];
    // Temps: 2 in ≤65, 1 in 74-75, 5 in 76-77, 2 in ≥80
    let temps = vec![60.0, 64.5, 74.2, 76.0, 76.5, 77.1, 76.8, 77.5, 80.0, 85.0];
    let probs = bucket_probabilities(&buckets, &temps);
    assert_eq!(probs.len(), 9);
    assert!((probs[0] - 0.2).abs() < 0.01);   // 2/10
    assert!((probs[5] - 0.1).abs() < 0.01);   // 1/10
    assert!((probs[6] - 0.5).abs() < 0.01);   // 5/10
    assert!((probs[8] - 0.2).abs() < 0.01);   // 2/10
    // Sum should be 1.0
    let sum: f64 = probs.iter().sum();
    assert!((sum - 1.0).abs() < 0.01);
}

#[test]
fn build_forecast_url() {
    let city = CityConfig::find("atlanta").unwrap();
    let url = build_open_meteo_url(city, "2026-03-14");
    assert!(url.contains("latitude=33.749"));
    assert!(url.contains("longitude=-84.388"));
    assert!(url.contains("temperature_unit=fahrenheit"));
    assert!(url.contains("temperature_2m_max"));
    assert!(url.contains("ecmwf_ifs025"));
    assert!(url.contains("start_date=2026-03-14"));
}

#[test]
fn build_forecast_url_celsius() {
    let city = CityConfig::find("london").unwrap();
    let url = build_open_meteo_url(city, "2026-03-14");
    assert!(!url.contains("temperature_unit=fahrenheit"));
    assert!(url.contains("latitude=51.508"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test weather_forecast_tests 2>&1 | head -10`
Expected: FAIL — module `forecast` not found

**Step 3: Write minimal implementation**

`src/weather/forecast.rs`:
```rust
use super::types::*;

/// Build Open-Meteo ensemble API URL for a city and target date.
pub fn build_open_meteo_url(city: &CityConfig, date: &str) -> String {
    let mut url = format!(
        "https://ensemble-api.open-meteo.com/v1/ensemble\
         ?latitude={}&longitude={}\
         &daily=temperature_2m_max\
         &models=ecmwf_ifs025,gfs025_ensemble\
         &start_date={date}&end_date={date}\
         &timezone=auto",
        city.lat, city.lon,
    );
    if city.temp_unit == TempUnit::Fahrenheit {
        url.push_str("&temperature_unit=fahrenheit");
    }
    url
}

/// Parse ensemble member temperatures from Open-Meteo JSON response.
/// Looks for keys matching `temperature_2m_max_memberNN`.
pub fn parse_ensemble_temps(json: &serde_json::Value) -> Option<Vec<f64>> {
    let daily = json.get("daily")?;
    let mut temps = Vec::new();
    for i in 1..=100 {
        let key = format!("temperature_2m_max_member{i:02}");
        if let Some(arr) = daily.get(&key) {
            if let Some(val) = arr.get(0).and_then(|v| v.as_f64()) {
                temps.push(val);
            }
        }
    }
    if temps.is_empty() { None } else { Some(temps) }
}

/// Count fraction of ensemble members in each bucket.
/// Returns Vec<f64> of same length as buckets, summing to 1.0.
pub fn bucket_probabilities(buckets: &[Bucket], temps: &[f64]) -> Vec<f64> {
    let n = temps.len() as f64;
    if n == 0.0 {
        return vec![0.0; buckets.len()];
    }
    buckets.iter().map(|b| {
        let count = temps.iter().filter(|&&t| b.contains(t)).count() as f64;
        count / n
    }).collect()
}

/// Fetch ensemble forecast for a city and date. Returns member temperatures.
pub async fn fetch_ensemble(
    http: &reqwest::Client,
    city: &CityConfig,
    date: &str,
) -> Result<Vec<f64>, Box<dyn std::error::Error + Send + Sync>> {
    let url = build_open_meteo_url(city, date);
    let resp: serde_json::Value = http.get(&url).send().await?.json().await?;
    parse_ensemble_temps(&resp)
        .ok_or_else(|| format!("no ensemble data for {} on {date}", city.name).into())
}
```

Update `src/weather/mod.rs`:
```rust
pub mod forecast;
pub mod types;
```

**Step 4: Run test to verify it passes**

Run: `cargo test --test weather_forecast_tests 2>&1`
Expected: all 4 tests PASS

**Step 5: Verify all tests**

Run: `cargo test 2>&1 | tail -5`
Expected: all tests PASS

**Step 6: Commit**

```bash
git add src/weather/forecast.rs src/weather/mod.rs tests/weather_forecast_tests.rs
git commit -m "feat(weather): add Open-Meteo ensemble client and bucket probability math"
```

---

### Task 3: Weather Signal — Edge Detection

**Files:**
- Create: `src/weather/signal.rs`
- Modify: `src/weather/mod.rs` (add `pub mod signal;`)
- Test: `tests/weather_signal_tests.rs`

**Step 1: Write failing test**

```rust
// tests/weather_signal_tests.rs
use polymarket_bot::weather::signal::*;
use polymarket_bot::weather::types::*;

#[test]
fn find_tail_edges_basic() {
    // 9 buckets, market prices, ensemble probabilities
    let market_prices = vec![0.01, 0.02, 0.10, 0.15, 0.25, 0.30, 0.10, 0.05, 0.02];
    let ensemble_probs = vec![0.05, 0.03, 0.08, 0.12, 0.20, 0.30, 0.12, 0.06, 0.04];
    // tail_count=2, max_tail_price=0.10, edge_threshold=0.02
    let edges = find_tail_edges(&market_prices, &ensemble_probs, 2, 0.10, 0.02);

    // Bucket 0: market=0.01, ensemble=0.05, edge=0.04 → YES (tail, cheap, edge > 0.02)
    // Bucket 1: market=0.02, ensemble=0.03, edge=0.01 → skip (edge < 0.02)
    // Bucket 7: market=0.05, ensemble=0.06, edge=0.01 → skip (edge < 0.02)
    // Bucket 8: market=0.02, ensemble=0.04, edge=0.02 → skip (edge not > 0.02, only >=)
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].bucket_index, 0);
    assert!((edges[0].p_ensemble - 0.05).abs() < 0.001);
    assert!((edges[0].edge - 0.04).abs() < 0.001);
}

#[test]
fn find_tail_edges_no_cheap_buckets() {
    let market_prices = vec![0.15, 0.15, 0.10, 0.10, 0.10, 0.10, 0.10, 0.10, 0.10];
    let ensemble_probs = vec![0.20, 0.15, 0.10, 0.10, 0.10, 0.10, 0.10, 0.10, 0.05];
    // All tail buckets priced > 0.10 → none qualify
    let edges = find_tail_edges(&market_prices, &ensemble_probs, 2, 0.10, 0.02);
    assert!(edges.is_empty());
}

#[test]
fn find_tail_edges_multiple() {
    let market_prices = vec![0.01, 0.01, 0.10, 0.10, 0.50, 0.10, 0.10, 0.01, 0.01];
    let ensemble_probs = vec![0.06, 0.05, 0.08, 0.10, 0.30, 0.15, 0.12, 0.08, 0.06];
    let edges = find_tail_edges(&market_prices, &ensemble_probs, 2, 0.10, 0.03);
    // Bucket 0: edge=0.05 ✓, Bucket 1: edge=0.04 ✓, Bucket 7: edge=0.07 ✓, Bucket 8: edge=0.05 ✓
    assert_eq!(edges.len(), 4);
}

#[test]
fn tail_edge_to_signal_fields() {
    let edge = TailEdge {
        bucket_index: 8,
        p_ensemble: 0.05,
        market_price: 0.02,
        edge: 0.03,
    };
    // p_hat for YES bet: p_ensemble
    // confidence: edge / market_price (relative edge)
    assert!((edge.p_hat() - 0.05).abs() < 0.001);
    assert!((edge.relative_edge() - 1.5).abs() < 0.01); // 0.03 / 0.02
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test weather_signal_tests 2>&1 | head -10`
Expected: FAIL — module `signal` not found

**Step 3: Write minimal implementation**

`src/weather/signal.rs`:
```rust
/// A detected edge on a tail bucket.
#[derive(Debug, Clone)]
pub struct TailEdge {
    pub bucket_index: u8,
    pub p_ensemble: f64,
    pub market_price: f64,
    pub edge: f64,
}

impl TailEdge {
    /// Our model probability (used as p_hat for YES side).
    pub fn p_hat(&self) -> f64 {
        self.p_ensemble
    }

    /// Relative edge: edge / market_price. Higher = stronger signal.
    pub fn relative_edge(&self) -> f64 {
        if self.market_price > 0.0 {
            self.edge / self.market_price
        } else {
            0.0
        }
    }
}

/// Scan tail buckets for edges worth trading.
///
/// - `market_prices`: Vec of 9 market YES prices (one per bucket)
/// - `ensemble_probs`: Vec of 9 ensemble probabilities (from bucket_probabilities)
/// - `tail_count`: how many buckets from each end count as "tail" (typically 2-3)
/// - `max_tail_price`: only trade buckets priced below this (e.g. 0.10)
/// - `edge_threshold`: minimum edge to consider (e.g. 0.03)
pub fn find_tail_edges(
    market_prices: &[f64],
    ensemble_probs: &[f64],
    tail_count: u8,
    max_tail_price: f64,
    edge_threshold: f64,
) -> Vec<TailEdge> {
    let n = market_prices.len();
    let mut edges = Vec::new();

    for i in 0..n {
        // Only tail buckets
        if !super::types::is_tail(i as u8, n as u8, tail_count) {
            continue;
        }
        let mp = market_prices[i];
        let ep = ensemble_probs[i];

        // Only cheap buckets (tail pricing)
        if mp > max_tail_price {
            continue;
        }

        let edge = ep - mp;
        if edge > edge_threshold {
            edges.push(TailEdge {
                bucket_index: i as u8,
                p_ensemble: ep,
                market_price: mp,
                edge,
            });
        }
    }

    edges
}
```

Update `src/weather/mod.rs`:
```rust
pub mod forecast;
pub mod signal;
pub mod types;
```

**Step 4: Run test to verify it passes**

Run: `cargo test --test weather_signal_tests 2>&1`
Expected: all 4 tests PASS

**Step 5: Commit**

```bash
git add src/weather/signal.rs src/weather/mod.rs tests/weather_signal_tests.rs
git commit -m "feat(weather): add tail edge detection — find underpriced tail buckets"
```

---

### Task 4: Weather Fetcher — Gamma API Discovery

**Files:**
- Create: `src/weather/fetcher.rs`
- Modify: `src/weather/mod.rs` (add `pub mod fetcher;`)
- Test: `tests/weather_fetcher_tests.rs`

**Step 1: Write failing test**

```rust
// tests/weather_fetcher_tests.rs
use polymarket_bot::weather::fetcher::*;

#[test]
fn parse_weather_event_json() {
    let json = serde_json::json!({
        "id": "262796",
        "slug": "highest-temperature-in-atlanta-on-march-14-2026",
        "title": "Highest temperature in Atlanta on March 14?",
        "negRisk": true,
        "endDate": "2026-03-14T12:00:00Z",
        "markets": [
            {
                "id": "1566200",
                "question": "Will the highest temperature in Atlanta be 65°F or below on March 14?",
                "groupItemTitle": "65°F or below",
                "groupItemThreshold": "0",
                "outcomes": "[\"Yes\", \"No\"]",
                "outcomePrices": "[\"0.01\", \"0.99\"]",
                "clobTokenIds": "[\"111\", \"222\"]",
                "bestBid": 0.01,
                "bestAsk": 0.02,
                "active": true,
                "closed": false
            },
            {
                "id": "1566201",
                "question": "Will the highest temperature in Atlanta be between 74-75°F on March 14?",
                "groupItemTitle": "74-75°F",
                "groupItemThreshold": "5",
                "outcomes": "[\"Yes\", \"No\"]",
                "outcomePrices": "[\"0.30\", \"0.70\"]",
                "clobTokenIds": "[\"333\", \"444\"]",
                "bestBid": 0.29,
                "bestAsk": 0.31,
                "active": true,
                "closed": false
            }
        ]
    });
    let event = parse_weather_event(&json).unwrap();
    assert_eq!(event.city, "atlanta");
    assert_eq!(event.target_date, "2026-03-14");
    assert_eq!(event.buckets.len(), 2);
    assert_eq!(event.buckets[0].label, "65°F or below");
    assert_eq!(event.buckets[0].token_yes, "111");
    assert!((event.buckets[0].best_ask - 0.02).abs() < 0.001);
    assert_eq!(event.buckets[1].label, "74-75°F");
}

#[test]
fn extract_city_from_title() {
    assert_eq!(city_from_title("Highest temperature in Atlanta on March 14?"), Some("atlanta".to_string()));
    assert_eq!(city_from_title("Highest temperature in New York City on March 14?"), Some("nyc".to_string()));
    assert_eq!(city_from_title("Highest temperature in São Paulo on March 14?"), Some("sao_paulo".to_string()));
    assert_eq!(city_from_title("Highest temperature in Tel Aviv on March 14?"), Some("tel_aviv".to_string()));
    assert_eq!(city_from_title("Highest temperature in Buenos Aires on March 14?"), Some("buenos_aires".to_string()));
    assert_eq!(city_from_title("Something unrelated"), None);
}

#[test]
fn extract_date_from_slug() {
    assert_eq!(
        date_from_slug("highest-temperature-in-atlanta-on-march-14-2026"),
        Some("2026-03-14".to_string())
    );
    assert_eq!(
        date_from_slug("highest-temperature-in-new-york-city-on-march-15-2026"),
        Some("2026-03-15".to_string())
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test weather_fetcher_tests 2>&1 | head -10`
Expected: FAIL — module `fetcher` not found

**Step 3: Write minimal implementation**

`src/weather/fetcher.rs`:
```rust
use super::types::*;

/// Parsed weather event with all bucket markets.
#[derive(Debug, Clone)]
pub struct WeatherEvent {
    pub event_id: String,
    pub city: String,
    pub target_date: String,
    pub end_date: String,
    pub buckets: Vec<BucketMarket>,
}

/// A single bucket market within a weather event.
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

/// Extract city name from event title.
/// "Highest temperature in Atlanta on March 14?" → "atlanta"
pub fn city_from_title(title: &str) -> Option<String> {
    let prefix = "Highest temperature in ";
    if !title.starts_with(prefix) {
        return None;
    }
    let rest = &title[prefix.len()..];
    let city_part = rest.split(" on ").next()?;

    // Map known multi-word/special names
    let normalized = match city_part {
        "New York City" => "nyc",
        "São Paulo" | "Sao Paulo" => "sao_paulo",
        "Tel Aviv" => "tel_aviv",
        "Buenos Aires" => "buenos_aires",
        _ => {
            // Single-word city: lowercase
            let lower = city_part.to_lowercase();
            // Verify it's a known city
            if CityConfig::find(&lower).is_some() {
                return Some(lower);
            }
            return None;
        }
    };
    Some(normalized.to_string())
}

/// Extract target date from event slug or endDate.
/// "highest-temperature-in-atlanta-on-march-14-2026" → "2026-03-14"
pub fn date_from_slug(slug: &str) -> Option<String> {
    // Find "-on-" then parse "month-day-year"
    let on_idx = slug.find("-on-")?;
    let date_part = &slug[on_idx + 4..];
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() < 3 {
        return None;
    }
    let month = match parts[0] {
        "january" => "01", "february" => "02", "march" => "03",
        "april" => "04", "may" => "05", "june" => "06",
        "july" => "07", "august" => "08", "september" => "09",
        "october" => "10", "november" => "11", "december" => "12",
        _ => return None,
    };
    let day: u8 = parts[1].parse().ok()?;
    let year = parts[2];
    Some(format!("{year}-{month}-{day:02}"))
}

/// Parse a weather event JSON from Gamma API into our internal type.
pub fn parse_weather_event(json: &serde_json::Value) -> Option<WeatherEvent> {
    let title = json.get("title")?.as_str()?;
    let city = city_from_title(title)?;
    let slug = json.get("slug")?.as_str().unwrap_or("");
    let end_date_str = json.get("endDate")?.as_str().unwrap_or("");

    let target_date = date_from_slug(slug)
        .or_else(|| {
            // Fallback: parse endDate ISO string
            end_date_str.get(..10).map(String::from)
        })?;

    let markets = json.get("markets")?.as_array()?;
    let mut buckets = Vec::new();

    for m in markets {
        let label = m.get("groupItemTitle")?.as_str()?;
        let bucket = Bucket::parse(label)?;
        let threshold: u8 = m.get("groupItemThreshold")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // Parse clobTokenIds: stringified JSON array
        let token_ids_str = m.get("clobTokenIds")?.as_str()?;
        let token_ids: Vec<String> = serde_json::from_str(token_ids_str).ok()?;
        if token_ids.len() < 2 { continue; }

        let best_bid = m.get("bestBid").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let best_ask = m.get("bestAsk").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let mid = (best_bid + best_ask) / 2.0;

        let market_id_raw = m.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");

        buckets.push(BucketMarket {
            market_id: market_id_raw.to_string(),
            label: label.to_string(),
            bucket: Bucket { index: threshold, ..bucket },
            threshold,
            token_yes: token_ids[0].clone(),
            token_no: token_ids[1].clone(),
            best_bid,
            best_ask,
            midpoint: mid,
        });
    }

    buckets.sort_by_key(|b| b.threshold);

    Some(WeatherEvent {
        event_id: json.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        city,
        target_date,
        end_date: end_date_str.to_string(),
        buckets,
    })
}

/// Fetch weather events from Gamma API.
pub async fn fetch_weather_events(
    http: &reqwest::Client,
    gamma_url: &str,
) -> Result<Vec<WeatherEvent>, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!(
        "{}/events?tag=weather&active=true&closed=false&limit=100",
        gamma_url.trim_end_matches('/')
    );
    let resp: Vec<serde_json::Value> = http.get(&url).send().await?.json().await?;
    let events: Vec<WeatherEvent> = resp.iter().filter_map(parse_weather_event).collect();
    Ok(events)
}
```

Update `src/weather/mod.rs`:
```rust
pub mod fetcher;
pub mod forecast;
pub mod signal;
pub mod types;
```

**Step 4: Run test to verify it passes**

Run: `cargo test --test weather_fetcher_tests 2>&1`
Expected: all 3 tests PASS

**Step 5: Commit**

```bash
git add src/weather/fetcher.rs src/weather/mod.rs tests/weather_fetcher_tests.rs
git commit -m "feat(weather): add Gamma API weather event discovery and parsing"
```

---

### Task 5: Weather Config + DB Migration

**Files:**
- Modify: `src/config.rs` (add `Weather` struct)
- Modify: `src/db/schema.rs` (add weather table migration)
- Modify: `src/db/queries.rs` (add weather insert functions)
- Modify: `src/paths.rs` (add weather config to default template)
- Modify: `.gitignore` (add `dev_weather.db`)
- Test: `tests/weather_db_tests.rs`

**Step 1: Write failing test**

```rust
// tests/weather_db_tests.rs
use polymarket_bot::db::{queries, schema};

#[test]
fn weather_tables_created() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    schema::create_tables(&conn).unwrap();

    // Verify weather_forecasts table exists
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM weather_forecasts", [], |row| row.get(0)
    ).unwrap();
    assert_eq!(count, 0);

    // Verify weather_markets table exists
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM weather_markets", [], |row| row.get(0)
    ).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn insert_weather_forecast() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    schema::create_tables(&conn).unwrap();

    queries::insert_weather_forecast(
        &conn, "atlanta", "2026-03-14", "ecmwf", 1, 76.2, 1710000000,
    ).unwrap();

    let count: i64 = conn.query_row(
        "SELECT count(*) FROM weather_forecasts WHERE city='atlanta'",
        [], |row| row.get(0)
    ).unwrap();
    assert_eq!(count, 1);
}

#[test]
fn insert_weather_market() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    schema::create_tables(&conn).unwrap();

    queries::insert_weather_market(
        &conn, "evt_123", "atlanta", "2026-03-14",
        0, "65°F or below", None, Some(65.0),
        "token_yes", "token_no",
        0.01, 0.02, 0.015, 0.05, 0.04, 1710000000,
    ).unwrap();

    let (city, edge): (String, f64) = conn.query_row(
        "SELECT city, edge FROM weather_markets WHERE event_id='evt_123'",
        [], |row| Ok((row.get(0)?, row.get(1)?))
    ).unwrap();
    assert_eq!(city, "atlanta");
    assert!((edge - 0.04).abs() < 0.001);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test weather_db_tests 2>&1 | head -10`
Expected: FAIL — `weather_forecasts` table not found

**Step 3: Write minimal implementation**

Add to `src/config.rs` after the `Execution` struct and its impl:
```rust
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Weather {
    #[serde(default = "default_weather_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_max_forecast_horizon")]
    pub max_forecast_horizon_hours: u64,
    #[serde(default = "default_edge_threshold")]
    pub edge_threshold: f64,
    #[serde(default = "default_max_tail_price")]
    pub max_tail_price: f64,
    #[serde(default = "default_tail_buckets")]
    pub tail_buckets: u8,
}

fn default_weather_poll_interval() -> u64 { 1800 }
fn default_max_forecast_horizon() -> u64 { 36 }
fn default_edge_threshold() -> f64 { 0.03 }
fn default_max_tail_price() -> f64 { 0.10 }
fn default_tail_buckets() -> u8 { 3 }

impl Default for Weather {
    fn default() -> Self {
        Self {
            poll_interval_secs: default_weather_poll_interval(),
            max_forecast_horizon_hours: default_max_forecast_horizon(),
            edge_threshold: default_edge_threshold(),
            max_tail_price: default_max_tail_price(),
            tail_buckets: default_tail_buckets(),
        }
    }
}
```

Add `weather` field to `Config` struct:
```rust
#[serde(default)]
pub weather: Weather,
```

Add weather migration to `src/db/schema.rs` `migrate()`:
```rust
    // Migration: create weather_forecasts table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS weather_forecasts (
            id INTEGER PRIMARY KEY,
            city TEXT NOT NULL,
            target_date TEXT NOT NULL,
            model TEXT NOT NULL,
            member INTEGER NOT NULL,
            temp_max REAL NOT NULL,
            fetched_ts INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_wf_city_date ON weather_forecasts(city, target_date);",
    )?;

    // Migration: create weather_markets table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS weather_markets (
            id INTEGER PRIMARY KEY,
            event_id TEXT NOT NULL,
            city TEXT NOT NULL,
            target_date TEXT NOT NULL,
            bucket_index INTEGER NOT NULL,
            bucket_label TEXT NOT NULL,
            bucket_lo REAL,
            bucket_hi REAL,
            token_yes TEXT NOT NULL,
            token_no TEXT NOT NULL,
            best_bid REAL,
            best_ask REAL,
            midpoint REAL,
            p_ensemble REAL,
            edge REAL,
            ts INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_wm_city_date ON weather_markets(city, target_date);",
    )?;
```

Add to `src/db/queries.rs`:
```rust
pub fn insert_weather_forecast(
    conn: &Connection,
    city: &str,
    target_date: &str,
    model: &str,
    member: i32,
    temp_max: f64,
    fetched_ts: TsMicros,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO weather_forecasts (city, target_date, model, member, temp_max, fetched_ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![city, target_date, model, member, temp_max, fetched_ts],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn insert_weather_market(
    conn: &Connection,
    event_id: &str,
    city: &str,
    target_date: &str,
    bucket_index: u8,
    bucket_label: &str,
    bucket_lo: Option<f64>,
    bucket_hi: Option<f64>,
    token_yes: &str,
    token_no: &str,
    best_bid: f64,
    best_ask: f64,
    midpoint: f64,
    p_ensemble: f64,
    edge: f64,
    ts: TsMicros,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO weather_markets (event_id, city, target_date, bucket_index, bucket_label,
         bucket_lo, bucket_hi, token_yes, token_no, best_bid, best_ask, midpoint, p_ensemble, edge, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![event_id, city, target_date, bucket_index, bucket_label,
                bucket_lo, bucket_hi, token_yes, token_no,
                best_bid, best_ask, midpoint, p_ensemble, edge, ts],
    )?;
    Ok(())
}
```

Add `dev_weather.db` to `.gitignore`.

Add `[weather]` section to default config template in `src/paths.rs` (append before the closing `"#`).

**Step 4: Run test to verify it passes**

Run: `cargo test --test weather_db_tests 2>&1`
Expected: all 3 tests PASS

**Step 5: Verify all tests still pass**

Run: `cargo test 2>&1 | tail -5`
Expected: ALL tests PASS (including all existing crypto tests)

**Step 6: Commit**

```bash
git add src/config.rs src/db/schema.rs src/db/queries.rs src/paths.rs .gitignore tests/weather_db_tests.rs
git commit -m "feat(weather): add config, DB tables, and query functions"
```

---

### Task 6: Wire run_weather() + Status Extension

**Files:**
- Modify: `src/main.rs:345-365` (fill `run_weather()` stub)
- Modify: `src/main.rs:85-175` (extend `run_status()` with weather section)
- Test: `tests/weather_integration_test.rs`

**Step 1: Write failing integration test**

```rust
// tests/weather_integration_test.rs
use polymarket_bot::weather::fetcher::*;
use polymarket_bot::weather::forecast::*;
use polymarket_bot::weather::signal::*;
use polymarket_bot::weather::types::*;

/// End-to-end: parse event → compute ensemble probs → find edges.
#[test]
fn full_weather_pipeline_mock() {
    // 1. Parse a weather event
    let event_json = serde_json::json!({
        "id": "100",
        "slug": "highest-temperature-in-atlanta-on-march-14-2026",
        "title": "Highest temperature in Atlanta on March 14?",
        "negRisk": true,
        "endDate": "2026-03-14T12:00:00Z",
        "markets": [
            { "id": "1", "groupItemTitle": "65°F or below", "groupItemThreshold": "0",
              "outcomes": "[\"Yes\",\"No\"]", "outcomePrices": "[\"0.01\",\"0.99\"]",
              "clobTokenIds": "[\"t1\",\"t2\"]", "bestBid": 0.01, "bestAsk": 0.02, "active": true, "closed": false },
            { "id": "2", "groupItemTitle": "66-67°F", "groupItemThreshold": "1",
              "outcomes": "[\"Yes\",\"No\"]", "outcomePrices": "[\"0.02\",\"0.98\"]",
              "clobTokenIds": "[\"t3\",\"t4\"]", "bestBid": 0.01, "bestAsk": 0.03, "active": true, "closed": false },
            { "id": "3", "groupItemTitle": "68-69°F", "groupItemThreshold": "2",
              "outcomes": "[\"Yes\",\"No\"]", "outcomePrices": "[\"0.05\",\"0.95\"]",
              "clobTokenIds": "[\"t5\",\"t6\"]", "bestBid": 0.04, "bestAsk": 0.06, "active": true, "closed": false },
            { "id": "4", "groupItemTitle": "70-71°F", "groupItemThreshold": "3",
              "outcomes": "[\"Yes\",\"No\"]", "outcomePrices": "[\"0.10\",\"0.90\"]",
              "clobTokenIds": "[\"t7\",\"t8\"]", "bestBid": 0.09, "bestAsk": 0.11, "active": true, "closed": false },
            { "id": "5", "groupItemTitle": "72-73°F", "groupItemThreshold": "4",
              "outcomes": "[\"Yes\",\"No\"]", "outcomePrices": "[\"0.20\",\"0.80\"]",
              "clobTokenIds": "[\"t9\",\"t10\"]", "bestBid": 0.19, "bestAsk": 0.21, "active": true, "closed": false },
            { "id": "6", "groupItemTitle": "74-75°F", "groupItemThreshold": "5",
              "outcomes": "[\"Yes\",\"No\"]", "outcomePrices": "[\"0.30\",\"0.70\"]",
              "clobTokenIds": "[\"t11\",\"t12\"]", "bestBid": 0.29, "bestAsk": 0.31, "active": true, "closed": false },
            { "id": "7", "groupItemTitle": "76-77°F", "groupItemThreshold": "6",
              "outcomes": "[\"Yes\",\"No\"]", "outcomePrices": "[\"0.20\",\"0.80\"]",
              "clobTokenIds": "[\"t13\",\"t14\"]", "bestBid": 0.19, "bestAsk": 0.21, "active": true, "closed": false },
            { "id": "8", "groupItemTitle": "78-79°F", "groupItemThreshold": "7",
              "outcomes": "[\"Yes\",\"No\"]", "outcomePrices": "[\"0.08\",\"0.92\"]",
              "clobTokenIds": "[\"t15\",\"t16\"]", "bestBid": 0.07, "bestAsk": 0.09, "active": true, "closed": false },
            { "id": "9", "groupItemTitle": "80°F or higher", "groupItemThreshold": "8",
              "outcomes": "[\"Yes\",\"No\"]", "outcomePrices": "[\"0.02\",\"0.98\"]",
              "clobTokenIds": "[\"t17\",\"t18\"]", "bestBid": 0.01, "bestAsk": 0.03, "active": true, "closed": false }
        ]
    });

    let event = parse_weather_event(&event_json).unwrap();
    assert_eq!(event.city, "atlanta");
    assert_eq!(event.buckets.len(), 9);

    // 2. Simulate ensemble: 20 members, most land in 74-77°F, some tails
    let ensemble_temps = vec![
        62.0, 64.0,     // bucket 0 (≤65): 2
        67.0,           // bucket 1 (66-67): 1
        70.5,           // bucket 3 (70-71): 1
        72.0, 73.5,     // bucket 4 (72-73): 2
        74.0, 75.0, 74.5, 75.5, // bucket 5 (74-75): 4
        76.0, 76.5, 77.0, 76.2, 77.5, // bucket 6 (76-77): 5
        78.0, 78.5,     // bucket 7 (78-79): 2
        80.5, 82.0, 81.0, // bucket 8 (≥80): 3
    ];
    let buckets_parsed: Vec<_> = event.buckets.iter().map(|bm| bm.bucket.clone()).collect();
    let probs = bucket_probabilities(&buckets_parsed, &ensemble_temps);

    // Verify probabilities sum to 1
    let sum: f64 = probs.iter().sum();
    assert!((sum - 1.0).abs() < 0.01, "probs sum to {sum}");

    // 3. Find tail edges
    let market_prices: Vec<f64> = event.buckets.iter().map(|b| b.best_ask).collect();
    let edges = find_tail_edges(&market_prices, &probs, 3, 0.10, 0.03);

    // Bucket 0: market=0.02, ensemble=2/20=0.10, edge=0.08 → YES ✓
    // Bucket 1: market=0.03, ensemble=1/20=0.05, edge=0.02 → skip (< 0.03)
    // Bucket 2: market=0.06, ensemble=0/20=0.00, edge=-0.06 → skip
    // Bucket 7: market=0.09, ensemble=2/20=0.10, edge=0.01 → skip (< 0.03)
    // Bucket 8: market=0.03, ensemble=3/20=0.15, edge=0.12 → YES ✓
    // tail_count=3, so buckets 0,1,2 and 6,7,8 are tails
    // Bucket 6: market=0.21 > 0.10 → skip (too expensive)
    assert!(edges.len() >= 1, "expected at least 1 tail edge, got {}", edges.len());

    // Bucket 0 should be found (edge = 0.08)
    let b0 = edges.iter().find(|e| e.bucket_index == 0);
    assert!(b0.is_some(), "bucket 0 edge not found");
    assert!((b0.unwrap().edge - 0.08).abs() < 0.01);

    // Bucket 8 should be found (edge = 0.12)
    let b8 = edges.iter().find(|e| e.bucket_index == 8);
    assert!(b8.is_some(), "bucket 8 edge not found");
    assert!((b8.unwrap().edge - 0.12).abs() < 0.02);
}
```

**Step 2: Run test to verify it passes** (this uses already-built code)

Run: `cargo test --test weather_integration_test 2>&1`
Expected: PASS (this is a pure integration test over tasks 1-4)

**Step 3: Implement `run_weather()` in `src/main.rs`**

Replace the placeholder stub at lines 345-365 with the full weather actor wiring. This is the main async loop:

1. Load config, init DB, create channels
2. Start writer actor (reuse existing)
3. Poll loop: every `weather.poll_interval_secs`:
   - Fetch weather events from Gamma API
   - For each event: fetch ensemble, compute bucket probs, find tail edges
   - For each edge: emit Signal → DecisionActor → Executor
4. Start existing DecisionActor and Executor (paper or live mode)
5. Handle shutdown

Also extend `run_status()` to query `weather_markets` and `weather_forecasts` tables and display a weather section.

**Step 4: Run all tests**

Run: `cargo test 2>&1 | tail -10`
Expected: ALL tests pass

**Step 5: Manual dev test**

Run: `cargo run -- weather --paper --db-path ./dev_weather.db 2>&1 | head -40`
Expected: sees weather events, fetches forecasts, logs tail edges. Does NOT touch `~/.polymarket/data.db`.

**Step 6: Commit**

```bash
git add src/main.rs tests/weather_integration_test.rs
git commit -m "feat(weather): wire run_weather() with full poll→signal→decision→executor pipeline"
```

---

### Task 7: Final Verification

**Step 1: Run all tests**

Run: `cargo test 2>&1`
Expected: ALL tests pass, zero warnings on weather code

**Step 2: Clippy**

Run: `cargo clippy --all-targets 2>&1`
Expected: no errors (warnings ok for existing code)

**Step 3: Verify crypto isolation**

Run: `git diff HEAD src/actors/ src/flow.rs src/polymarket/ src/types.rs src/math/`
Expected: empty diff — no crypto files modified

**Step 4: Dev test with local DB**

Run: `cargo run -- weather --paper --db-path ./dev_weather.db`
Let it run for one poll cycle (~30 seconds for API calls). Verify:
- Weather events discovered and logged
- Ensemble forecasts fetched
- Tail edges detected (if any exist)
- DB populated: `sqlite3 ./dev_weather.db "SELECT count(*) FROM weather_forecasts; SELECT count(*) FROM weather_markets;"`

**Step 5: Status command**

Run: `cargo run -- status --db-path ./dev_weather.db`
Expected: shows weather section with cities tracked and tail signals

**Step 6: Final commit + notify user**

```bash
git add -A
git commit -m "feat(weather): complete weather module — ready for installation"
```

Tell user: "Weather module complete. All tests pass, crypto code untouched. Ready for `cargo install --path .` when you approve."
