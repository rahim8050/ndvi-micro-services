use crate::types::{CurrentWeather, DailyForecast, Location, ProviderName};

mod nasa_power;
mod open_meteo;

pub use nasa_power::NasaPowerProvider;
pub use open_meteo::OpenMeteoProvider;

#[derive(Debug)]
pub enum ProviderError {
    Upstream(String),
    InvalidResponse(String),
}

#[derive(Clone)]
pub struct Providers {
    open_meteo: OpenMeteoProvider,
    nasa_power: NasaPowerProvider,
}

impl Providers {
    pub fn from_env() -> Self {
        Self {
            open_meteo: OpenMeteoProvider::from_env(),
            nasa_power: NasaPowerProvider::from_env(),
        }
    }

    pub async fn current(
        &self,
        provider: ProviderName,
        location: &Location,
    ) -> Result<CurrentWeather, ProviderError> {
        match provider {
            ProviderName::OpenMeteo => self.open_meteo.current(location).await,
            ProviderName::NasaPower => self.nasa_power.current(location).await,
        }
    }

    pub async fn daily(
        &self,
        provider: ProviderName,
        location: &Location,
        start: chrono::NaiveDate,
        end: chrono::NaiveDate,
    ) -> Result<Vec<DailyForecast>, ProviderError> {
        match provider {
            ProviderName::OpenMeteo => self.open_meteo.daily(location, start, end).await,
            ProviderName::NasaPower => self.nasa_power.daily(location, start, end).await,
        }
    }
}
