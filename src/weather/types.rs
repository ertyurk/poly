use serde::{Deserialize, Serialize};

// ── TempUnit ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TempUnit {
    Fahrenheit,
    Celsius,
}

impl TempUnit {
    /// Width of a single bucket for this unit system.
    const fn bucket_width(self) -> u8 {
        match self {
            Self::Fahrenheit => 2,
            Self::Celsius => 1,
        }
    }
}

// ── Bucket ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bucket {
    pub index: u8,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    pub unit: TempUnit,
    pub label: String,
}

impl Bucket {
    /// Parse a Polymarket `groupItemTitle` into a `Bucket`.
    ///
    /// Supported formats:
    /// - `"74-75°F"` — bounded Fahrenheit range
    /// - `"65°F or below"` — lower tail
    /// - `"80°F or higher"` — upper tail
    /// - `"10°C"` — single-degree Celsius bucket
    /// - `"6°C or below"` — lower tail Celsius
    /// - `"14°C or higher"` — upper tail Celsius
    pub fn parse(label: &str) -> Option<Self> {
        // Determine unit by looking for °F or °C.
        let (unit, degree_marker) = if label.contains("°F") {
            (TempUnit::Fahrenheit, "°F")
        } else if label.contains("°C") {
            (TempUnit::Celsius, "°C")
        } else {
            return None;
        };

        // Strip the degree marker and any surrounding whitespace.
        let body = label.replace(degree_marker, "");
        let body = body.trim();

        let (lo, hi) = if let Some(rest) = body.strip_suffix("or below") {
            // Lower tail: "65 or below"
            let val: f64 = rest.trim().parse().ok()?;
            (None, Some(val))
        } else if let Some(rest) = body.strip_suffix("or higher") {
            // Upper tail: "80 or higher"
            let val: f64 = rest.trim().parse().ok()?;
            (Some(val), None)
        } else if body.contains('-') {
            // Bounded range: "74-75"
            let mut parts = body.split('-');
            // Handle possible negative numbers: if body starts with '-', the
            // first split element will be "" (empty). We need to be careful.
            let lo_str = parts.next()?.trim();
            let hi_str = parts.next()?.trim();
            // Make sure there's no third piece (which would mean an unexpected format).
            if parts.next().is_some() {
                return None;
            }
            let lo_val: f64 = lo_str.parse().ok()?;
            let hi_val: f64 = hi_str.parse().ok()?;
            (Some(lo_val), Some(hi_val))
        } else {
            // Single value: "10" (Celsius single-degree bucket)
            let val: f64 = body.parse().ok()?;
            (Some(val), Some(val))
        };

        Some(Self {
            index: 0,
            lo,
            hi,
            unit,
            label: label.to_owned(),
        })
    }

    /// Check whether `temp` falls within this bucket.
    ///
    /// - Bounded °F: `[lo, lo + 2)`
    /// - Bounded °C: `[lo, lo + 1)`
    /// - Lower tail: `(-inf, hi + bucket_width)`
    /// - Upper tail: `[lo, +inf)`
    pub fn contains(&self, temp: f64) -> bool {
        let width = f64::from(self.unit.bucket_width());

        match (self.lo, self.hi) {
            (Some(lo), Some(_)) => {
                // Bounded bucket
                temp >= lo && temp < lo + width
            }
            (None, Some(hi)) => {
                // Lower tail: (-inf, hi + bucket_width)
                temp < hi + width
            }
            (Some(lo), None) => {
                // Upper tail: [lo, +inf)
                temp >= lo
            }
            (None, None) => false,
        }
    }
}

// ── CityConfig ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CityConfig {
    pub name: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub temp_unit: TempUnit,
    pub icao: &'static str,
}

static CITIES: [CityConfig; 20] = [
    // Fahrenheit cities
    CityConfig { name: "atlanta", lat: 33.749, lon: -84.388, temp_unit: TempUnit::Fahrenheit, icao: "KATL" },
    CityConfig { name: "chicago", lat: 41.878, lon: -87.630, temp_unit: TempUnit::Fahrenheit, icao: "KORD" },
    CityConfig { name: "miami", lat: 25.762, lon: -80.192, temp_unit: TempUnit::Fahrenheit, icao: "KMIA" },
    CityConfig { name: "nyc", lat: 40.713, lon: -74.006, temp_unit: TempUnit::Fahrenheit, icao: "KLGA" },
    CityConfig { name: "dallas", lat: 32.777, lon: -96.797, temp_unit: TempUnit::Fahrenheit, icao: "KDFW" },
    CityConfig { name: "seattle", lat: 47.606, lon: -122.332, temp_unit: TempUnit::Fahrenheit, icao: "KSEA" },
    // Celsius cities
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

impl CityConfig {
    /// Look up a city by name (case-sensitive, lowercase).
    pub fn find(name: &str) -> Option<&'static Self> {
        CITIES.iter().find(|c| c.name == name)
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Returns `true` if `bucket_index` is in the lower or upper tail.
///
/// Lower tail: indices `0 .. tail_count`.
/// Upper tail: indices `total_buckets - tail_count .. total_buckets`.
pub const fn is_tail(bucket_index: u8, total_buckets: u8, tail_count: u8) -> bool {
    bucket_index < tail_count || bucket_index >= total_buckets.saturating_sub(tail_count)
}

/// Build a canonical weather market identifier.
///
/// Format: `WX_{city}_{date}_{bucket_index}`
pub fn weather_market_id(city: &str, date: &str, bucket_index: u8) -> String {
    format!("WX_{city}_{date}_{bucket_index}")
}
