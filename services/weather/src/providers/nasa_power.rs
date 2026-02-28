use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value};

use crate::providers::ProviderError;
use crate::types::{CurrentWeather, DailyForecast, Location, ProviderName};

#[derive(Clone)]
pub struct NasaPowerProvider {
    base_url: String,
    community: String,
    client: reqwest::Client,
}

impl NasaPowerProvider {
    pub fn from_env() -> Self {
        let base_url = std::env::var("NASA_POWER_BASE_URL")
            .unwrap_or_else(|_| "https://power.larc.nasa.gov/api/temporal/daily/point".to_string());
        let timeout = std::env::var("NASA_POWER_TIMEOUT_S")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(10.0);
        let community = std::env::var("WEATHER_NASA_POWER_COMMUNITY")
            .or_else(|_| std::env::var("NASA_POWER_COMMUNITY"))
            .unwrap_or_else(|_| "AG".to_string());

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs_f64(timeout))
            .build()
            .expect("failed to build reqwest client");

        Self {
            base_url,
            community,
            client,
        }
    }

    pub async fn current(&self, loc: &Location) -> Result<CurrentWeather, ProviderError> {
        let today = Utc::now().with_timezone(&loc.tz).date_naive();
        let start = today - chrono::Duration::days(1);
        let forecasts = self.daily(loc, start, today).await?;
        let latest = forecasts.iter().max_by_key(|f| f.day);

        let observed_day = latest.map(|f| f.day).unwrap_or(today);
        let observed_at = loc
            .tz
            .from_local_datetime(&observed_day.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .map(|dt| dt.with_timezone(&dt.offset().fix()))
            .unwrap_or_else(|| {
                let now = Utc::now().with_timezone(&loc.tz);
                now.with_timezone(&now.offset().fix())
            });

        let temperature = choose_temperature(latest);
        Ok(CurrentWeather {
            observed_at,
            temperature_c: temperature,
            wind_speed_mps: None,
            source: ProviderName::NasaPower,
        })
    }

    pub async fn daily(
        &self,
        loc: &Location,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<DailyForecast>, ProviderError> {
        let params = [
            ("latitude", loc.lat.to_string()),
            ("longitude", loc.lon.to_string()),
            ("start", format_date(start)),
            ("end", format_date(end)),
            ("time-standard", "UTC".to_string()),
            ("community", self.community.clone()),
            ("parameters", "T2M_MIN,T2M_MAX,PRECTOTCORR".to_string()),
            ("format", "JSON".to_string()),
        ];
        let payload = self.request(&params).await?;
        let properties = payload.get("properties").and_then(|v| v.as_object());
        let fill_value = properties
            .and_then(|obj| obj.get("fill_value"))
            .and_then(to_f64)
            .unwrap_or(-999.0);
        let parameters = properties
            .and_then(|obj| obj.get("parameter"))
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let tmin = map_from(&parameters, "T2M_MIN");
        let tmax = map_from(&parameters, "T2M_MAX");
        let precip = map_from(&parameters, "PRECTOTCORR");

        let mut keys: BTreeSet<String> = BTreeSet::new();
        keys.extend(tmin.keys().cloned());
        keys.extend(tmax.keys().cloned());
        keys.extend(precip.keys().cloned());

        let mut forecasts = Vec::new();
        for key in keys {
            let local_day = parse_day_to_local(&key, loc.tz);
            let Some(day) = local_day else { continue };
            let t_min = extract_value(&tmin, &key, fill_value);
            let t_max = extract_value(&tmax, &key, fill_value);
            let precipitation = extract_value(&precip, &key, fill_value);
            forecasts.push(DailyForecast {
                day,
                t_min_c: t_min,
                t_max_c: t_max,
                precipitation_mm: precipitation,
                wind_speed_max_mps: None,
                source: ProviderName::NasaPower,
            });
        }
        Ok(forecasts)
    }

    async fn request(
        &self,
        params: &[(&str, String)],
    ) -> Result<Map<String, Value>, ProviderError> {
        let response = self
            .client
            .get(&self.base_url)
            .query(params)
            .send()
            .await
            .map_err(|err| ProviderError::Upstream(err.to_string()))?;

        if response.status().is_client_error() || response.status().is_server_error() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Upstream(format!(
                "nasa_power status={} body={}",
                status,
                snippet(&body)
            )));
        }

        let payload = response
            .json::<Value>()
            .await
            .map_err(|err| ProviderError::InvalidResponse(err.to_string()))?;
        let obj = payload
            .as_object()
            .cloned()
            .ok_or_else(|| ProviderError::InvalidResponse("unexpected payload".to_string()))?;
        Ok(obj)
    }
}

fn parse_day_to_local(raw: &str, tz: Tz) -> Option<NaiveDate> {
    let parsed = NaiveDate::parse_from_str(raw, "%Y%m%d").ok()?;
    let naive = parsed.and_hms_opt(0, 0, 0)?;
    let utc_dt: DateTime<Utc> = Utc.from_utc_datetime(&naive);
    Some(utc_dt.with_timezone(&tz).date_naive())
}

fn extract_value(container: &Map<String, Value>, key: &str, fill: f64) -> Option<f64> {
    let raw = container.get(key)?;
    let value = to_f64(raw)?;
    if (value - fill).abs() < f64::EPSILON {
        return None;
    }
    Some(value)
}

fn map_from(source: &Map<String, Value>, key: &str) -> Map<String, Value> {
    source
        .get(key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default()
}

fn to_f64(value: &Value) -> Option<f64> {
    if let Some(v) = value.as_f64() {
        return Some(v);
    }
    if let Some(v) = value.as_i64() {
        return Some(v as f64);
    }
    if let Some(v) = value.as_str() {
        return v.parse::<f64>().ok();
    }
    None
}

fn choose_temperature(latest: Option<&DailyForecast>) -> Option<f64> {
    let forecast = latest?;
    match (forecast.t_min_c, forecast.t_max_c) {
        (Some(min), Some(max)) => Some((min + max) / 2.0),
        (_, Some(max)) => Some(max),
        (Some(min), None) => Some(min),
        (None, None) => None,
    }
}

fn format_date(value: NaiveDate) -> String {
    value.format("%Y%m%d").to_string()
}

fn snippet(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.len() > 200 {
        format!("{}...", &trimmed[..200])
    } else {
        trimmed.to_string()
    }
}
