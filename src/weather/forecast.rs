use crate::weather::types::{Bucket, CityConfig, TempUnit};

/// Build the Open-Meteo ensemble API URL for a given city and date.
///
/// Appends `&temperature_unit=fahrenheit` only for Fahrenheit cities.
pub fn build_open_meteo_url(city: &CityConfig, date: &str) -> String {
    let base = format!(
        "https://ensemble-api.open-meteo.com/v1/ensemble?latitude={lat}&longitude={lon}&daily=temperature_2m_max&models=ecmwf_ifs025,gfs025&start_date={date}&end_date={date}&timezone=auto",
        lat = city.lat,
        lon = city.lon,
    );

    if city.temp_unit == TempUnit::Fahrenheit {
        format!("{base}&temperature_unit=fahrenheit")
    } else {
        base
    }
}

/// Extract ensemble member temperatures from Open-Meteo JSON response.
///
/// Looks for keys matching `temperature_2m_max_memberNN` (01..99) inside
/// `json["daily"]` and returns the first element of each array.
/// Returns `None` if no member keys are found.
pub fn parse_ensemble_temps(json: &serde_json::Value) -> Option<Vec<f64>> {
    let daily = json.get("daily")?;
    let obj = daily.as_object()?;

    let mut temps = Vec::new();

    // When querying multiple models, Open-Meteo appends model suffixes:
    //   temperature_2m_max_member01_ecmwf_ifs025_ensemble
    //   temperature_2m_max_member01_ncep_gefs025
    // With a single model it's just: temperature_2m_max_member01
    // Match any key that starts with "temperature_2m_max_member".
    for (key, val) in obj.iter() {
        if key.starts_with("temperature_2m_max_member") {
            if let Some(arr) = val.as_array() {
                if let Some(temp) = arr.first().and_then(serde_json::Value::as_f64) {
                    temps.push(temp);
                }
            }
        }
    }

    if temps.is_empty() { None } else { Some(temps) }
}

/// Compute the fraction of ensemble temps falling into each bucket.
///
/// Returns a `Vec<f64>` of the same length as `buckets`, summing to ~1.0
/// (assuming every temp falls into exactly one bucket).
pub fn bucket_probabilities(buckets: &[Bucket], temps: &[f64]) -> Vec<f64> {
    if temps.is_empty() {
        return vec![0.0; buckets.len()];
    }

    let n = temps.len() as f64;
    buckets
        .iter()
        .map(|b| {
            let count = temps.iter().filter(|&&t| b.contains(t)).count() as f64;
            count / n
        })
        .collect()
}

/// Fetch ensemble temperature forecasts from Open-Meteo for a city + date.
pub async fn fetch_ensemble(
    http: &reqwest::Client,
    city: &CityConfig,
    date: &str,
) -> Result<Vec<f64>, Box<dyn std::error::Error + Send + Sync>> {
    let url = build_open_meteo_url(city, date);
    let resp = http.get(&url).send().await?;
    let text = resp.text().await?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        format!("JSON parse error for {}: {} — body: {}", city.name, e, &text[..text.len().min(200)])
    })?;
    parse_ensemble_temps(&json)
        .ok_or_else(|| {
            let keys: Vec<_> = json.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
            let daily_keys: Vec<_> = json.get("daily").and_then(|d| d.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default();
            format!("no ensemble members: top_keys={keys:?} daily_keys={daily_keys:?}").into()
        })
}
