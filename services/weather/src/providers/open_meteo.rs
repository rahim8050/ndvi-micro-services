use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value};

use crate::providers::ProviderError;
use crate::types::{CurrentWeather, DailyForecast, Location, ProviderName};

#[derive(Clone)]
pub struct OpenMeteoProvider {
    base_url: String,
    max_retries: usize,
    backoff: Duration,
    client: reqwest::Client,
}

impl OpenMeteoProvider {
    pub fn from_env() -> Self {
        let base_url = std::env::var("OPEN_METEO_BASE_URL")
            .unwrap_or_else(|_| "https://api.open-meteo.com/v1/forecast".to_string());
        let timeout = std::env::var("OPEN_METEO_TIMEOUT_S")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(10.0);
        let max_retries = std::env::var("OPEN_METEO_MAX_RETRIES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(2);
        let backoff = std::env::var("OPEN_METEO_BACKOFF_S")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.5);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs_f64(timeout))
            .build()
            .expect("failed to build reqwest client");

        Self {
            base_url,
            max_retries,
            backoff: Duration::from_secs_f64(backoff),
            client,
        }
    }

    pub async fn current(&self, loc: &Location) -> Result<CurrentWeather, ProviderError> {
        let params = [
            ("latitude", loc.lat.to_string()),
            ("longitude", loc.lon.to_string()),
            ("current", "temperature_2m,wind_speed_10m".to_string()),
            ("timezone", loc.tz_name.clone()),
        ];

        let payload = self.request(&params).await?;
        let current_block = payload.get("current").and_then(|v| v.as_object());
        let observed_at = current_block
            .and_then(|obj| obj.get("time"))
            .and_then(|value| value.as_str())
            .and_then(|raw| parse_datetime(raw, loc.tz))
            .unwrap_or_else(|| {
                let now = Utc::now().with_timezone(&loc.tz);
                now.with_timezone(&now.offset().fix())
            });

        let temperature = current_block
            .and_then(|obj| obj.get("temperature_2m"))
            .and_then(to_f64);
        let wind_speed = current_block
            .and_then(|obj| obj.get("wind_speed_10m"))
            .and_then(to_f64);

        Ok(CurrentWeather {
            observed_at,
            temperature_c: temperature,
            wind_speed_mps: wind_speed,
            source: ProviderName::OpenMeteo,
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
            ("start_date", start.to_string()),
            ("end_date", end.to_string()),
            (
                "daily",
                "temperature_2m_min,temperature_2m_max,precipitation_sum".to_string(),
            ),
            ("timezone", loc.tz_name.clone()),
        ];

        let payload = self.request(&params).await?;
        let daily_block = payload.get("daily").and_then(|v| v.as_object());
        let dates = daily_block.and_then(|obj| obj.get("time"));
        let t_min_list = daily_block.and_then(|obj| obj.get("temperature_2m_min"));
        let t_max_list = daily_block.and_then(|obj| obj.get("temperature_2m_max"));
        let precip_list = daily_block.and_then(|obj| obj.get("precipitation_sum"));

        let mut forecasts = Vec::new();
        let date_values = dates
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for (idx, raw_day) in date_values.iter().enumerate() {
            let day = raw_day.as_str().and_then(parse_date);
            let Some(day) = day else { continue };
            let t_min = list_value(t_min_list, idx);
            let t_max = list_value(t_max_list, idx);
            let precip = list_value(precip_list, idx);
            forecasts.push(DailyForecast {
                day,
                t_min_c: t_min,
                t_max_c: t_max,
                precipitation_mm: precip,
                source: ProviderName::OpenMeteo,
            });
        }
        Ok(forecasts)
    }

    async fn request(
        &self,
        params: &[(&str, String)],
    ) -> Result<Map<String, Value>, ProviderError> {
        let mut last_error: Option<ProviderError> = None;
        for attempt in 0..=self.max_retries {
            let response = self.client.get(&self.base_url).query(params).send().await;

            let response = match response {
                Ok(value) => value,
                Err(err) => {
                    last_error = Some(ProviderError::Upstream(err.to_string()));
                    if attempt < self.max_retries {
                        tokio::time::sleep(self.backoff.mul_f64((attempt as f64) + 1.0)).await;
                        continue;
                    }
                    return Err(ProviderError::Upstream(err.to_string()));
                }
            };

            if response.status().is_server_error() && attempt < self.max_retries {
                tokio::time::sleep(self.backoff.mul_f64((attempt as f64) + 1.0)).await;
                continue;
            }

            if response.status().is_client_error() || response.status().is_server_error() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(ProviderError::Upstream(format!(
                    "open_meteo status={} body={}",
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
            return Ok(obj);
        }

        Err(last_error
            .unwrap_or_else(|| ProviderError::Upstream("open_meteo request failed".to_string())))
    }
}

fn parse_datetime(raw: &str, tz: Tz) -> Option<DateTime<FixedOffset>> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        let localized = parsed.with_timezone(&tz);
        return Some(localized.with_timezone(&localized.offset().fix()));
    }

    let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    let localized = tz.from_local_datetime(&naive).single()?;
    Some(localized.with_timezone(&localized.offset().fix()))
}

fn parse_date(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()
}

fn list_value(values: Option<&Value>, idx: usize) -> Option<f64> {
    let array = values.and_then(|v| v.as_array())?;
    let value = array.get(idx)?;
    to_f64(value)
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

fn snippet(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.len() > 200 {
        format!("{}...", &trimmed[..200])
    } else {
        trimmed.to_string()
    }
}
