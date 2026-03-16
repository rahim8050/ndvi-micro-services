use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use ndvi_common::Envelope;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

use crate::config::WeatherConfig;
use crate::metrics;
use crate::services;
use crate::state::AppState;
use crate::types::{DailyForecast, HourlyForecast, ProviderName, WeeklyReport};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        .route("/api/v1/weather/current/", get(weather_current))
        .route("/api/v1/weather/daily/", get(weather_daily))
        .route("/api/v1/weather/weekly/", get(weather_weekly))
        .route("/api/v1/weather/hourly/", get(weather_hourly))
        .route_layer(middleware::from_fn(metrics::metrics_middleware))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    let envelope = Envelope::success("ok", json!({"status": "ok"}));
    (StatusCode::OK, Json(envelope))
}

async fn metrics_handler() -> Response {
    match metrics::render_metrics() {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, metrics::METRICS_CONTENT_TYPE)],
            body,
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "failed to render metrics");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn weather_current(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let params = match parse_base_params(&params, &state.config) {
        Ok(value) => value,
        Err(response) => return response,
    };

    let cache_key = format!("weather:current:{}", params.cache_key_segment());
    let current = match state.cache.get_current(&cache_key) {
        Some(value) => value,
        None => {
            let location = params.location();
            match services::get_current(&state.providers, params.provider, &location).await {
                Ok(value) => {
                    state.cache.set_current(
                        cache_key,
                        value.clone(),
                        state.config.cache_ttl_current_s,
                    );
                    value
                }
                Err(err) => return provider_error(err),
            }
        }
    };

    let payload = CurrentWeatherResponse::from(current);
    let envelope = Envelope::success("OK", payload);
    (StatusCode::OK, Json(envelope)).into_response()
}

async fn weather_daily(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let params = match parse_range_params(&params, &state.config) {
        Ok(value) => value,
        Err(response) => return response,
    };

    let cache_key = format!("weather:daily:{}", params.cache_key_segment());
    let forecasts = match state.cache.get_daily(&cache_key) {
        Some(value) => value,
        None => {
            let location = params.base.location();
            match services::get_daily(
                &state.providers,
                params.base.provider,
                &location,
                params.start,
                params.end,
            )
            .await
            {
                Ok(value) => {
                    state
                        .cache
                        .set_daily(cache_key, value.clone(), state.config.cache_ttl_daily_s);
                    value
                }
                Err(err) => return provider_error(err),
            }
        }
    };

    let today = Utc::now()
        .with_timezone(&state.config.default_tz)
        .date_naive();
    let payload = WeatherDailyData {
        forecasts: forecasts
            .iter()
            .map(|forecast| DailyForecastResponse::from(forecast, &state.config, today))
            .collect(),
    };
    let envelope = Envelope::success("OK", payload);
    (StatusCode::OK, Json(envelope)).into_response()
}

async fn weather_weekly(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let params = match parse_range_params(&params, &state.config) {
        Ok(value) => value,
        Err(response) => return response,
    };

    let weekly_cache_key = format!("weather:weekly:{}", params.cache_key_segment());
    let reports = match state.cache.get_weekly(&weekly_cache_key) {
        Some(value) => value,
        None => {
            let daily_cache_key = format!("weather:daily:{}", params.cache_key_segment());
            let forecasts = match state.cache.get_daily(&daily_cache_key) {
                Some(value) => value,
                None => {
                    let location = params.base.location();
                    match services::get_daily(
                        &state.providers,
                        params.base.provider,
                        &location,
                        params.start,
                        params.end,
                    )
                    .await
                    {
                        Ok(value) => {
                            state.cache.set_daily(
                                daily_cache_key,
                                value.clone(),
                                state.config.cache_ttl_daily_s,
                            );
                            value
                        }
                        Err(err) => return provider_error(err),
                    }
                }
            };
            let generated = services::aggregate_weekly(&forecasts, params.base.provider);
            state.cache.set_weekly(
                weekly_cache_key,
                generated.clone(),
                state.config.cache_ttl_weekly_s,
            );
            generated
        }
    };
    let today = Utc::now()
        .with_timezone(&state.config.default_tz)
        .date_naive();
    let payload = WeatherWeeklyData {
        reports: reports
            .iter()
            .map(|report| WeeklyReportResponse::from(report, &state.config, today))
            .collect(),
    };
    let envelope = Envelope::success("OK", payload);
    (StatusCode::OK, Json(envelope)).into_response()
}

