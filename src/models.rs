use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct NdviInput {
    pub farm_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub source: Option<String>,
    pub geometry: Option<Value>,
}

impl NdviInput {
    pub fn validate(&self) -> Result<(), String> {
        fn in_range(value: f64) -> bool {
            value.is_finite() && (0.0..=1.0).contains(&value)
        }

        if !in_range(self.min) || !in_range(self.mean) || !in_range(self.max) {
            return Err("ndvi values must be finite and in range 0.0..=1.0".to_string());
        }

        if self.min > self.mean || self.mean > self.max {
            return Err("ndvi values must satisfy min <= mean <= max".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::NdviInput;
    use chrono::Utc;
    use uuid::Uuid;

    fn sample() -> NdviInput {
        NdviInput {
            farm_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            mean: 0.6,
            min: 0.4,
            max: 0.8,
            source: None,
            geometry: None,
        }
    }

    #[test]
    fn validate_ok() {
        let input = sample();
        assert!(input.validate().is_ok());
    }

    #[test]
    fn validate_range_error() {
        let mut input = sample();
        input.mean = 1.5;
        assert!(input.validate().is_err());
    }

    #[test]
    fn validate_order_error() {
        let mut input = sample();
        input.min = 0.7;
        input.mean = 0.6;
        assert!(input.validate().is_err());
    }
}
