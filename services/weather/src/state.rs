use sqlx::MySqlPool;

use crate::cache::WeatherResponseCache;
use crate::config::WeatherConfig;
use crate::providers::Providers;

#[derive(Clone)]
pub struct AppState {
    pub pool: MySqlPool,
    pub config: WeatherConfig,
    pub providers: Providers,
    pub cache: WeatherResponseCache,
}