async fn weather_hourly(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let params = match parse_hourly_params(&params, &state.config) {
        Ok(value) => value,
        Err(response) => return response,
    };

    let cache_key = format!("weather:hourly:{}", params.cache_key_segment());
    let forecasts = match state.cache.get_hourly(&cache_key) {
        Some(value) => value,
        None => {
            let location = params.base.location();
            match services::get_hourly(
                &state.providers,
                params.base.provider,
                &location,
                params.hours,
            )
            .await
            {
                Ok(value) => {
                    state.cache.set_hourly(
                        cache_key,
                        value.clone(),
                        state.config.cache_ttl_hourly_s,
                    );
                    value
                }
                Err(err) => return provider_error(err),
            }
        }
    };

    let payload = HourlyForecastData {
        hours: forecasts
            .iter()
            .map(|forecast| HourlyForecastResponse::from(forecast))
            .collect(),
    };
    let envelope = Envelope::success("OK", payload);
    (StatusCode::OK, Json(envelope)).into_response()
}

#[derive(Serialize)]
struct ErrorEnvelope {
    status: i32,
    message: String,
    errors: Value,
}

fn error_response(status: StatusCode, message: &str, errors: Value) -> Response {
    let envelope = ErrorEnvelope {
        status: 1,
        message: message.to_string(),
        errors,
    };
    (status, Json(envelope)).into_response()
}

fn provider_error(err: crate::providers::ProviderError) -> Response {
    tracing::error!(error = ?err, "weather provider error");
    let errors = json!({ "detail": "Weather upstream error" });
    error_response(StatusCode::BAD_GATEWAY, "Weather upstream error", errors)
}

#[derive(Clone)]
struct ValidatedBaseParams {
    lat: f64,
    lon: f64,
    tz: chrono_tz::Tz,
    tz_name: String,
    provider: ProviderName,
}

impl ValidatedBaseParams {
    fn location(&self) -> crate::types::Location {
        crate::types::Location {
            lat: self.lat,
            lon: self.lon,
            tz: self.tz,
            tz_name: self.tz_name.clone(),
        }
    }

    fn cache_key_segment(&self) -> String {
        format!(
            "{}:{:.4}:{:.4}:{}",
            self.provider.as_str(),
            self.lat,
            self.lon,
            self.tz_name
        )
    }
}

struct ValidatedRangeParams {
    base: ValidatedBaseParams,
    start: NaiveDate,
    end: NaiveDate,
}

impl ValidatedRangeParams {
    fn cache_key_segment(&self) -> String {
        format!(
            "{}:{}:{}",
            self.base.cache_key_segment(),
            self.start,
            self.end
        )
    }
}

struct ValidatedHourlyParams {
    base: ValidatedBaseParams,
    hours: u32,
}

impl ValidatedHourlyParams {
    fn cache_key_segment(&self) -> String {
        format!("{}:{}", self.base.cache_key_segment(), self.hours)
    }
}

fn parse_base_params(
    params: &HashMap<String, String>,
    config: &WeatherConfig,
) -> Result<ValidatedBaseParams, Response> {
    let mut errors: Map<String, Value> = Map::new();
    let lat = parse_required_float(params, "lat", -90.0, 90.0, &mut errors);
    let lon = parse_required_float(params, "lon", -180.0, 180.0, &mut errors);

    let tz_value = params.get("tz").map(String::as_str);
    let (tz, tz_name) = match config.resolve_tz(tz_value) {
        Ok(value) => value,
        Err(message) => {
            push_error(&mut errors, "tz", &message);
            (config.default_tz, config.default_tz_name.clone())
        }
    };

    let provider_raw = params.get("provider").map(String::as_str);
    let provider = match services::resolve_provider(provider_raw, config) {
        Ok(value) => value,
        Err(message) => {
            push_error(&mut errors, "provider", message);
            config.provider_default
        }
    };

    if !errors.is_empty() {
        let errors = Value::Object(errors);
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "Request failed",
            errors,
        ));
    }

    Ok(ValidatedBaseParams {
        lat: lat.unwrap_or(0.0),
        lon: lon.unwrap_or(0.0),
        tz,
        tz_name,
        provider,
    })
}

fn parse_range_params(
    params: &HashMap<String, String>,
    config: &WeatherConfig,
) -> Result<ValidatedRangeParams, Response> {
    let base = match parse_base_params(params, config) {
        Ok(value) => value,
        Err(response) => return Err(response),
    };

    let mut errors: Map<String, Value> = Map::new();
    let start = parse_required_date(params, "start", &mut errors);
    let end = parse_required_date(params, "end", &mut errors);

    if let (Some(start), Some(end)) = (start, end) {
        if start > end {
            push_error(
                &mut errors,
                "non_field_errors",
                "start must be on or before end.",
            );
        } else {
            let delta_days = (end - start).num_days();
            if delta_days > config.max_range_days {
                push_error(
                    &mut errors,
                    "non_field_errors",
                    "Requested range exceeds WEATHER_MAX_RANGE_DAYS.",
                );
            }
        }
    }

    if !errors.is_empty() {
        let errors = Value::Object(errors);
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "Request failed",
            errors,
        ));
    }

    Ok(ValidatedRangeParams {
        base,
        start: start.unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()),
        end: end.unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()),
    })
}

