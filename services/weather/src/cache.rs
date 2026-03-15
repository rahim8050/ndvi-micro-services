use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{CurrentWeather, DailyForecast, HourlyForecast, WeeklyReport};

#[derive(Clone)]
pub struct WeatherResponseCache {
    current: Arc<Mutex<HashMap<String, TimedEntry<CurrentWeather>>>>,
    daily: Arc<Mutex<HashMap<String, TimedEntry<Vec<DailyForecast>>>>>,
    weekly: Arc<Mutex<HashMap<String, TimedEntry<Vec<WeeklyReport>>>>>,
    hourly: Arc<Mutex<HashMap<String, TimedEntry<Vec<HourlyForecast>>>>>,
}

#[derive(Clone)]
struct TimedEntry<T> {
    value: T,
    expires_at: Instant,
}

impl WeatherResponseCache {
    pub fn new() -> Self {
        Self {
            current: Arc::new(Mutex::new(HashMap::new())),
            daily: Arc::new(Mutex::new(HashMap::new())),
            weekly: Arc::new(Mutex::new(HashMap::new())),
            hourly: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get_current(&self, key: &str) -> Option<CurrentWeather> {
        get_cached(&self.current, key)
    }

    pub fn set_current(&self, key: String, value: CurrentWeather, ttl_secs: u64) {
        set_cached(&self.current, key, value, ttl_secs);
    }

    pub fn get_daily(&self, key: &str) -> Option<Vec<DailyForecast>> {
        get_cached(&self.daily, key)
    }

    pub fn set_daily(&self, key: String, value: Vec<DailyForecast>, ttl_secs: u64) {
        set_cached(&self.daily, key, value, ttl_secs);
    }

    pub fn get_weekly(&self, key: &str) -> Option<Vec<WeeklyReport>> {
        get_cached(&self.weekly, key)
    }

    pub fn set_weekly(&self, key: String, value: Vec<WeeklyReport>, ttl_secs: u64) {
        set_cached(&self.weekly, key, value, ttl_secs);
    }

    pub fn get_hourly(&self, key: &str) -> Option<Vec<HourlyForecast>> {
        get_cached(&self.hourly, key)
    }

    pub fn set_hourly(&self, key: String, value: Vec<HourlyForecast>, ttl_secs: u64) {
        set_cached(&self.hourly, key, value, ttl_secs);
    }
}

fn get_cached<T: Clone>(store: &Mutex<HashMap<String, TimedEntry<T>>>, key: &str) -> Option<T> {
    let mut guard = store.lock().ok()?;
    match guard.get(key) {
        Some(entry) if entry.expires_at > Instant::now() => Some(entry.value.clone()),
        Some(_) => {
            guard.remove(key);
            None
        }
        None => None,
    }
}

fn set_cached<T: Clone>(
    store: &Mutex<HashMap<String, TimedEntry<T>>>,
    key: String,
    value: T,
    ttl_secs: u64,
) {
    if ttl_secs == 0 {
        return;
    }

    if let Ok(mut guard) = store.lock() {
        let expires_at = Instant::now() + Duration::from_secs(ttl_secs);
        guard.insert(key, TimedEntry { value, expires_at });
    }
}
