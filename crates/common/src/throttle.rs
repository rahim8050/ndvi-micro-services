use std::{
    num::NonZeroU32,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    response::{IntoResponse, Response},
    Json,
};
use futures_util::future::BoxFuture;
use governor::{clock::DefaultClock, state::keyed::DashMapStateStore, Quota, RateLimiter};
use http::{Request, StatusCode};
use serde_json::json;
use tower::Service;

use crate::{AuthContext, Envelope};

#[derive(Clone)]
pub struct ThrottleConfig {
    pub enabled: bool,
    pub anon_rate: String,
    pub user_rate: String,
    pub api_key_rate: String,
}

impl ThrottleConfig {
    pub fn from_env() -> Self {
        let enabled = std::env::var("THROTTLE_ENABLED")
            .map(|value| value != "0" && value.to_lowercase() != "false")
            .unwrap_or(true);
        let anon_rate =
            std::env::var("THROTTLE_ANON_RATE").unwrap_or_else(|_| "100/min".to_string());
        let user_rate =
            std::env::var("THROTTLE_USER_RATE").unwrap_or_else(|_| "1000/min".to_string());
        let api_key_rate =
            std::env::var("API_KEY_THROTTLE_RATE").unwrap_or_else(|_| "10/min".to_string());
        Self {
            enabled,
            anon_rate,
            user_rate,
            api_key_rate,
        }
    }
}

#[derive(Clone)]
pub struct ThrottleState {
    enabled: bool,
    anon: RateLimiter<String, DashMapStateStore<String>, DefaultClock>,
    user: RateLimiter<String, DashMapStateStore<String>, DefaultClock>,
    api_key: RateLimiter<String, DashMapStateStore<String>, DefaultClock>,
}

impl ThrottleState {
    pub fn from_env() -> Self {
        let config = ThrottleConfig::from_env();
        Self::new(config)
    }

    pub fn new(config: ThrottleConfig) -> Self {
        let anon = limiter_from_rate(&config.anon_rate);
        let user = limiter_from_rate(&config.user_rate);
        let api_key = limiter_from_rate(&config.api_key_rate);
        Self {
            enabled: config.enabled,
            anon,
            user,
            api_key,
        }
    }

    fn check_key(&self, key: &str, kind: ThrottleKind) -> bool {
        if !self.enabled {
            return true;
        }
        match kind {
            ThrottleKind::Anon => self.anon.check_key(&key.to_string()).is_ok(),
            ThrottleKind::User => self.user.check_key(&key.to_string()).is_ok(),
            ThrottleKind::ApiKey => self.api_key.check_key(&key.to_string()).is_ok(),
        }
    }
}

#[derive(Clone)]
pub struct ThrottleLayer {
    state: Arc<ThrottleState>,
}

impl ThrottleLayer {
    pub fn new(state: ThrottleState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }

    pub fn from_env() -> Self {
        Self::new(ThrottleState::from_env())
    }
}

impl<S> tower::Layer<S> for ThrottleLayer {
    type Service = ThrottleService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ThrottleService {
            inner,
            state: self.state.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ThrottleService<S> {
    inner: S,
    state: Arc<ThrottleState>,
}

impl<S, B> Service<Request<B>> for ThrottleService<S>
where
    S: Service<Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let mut inner = self.inner.clone();
        let state = self.state.clone();
        let auth = req.extensions().get::<AuthContext>().cloned();
        let key = throttle_key(&req, auth.as_ref());
        let kind = throttle_kind(auth.as_ref());

        Box::pin(async move {
            if !state.check_key(&key, kind) {
                let body =
                    Envelope::failure("Too many requests", Some(json!({"detail": "rate_limited"})));
                let response = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
                return Ok(response);
            }
            inner.call(req).await
        })
    }
}

#[derive(Copy, Clone)]
enum ThrottleKind {
    Anon,
    User,
    ApiKey,
}

fn throttle_kind(auth: Option<&AuthContext>) -> ThrottleKind {
    match auth {
        Some(AuthContext {
            kind: crate::AuthKind::Jwt { .. },
        }) => ThrottleKind::User,
        Some(AuthContext {
            kind: crate::AuthKind::ApiKey(_),
            ..
        }) => ThrottleKind::ApiKey,
        None => ThrottleKind::Anon,
    }
}

fn throttle_key<B>(req: &Request<B>, auth: Option<&AuthContext>) -> String {
    if let Some(auth) = auth {
        return auth.throttle_key();
    }
    if let Some(ip) = client_ip(req) {
        return format!("anon:{ip}");
    }
    "anon:unknown".to_string()
}

fn client_ip<B>(req: &Request<B>) -> Option<String> {
    if let Some(forwarded) = req.headers().get("x-forwarded-for") {
        if let Ok(value) = forwarded.to_str() {
            if let Some(first) = value.split(',').next() {
                let trimmed = first.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    req.headers()
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

fn limiter_from_rate(rate: &str) -> RateLimiter<String, DashMapStateStore<String>, DefaultClock> {
    let quota = parse_quota(rate).unwrap_or_else(|| Quota::per_minute(nonzero(100)));
    RateLimiter::keyed(quota)
}

fn parse_quota(rate: &str) -> Option<Quota> {
    let parts: Vec<&str> = rate.split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let count = parts[0].parse::<u32>().ok()?;
    let quota = nonzero(count);
    match parts[1] {
        "sec" | "second" | "seconds" => Some(Quota::per_second(quota)),
        "min" | "minute" | "minutes" => Some(Quota::per_minute(quota)),
        "hour" | "hours" => Some(Quota::per_hour(quota)),
        _ => None,
    }
}

fn nonzero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap_or_else(|| NonZeroU32::new(1).unwrap())
}