fn parse_hourly_params(
    params: &HashMap<String, String>,
    config: &WeatherConfig,
) -> Result<ValidatedHourlyParams, Response> {
    let base = match parse_base_params(params, config) {
        Ok(value) => value,
        Err(response) => return Err(response),
    };

    let mut errors: Map<String, Value> = Map::new();
    let hours = parse_hours_param(params, &mut errors);

    if !errors.is_empty() {
        let errors = Value::Object(errors);
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "Request failed",
            errors,
        ));
    }

    Ok(ValidatedHourlyParams {
        base,
        hours: hours.unwrap_or(48),
    })
}

fn parse_hours_param(
    params: &HashMap<String, String>,
    errors: &mut Map<String, Value>,
) -> Option<u32> {
    let raw = match params.get("hours") {
        Some(value) => value,
        None => return Some(48),
    };

    let value = match raw.parse::<u32>() {
        Ok(value) => value,
        Err(_) => {
            push_error(errors, "hours", "A valid integer is required.");
            return None;
        }
    };

    if value < 1 || value > 168 {
        push_error(errors, "hours", "Ensure this value is between 1 and 168.");
        return None;
    }
    Some(value)
}

fn parse_required_float(
    params: &HashMap<String, String>,
    key: &str,
    min: f64,
    max: f64,
    errors: &mut Map<String, Value>,
) -> Option<f64> {
    let raw = match params.get(key) {
        Some(value) => value,
        None => {
            push_error(errors, key, "This field is required.");
            return None;
        }
    };

    let value = match raw.parse::<f64>() {
        Ok(value) => value,
        Err(_) => {
            push_error(errors, key, "A valid number is required.");
            return None;
        }
    };

    if value < min {
        push_error(
            errors,
            key,
            &format!("Ensure this value is greater than or equal to {min}."),
        );
        return None;
    }
    if value > max {
        push_error(
            errors,
            key,
            &format!("Ensure this value is less than or equal to {max}."),
        );
        return None;
    }
    Some(value)
}

fn parse_required_date(
    params: &HashMap<String, String>,
    key: &str,
    errors: &mut Map<String, Value>,
) -> Option<NaiveDate> {
    let raw = match params.get(key) {
        Some(value) => value,
        None => {
            push_error(errors, key, "This field is required.");
            return None;
        }
    };
    match NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        Ok(value) => Some(value),
        Err(_) => {
            push_error(
                errors,
                key,
                "Date has wrong format. Use one of these formats instead: YYYY-MM-DD.",
            );
            None
        }
    }
}

fn push_error(errors: &mut Map<String, Value>, key: &str, message: &str) {
    let entry = errors
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(items) = entry {
        items.push(Value::String(message.to_string()));
    }
}

#[derive(Serialize)]
struct CurrentWeatherResponse {
    observed_at: DateTime<FixedOffset>,
    temperature_c: Option<f64>,
    wind_speed_mps: Option<f64>,
    source: String,
}

impl From<crate::types::CurrentWeather> for CurrentWeatherResponse {
    fn from(value: crate::types::CurrentWeather) -> Self {
        Self {
            observed_at: value.observed_at,
            temperature_c: value.temperature_c,
            wind_speed_mps: value.wind_speed_mps,
            source: value.source.as_str().to_string(),
        }
    }
}

#[derive(Serialize)]
struct DailyForecastResponse {
    day: NaiveDate,
    t_min_c: Option<f64>,
    t_max_c: Option<f64>,
    precipitation_mm: Option<f64>,
    wind_speed_max_mps: Option<f64>,
    is_partial: bool,
    missing_fields: Vec<String>,
    source: String,
}

impl DailyForecastResponse {
    fn from(value: &DailyForecast, config: &WeatherConfig, today: NaiveDate) -> Self {
        let missing = services::missing_fields(value);
        let is_partial = services::is_partial_day(value, config, today);
        Self {
            day: value.day,
            t_min_c: value.t_min_c,
            t_max_c: value.t_max_c,
            precipitation_mm: value.precipitation_mm,
            wind_speed_max_mps: value.wind_speed_max_mps,
            is_partial,
            missing_fields: missing.iter().map(|field| field.to_string()).collect(),
            source: value.source.as_str().to_string(),
        }
    }
}

#[derive(Serialize)]
struct WeatherDailyData {
    forecasts: Vec<DailyForecastResponse>,
}

