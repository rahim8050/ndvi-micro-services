use crate::types::ProviderName;

#[derive(Clone)]
pub struct WeatherConfig {
    pub default_tz: chrono_tz::Tz,
    pub default_tz_name: String,
    pub max_range_days: i64,
    pub provider_default: ProviderName,
    pub nasa_power_daily_lag_days: i64,
    pub cache_ttl_current_s: u64,
    pub cache_ttl_daily_s: u64,
    pub cache_ttl_weekly_s: u64,
    pub cache_ttl_hourly_s: u64,
}

impl WeatherConfig {
    pub fn from_env() -> Result<Self, String> {
        let default_tz_name =
            std::env::var("WEATHER_DEFAULT_TZ").unwrap_or_else(|_| "Africa/Nairobi".to_string());
        let default_tz = default_tz_name
            .parse::<chrono_tz::Tz>()
            .map_err(|_| "WEATHER_DEFAULT_TZ is invalid")?;

        let provider_default_raw =
            std::env::var("WEATHER_PROVIDER_DEFAULT").unwrap_or_else(|_| "open_meteo".to_string());
        let provider_default = provider_default_raw
            .parse::<ProviderName>()
            .map_err(|_| "WEATHER_PROVIDER_DEFAULT must be open_meteo or nasa_power")?;

        let max_range_days = std::env::var("WEATHER_MAX_RANGE_DAYS")
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(366);

        let nasa_power_daily_lag_days = std::env::var("NASA_POWER_DAILY_LAG_DAYS")
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(2);
        let cache_ttl_current_s = std::env::var("WEATHER_CACHE_TTL_CURRENT_S")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(120);
        let cache_ttl_daily_s = std::env::var("WEATHER_CACHE_TTL_DAILY_S")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(900);
        let cache_ttl_weekly_s = std::env::var("WEATHER_CACHE_TTL_WEEKLY_S")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1800);
        let cache_ttl_hourly_s = std::env::var("WEATHER_CACHE_TTL_HOURLY_S")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(600);

        Ok(Self {
            default_tz,
            default_tz_name,
            max_range_days,
            provider_default,
            nasa_power_daily_lag_days,
            cache_ttl_current_s,
            cache_ttl_daily_s,
            cache_ttl_weekly_s,
            cache_ttl_hourly_s,
        })
    }

    pub fn resolve_tz(&self, candidate: Option<&str>) -> Result<(chrono_tz::Tz, String), String> {
        if let Some(raw) = candidate {
            let tz = raw
                .parse::<chrono_tz::Tz>()
                .map_err(|_| "Invalid timezone.")?;
            return Ok((tz, raw.to_string()));
        }
        Ok((self.default_tz, self.default_tz_name.clone()))
    }
}
