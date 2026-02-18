use serde::Serialize;
use serde_json::Value;

pub type JsonValue = Value;

pub mod auth;
pub mod throttle;

pub use auth::{AuthContext, AuthKind};

#[derive(Debug, Serialize)]
pub struct Envelope<T> {
    pub status: i32,
    pub message: String,
    pub data: Option<T>,
    pub errors: Option<JsonValue>,
}

impl<T> Envelope<T> {
    pub fn success(message: impl Into<String>, data: T) -> Self {
        Self {
            status: 0,
            message: message.into(),
            data: Some(data),
            errors: None,
        }
    }
}

impl Envelope<JsonValue> {
    pub fn failure(message: impl Into<String>, errors: Option<JsonValue>) -> Self {
        Self {
            status: 1,
            message: message.into(),
            data: None,
            errors,
        }
    }
}
