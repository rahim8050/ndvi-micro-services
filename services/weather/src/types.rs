use chrono::{DateTime, FixedOffset, NaiveDate};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderName {
    OpenMeteo,
    NasaPower,
}

impl ProviderName {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderName::OpenMeteo => "open_meteo",
            ProviderName::NasaPower => "nasa_power",
        }
    }
}

impl std::str::FromStr for ProviderName {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_lowercase().as_str() {
            "open_meteo" => Ok(ProviderName::OpenMeteo),
            "nasa_power" => Ok(ProviderName::NasaPower),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Location {
    pub lat: f64,
    pub lon: f64,
    pub tz: Tz,
    pub tz_name: String,
}

#[derive(Debug, Clone)]
pub struct CurrentWeather {
    pub observed_at: DateTime<FixedOffset>,
    pub temperature_c: Option<f64>,
    pub wind_speed_mps: Option<f64>,
    pub source: ProviderName,
}

#[derive(Debug, Clone)]
pub struct DailyForecast {
    pub day: NaiveDate,
    pub t_min_c: Option<f64>,
    pub t_max_c: Option<f64>,
    pub precipitation_mm: Option<f64>,
    pub wind_speed_max_mps: Option<f64>,
    pub source: ProviderName,
}

#[derive(Debug, Clone)]
pub struct WeeklyReport {
    pub week_start: NaiveDate,
    pub week_end: NaiveDate,
    pub t_min_avg_c: Option<f64>,
    pub t_max_avg_c: Option<f64>,
    pub precipitation_sum_mm: Option<f64>,
    pub days: Vec<DailyForecast>,
    pub source: ProviderName,
}

#[derive(Debug, Clone)]
pub struct HourlyForecast {
    pub timestamp: DateTime<FixedOffset>,
    pub temperature_c: Option<f64>,
    pub precipitation_mm: Option<f64>,
    pub wind_speed_mps: Option<f64>,
    pub cloud_cover_pct: Option<f64>,
    pub source: ProviderName,
}