#[derive(Serialize)]
struct WeeklyReportResponse {
    week_start: NaiveDate,
    week_end: NaiveDate,
    t_min_avg_c: Option<f64>,
    t_max_avg_c: Option<f64>,
    precipitation_sum_mm: Option<f64>,
    days: Vec<DailyForecastResponse>,
    is_partial: bool,
    missing_days_count: usize,
    source: String,
}

impl WeeklyReportResponse {
    fn from(value: &WeeklyReport, config: &WeatherConfig, today: NaiveDate) -> Self {
        let days = value
            .days
            .iter()
            .map(|day| DailyForecastResponse::from(day, config, today))
            .collect::<Vec<_>>();
        let missing_days_count = value
            .days
            .iter()
            .filter(|day| services::is_missing_day(day))
            .count();
        let is_partial = value
            .days
            .iter()
            .any(|day| services::is_partial_day(day, config, today));
        Self {
            week_start: value.week_start,
            week_end: value.week_end,
            t_min_avg_c: value.t_min_avg_c,
            t_max_avg_c: value.t_max_avg_c,
            precipitation_sum_mm: value.precipitation_sum_mm,
            days,
            is_partial,
            missing_days_count,
            source: value.source.as_str().to_string(),
        }
    }
}

#[derive(Serialize)]
struct WeatherWeeklyData {
    reports: Vec<WeeklyReportResponse>,
}

#[derive(Serialize)]
struct HourlyForecastResponse {
    timestamp: DateTime<FixedOffset>,
    temperature_c: Option<f64>,
    precipitation_mm: Option<f64>,
    wind_speed_mps: Option<f64>,
    cloud_cover_pct: Option<f64>,
    source: String,
}

impl From<&HourlyForecast> for HourlyForecastResponse {
    fn from(value: &HourlyForecast) -> Self {
        Self {
            timestamp: value.timestamp,
            temperature_c: value.temperature_c,
            precipitation_mm: value.precipitation_mm,
            wind_speed_mps: value.wind_speed_mps,
            cloud_cover_pct: value.cloud_cover_pct,
            source: value.source.as_str().to_string(),
        }
    }
}

#[derive(Serialize)]
struct HourlyForecastData {
    hours: Vec<HourlyForecastResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn daily_forecast_response_includes_wind_speed_max_mps() {
        let config = WeatherConfig {
            default_tz: "Africa/Nairobi".parse().expect("valid timezone"),
            default_tz_name: "Africa/Nairobi".to_string(),
            max_range_days: 366,
            provider_default: ProviderName::OpenMeteo,
            nasa_power_daily_lag_days: 2,
            cache_ttl_current_s: 120,
            cache_ttl_daily_s: 900,
            cache_ttl_weekly_s: 1800,
            cache_ttl_hourly_s: 600,
        };
        let forecast = DailyForecast {
            day: NaiveDate::from_ymd_opt(2026, 2, 28).expect("valid date"),
            t_min_c: Some(10.0),
            t_max_c: Some(20.0),
            precipitation_mm: Some(1.2),
            wind_speed_max_mps: Some(6.5),
            source: ProviderName::OpenMeteo,
        };

        let response = DailyForecastResponse::from(
            &forecast,
            &config,
            NaiveDate::from_ymd_opt(2026, 2, 28).expect("valid date"),
        );
        let json = serde_json::to_value(response).expect("serializes");
        assert_eq!(json["wind_speed_max_mps"].as_f64(), Some(6.5));
    }

    #[test]
    fn daily_forecast_response_serializes_null_wind_speed_max_mps() {
        let config = WeatherConfig {
            default_tz: "Africa/Nairobi".parse().expect("valid timezone"),
            default_tz_name: "Africa/Nairobi".to_string(),
            max_range_days: 366,
            provider_default: ProviderName::NasaPower,
            nasa_power_daily_lag_days: 2,
            cache_ttl_current_s: 120,
            cache_ttl_daily_s: 900,
            cache_ttl_weekly_s: 1800,
            cache_ttl_hourly_s: 600,
        };
        let forecast = DailyForecast {
            day: NaiveDate::from_ymd_opt(2026, 2, 28).expect("valid date"),
            t_min_c: Some(10.0),
            t_max_c: Some(20.0),
            precipitation_mm: Some(1.2),
            wind_speed_max_mps: None,
            source: ProviderName::NasaPower,
        };

        let response = DailyForecastResponse::from(
            &forecast,
            &config,
            NaiveDate::from_ymd_opt(2026, 2, 28).expect("valid date"),
        );
        let json = serde_json::to_value(response).expect("serializes");
        assert!(json["wind_speed_max_mps"].is_null());
    }
}
