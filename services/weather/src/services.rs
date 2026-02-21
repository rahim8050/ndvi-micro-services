use chrono::{Datelike, Duration, NaiveDate};

use crate::config::WeatherConfig;
use crate::providers::{ProviderError, Providers};
use crate::types::{DailyForecast, Location, ProviderName, WeeklyReport};

pub async fn get_current(
    providers: &Providers,
    provider: ProviderName,
    location: &Location,
) -> Result<crate::types::CurrentWeather, ProviderError> {
    providers.current(provider, location).await
}

pub async fn get_daily(
    providers: &Providers,
    provider: ProviderName,
    location: &Location,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<DailyForecast>, ProviderError> {
    providers.daily(provider, location, start, end).await
}

pub fn aggregate_weekly(forecasts: &[DailyForecast], provider: ProviderName) -> Vec<WeeklyReport> {
    let mut buckets: std::collections::BTreeMap<NaiveDate, WeeklyBucket> =
        std::collections::BTreeMap::new();

    let mut sorted = forecasts.to_vec();
    sorted.sort_by_key(|f| f.day);

    for forecast in sorted {
        let week_start =
            forecast.day - Duration::days(forecast.day.weekday().num_days_from_monday() as i64);
        let week_end = week_start + Duration::days(6);
        let bucket = buckets.entry(week_start).or_insert_with(|| WeeklyBucket {
            week_end,
            days: Vec::new(),
            tmin_sum: 0.0,
            tmin_count: 0,
            tmax_sum: 0.0,
            tmax_count: 0,
            precip_sum: 0.0,
            precip_count: 0,
        });

        if let Some(value) = forecast.t_min_c {
            bucket.tmin_sum += value;
            bucket.tmin_count += 1;
        }
        if let Some(value) = forecast.t_max_c {
            bucket.tmax_sum += value;
            bucket.tmax_count += 1;
        }
        if let Some(value) = forecast.precipitation_mm {
            bucket.precip_sum += value;
            bucket.precip_count += 1;
        }

        bucket.days.push(forecast);
    }

    buckets
        .into_iter()
        .map(|(week_start, bucket)| {
            let t_min_avg = if bucket.tmin_count > 0 {
                Some(bucket.tmin_sum / bucket.tmin_count as f64)
            } else {
                None
            };
            let t_max_avg = if bucket.tmax_count > 0 {
                Some(bucket.tmax_sum / bucket.tmax_count as f64)
            } else {
                None
            };
            let precipitation_sum = if bucket.precip_count > 0 {
                Some(bucket.precip_sum)
            } else {
                None
            };
            WeeklyReport {
                week_start,
                week_end: bucket.week_end,
                t_min_avg_c: t_min_avg,
                t_max_avg_c: t_max_avg,
                precipitation_sum_mm: precipitation_sum,
                days: bucket.days,
                source: provider,
            }
        })
        .collect()
}

pub fn missing_fields(day: &DailyForecast) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if day.t_min_c.is_none() {
        missing.push("t_min_c");
    }
    if day.t_max_c.is_none() {
        missing.push("t_max_c");
    }
    if day.precipitation_mm.is_none() {
        missing.push("precipitation_mm");
    }
    missing
}

pub fn is_missing_day(day: &DailyForecast) -> bool {
    day.t_min_c.is_none() && day.t_max_c.is_none() && day.precipitation_mm.is_none()
}

pub fn is_partial_day(day: &DailyForecast, config: &WeatherConfig, today: NaiveDate) -> bool {
    let missing = missing_fields(day);
    if missing.is_empty() {
        return false;
    }
    if day.source == ProviderName::NasaPower {
        let cutoff = today - Duration::days(config.nasa_power_daily_lag_days);
        if day.day > cutoff {
            return true;
        }
    }
    true
}

pub fn resolve_provider(
    raw: Option<&str>,
    config: &WeatherConfig,
) -> Result<ProviderName, &'static str> {
    if let Some(provider) = raw {
        if provider.trim().is_empty() {
            return Ok(config.provider_default);
        }
        return provider
            .parse::<ProviderName>()
            .map_err(|_| "Unknown provider.");
    }
    Ok(config.provider_default)
}

struct WeeklyBucket {
    week_end: NaiveDate,
    days: Vec<DailyForecast>,
    tmin_sum: f64,
    tmin_count: usize,
    tmax_sum: f64,
    tmax_count: usize,
    precip_sum: f64,
    precip_count: usize,
}
