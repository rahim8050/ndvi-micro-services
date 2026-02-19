use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use pbkdf2::pbkdf2_hmac;
use serde::Deserialize;
use sha2::Sha256;
use sqlx::{MySqlPool, Row};
use std::sync::Arc;
use subtle::ConstantTimeEq;

use crate::Envelope;
use axum::{
    body::Body,
    extract::State,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use http::{Request, StatusCode};
use serde_json::json;

pub const API_KEY_PREFIX: &str = "wk_live_";
const PREFIX_LENGTH: usize = 12;
const LAST_USED_AT_WRITE_MINUTES: i64 = 5;

#[derive(Debug, Clone)]
pub struct JwtConfig {
    pub signing_key: String,
    pub issuer: Option<String>,
    pub audience: Option<String>,
}

impl JwtConfig {
    pub fn from_env() -> Result<Self, AuthError> {
        let signing_key = std::env::var("JWT_SIGNING_KEY")
            .map_err(|_| AuthError::Misconfigured("JWT_SIGNING_KEY is required"))?;
        let issuer = std::env::var("JWT_ISSUER").ok();
        let audience = std::env::var("JWT_AUDIENCE").ok();
        Ok(Self {
            signing_key,
            issuer,
            audience,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ApiKeyConfig {
    pub pepper: String,
}

impl ApiKeyConfig {
    pub fn from_env() -> Result<Self, AuthError> {
        let pepper = std::env::var("DJANGO_API_KEY_PEPPER")
            .map_err(|_| AuthError::Misconfigured("DJANGO_API_KEY_PEPPER is required"))?;
        Ok(Self { pepper })
    }
}

#[derive(Debug, Clone)]
pub struct ApiKeyInfo {
    pub key_id: String,
    pub user_id: i64,
    pub scope: String,
}

#[derive(Debug, Clone)]
pub enum AuthKind {
    Jwt { subject: String },
    ApiKey(ApiKeyInfo),
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub kind: AuthKind,
}

impl AuthContext {
    pub fn throttle_key(&self) -> String {
        match &self.kind {
            AuthKind::Jwt { subject } => format!("user:{subject}"),
            AuthKind::ApiKey(info) => format!("api_key:{}", info.key_id),
        }
    }
}

#[derive(Debug)]
pub enum AuthError {
    Missing,
    Invalid(&'static str),
    Misconfigured(&'static str),
    Internal(&'static str),
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

pub fn parse_bearer_token(header: &str) -> Option<String> {
    let header = header.trim();
    if !header.starts_with("Bearer ") {
        return None;
    }
    let token = header.trim_start_matches("Bearer ").trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

pub fn validate_jwt(token: &str, config: &JwtConfig) -> Result<AuthContext, AuthError> {
    let mut validation = Validation::new(Algorithm::HS256);
    if let Some(issuer) = &config.issuer {
        validation.set_issuer(&[issuer.as_str()]);
    }
    if let Some(audience) = &config.audience {
        validation.set_audience(&[audience.as_str()]);
    }

    let key = DecodingKey::from_secret(config.signing_key.as_bytes());
    let data = decode::<Claims>(token, &key, &validation)
        .map_err(|_| AuthError::Invalid("invalid_jwt"))?;

    Ok(AuthContext {
        kind: AuthKind::Jwt {
            subject: data.claims.sub,
        },
    })
}

#[async_trait]
pub trait ApiKeyValidator: Send + Sync {
    async fn validate(&self, raw_key: &str) -> Result<AuthContext, AuthError>;
}

#[derive(Clone)]
pub struct MySqlApiKeyValidator {
    pub pool: MySqlPool,
    pub config: ApiKeyConfig,
}

#[async_trait]
impl ApiKeyValidator for MySqlApiKeyValidator {
    async fn validate(&self, raw_key: &str) -> Result<AuthContext, AuthError> {
        validate_api_key_mysql(&self.pool, raw_key, &self.config).await
    }
}

fn verify_pbkdf2_sha256(hash: &str, secret: &str) -> bool {
    let parts: Vec<&str> = hash.split('$').collect();
    if parts.len() != 4 {
        return false;
    }
    let algorithm = parts[0];
    if algorithm != "pbkdf2_sha256" {
        return false;
    }
    let iterations: u32 = match parts[1].parse() {
        Ok(value) => value,
        Err(_) => return false,
    };
    let salt = parts[2];
    let expected = parts[3];

    let mut output = [0u8; 32];
    pbkdf2_hmac::<Sha256>(secret.as_bytes(), salt.as_bytes(), iterations, &mut output);
    let computed = BASE64.encode(output);
    computed.as_bytes().ct_eq(expected.as_bytes()).into()
}

pub async fn validate_api_key_mysql(
    pool: &MySqlPool,
    raw_key: &str,
    config: &ApiKeyConfig,
) -> Result<AuthContext, AuthError> {
    if !raw_key.starts_with(API_KEY_PREFIX) || raw_key.len() < PREFIX_LENGTH + 4 {
        return Err(AuthError::Invalid("invalid_api_key"));
    }

    let prefix = &raw_key[..PREFIX_LENGTH];
    let last4 = &raw_key[raw_key.len() - 4..];
    let peppered = format!("{}:{}", config.pepper, raw_key);

    let rows = sqlx::query(
        "SELECT k.id, k.user_id, k.key_hash, k.revoked_at, k.expires_at, k.scope, u.is_active \
         FROM api_keys_apikey k \
         JOIN auth_user u ON u.id = k.user_id \
         WHERE k.prefix = ? AND k.last4 = ?",
    )
    .bind(prefix)
    .bind(last4)
    .fetch_all(pool)
    .await
    .map_err(|_| AuthError::Internal("api_key_query_failed"))?;

    if rows.is_empty() {
        return Err(AuthError::Invalid("invalid_api_key"));
    }

    let now = Utc::now();
    let mut matched: Option<ApiKeyInfo> = None;
    for row in rows {
        let key_hash: String = row.try_get("key_hash").unwrap_or_default();
        if !verify_pbkdf2_sha256(&key_hash, &peppered) {
            continue;
        }

        let revoked_at: Option<DateTime<Utc>> = row.try_get("revoked_at").ok();
        if revoked_at.is_some() {
            return Err(AuthError::Invalid("api_key_revoked"));
        }

        let expires_at: Option<DateTime<Utc>> = row.try_get("expires_at").ok();
        if let Some(exp) = expires_at {
            if exp <= now {
                return Err(AuthError::Invalid("api_key_expired"));
            }
        }

        let is_active: Option<i8> = row.try_get("is_active").ok();
        if let Some(active) = is_active {
            if active == 0 {
                return Err(AuthError::Invalid("user_inactive"));
            }
        }

        let key_id: String = row.try_get("id").unwrap_or_default();
        let user_id: i64 = row.try_get("user_id").unwrap_or_default();
        let scope: String = row.try_get("scope").unwrap_or_else(|_| "read".to_string());

        matched = Some(ApiKeyInfo {
            key_id,
            user_id,
            scope,
        });
        break;
    }

    let info = matched.ok_or(AuthError::Invalid("invalid_api_key"))?;
    update_last_used(pool, &info, now).await;

    Ok(AuthContext {
        kind: AuthKind::ApiKey(info),
    })
}

async fn update_last_used(pool: &MySqlPool, info: &ApiKeyInfo, now: DateTime<Utc>) {
    let cutoff = now - chrono::Duration::minutes(LAST_USED_AT_WRITE_MINUTES);
    let _ = sqlx::query(
        "UPDATE api_keys_apikey \
         SET last_used_at = ? \
         WHERE id = ? AND (last_used_at IS NULL OR last_used_at < ?)",
    )
    .bind(now)
    .bind(&info.key_id)
    .bind(cutoff)
    .execute(pool)
    .await;
}

pub async fn authenticate_request(
    auth_header: Option<&str>,
    api_key_header: Option<&str>,
    jwt_config: &JwtConfig,
    api_key_validator: Option<&dyn ApiKeyValidator>,
) -> Result<AuthContext, AuthError> {
    if let Some(header) = auth_header {
        if let Some(token) = parse_bearer_token(header) {
            return validate_jwt(&token, jwt_config);
        }
    }

    if let Some(api_key) = api_key_header {
        let validator = api_key_validator.ok_or(AuthError::Misconfigured(
            "api_key_validation_not_configured",
        ))?;
        return validator.validate(api_key).await;
    }

    Err(AuthError::Missing)
}

pub fn header_from_request(req: &http::Request<Body>, name: &str) -> Option<String> {
    req.headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

pub fn auth_header(req: &http::Request<Body>) -> Option<String> {
    header_from_request(req, http::header::AUTHORIZATION.as_str())
}

pub fn api_key_header(req: &http::Request<Body>) -> Option<String> {
    header_from_request(req, "x-api-key")
}

#[derive(Clone)]
pub struct AuthState {
    pub enabled: bool,
    pub jwt: Option<JwtConfig>,
    pub api_key_validator: Option<Arc<dyn ApiKeyValidator>>,
}

impl AuthState {
    pub fn from_env(
        api_key_validator: Option<Arc<dyn ApiKeyValidator>>,
    ) -> Result<Self, AuthError> {
        let disabled = std::env::var("AUTH_DISABLED")
            .map(|value| value == "1" || value.to_lowercase() == "true")
            .unwrap_or(false);
        if disabled {
            return Ok(Self {
                enabled: false,
                jwt: None,
                api_key_validator,
            });
        }
        let jwt = JwtConfig::from_env()?;
        Ok(Self {
            enabled: true,
            jwt: Some(jwt),
            api_key_validator,
        })
    }
}

pub async fn auth_middleware(
    State(state): State<AuthState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    if !state.enabled {
        return next.run(req).await;
    }

    let path = req.uri().path();
    if is_bypass_path(path) {
        return next.run(req).await;
    }

    let jwt_config = match &state.jwt {
        Some(config) => config,
        None => {
            let body =
                Envelope::failure("Auth misconfigured", Some(json!({"detail": "jwt_missing"})));
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
        }
    };

    let auth_header = auth_header(&req);
    let api_key_header = api_key_header(&req);

    let result = authenticate_request(
        auth_header.as_deref(),
        api_key_header.as_deref(),
        jwt_config,
        state.api_key_validator.as_deref(),
    )
    .await;

    match result {
        Ok(context) => {
            req.extensions_mut().insert(context);
            next.run(req).await
        }
        Err(AuthError::Missing) | Err(AuthError::Invalid(_)) => {
            let body = Envelope::failure("Unauthorized", None);
            (StatusCode::UNAUTHORIZED, Json(body)).into_response()
        }
        Err(AuthError::Misconfigured(detail)) => {
            let body = Envelope::failure("Auth misconfigured", Some(json!({ "detail": detail })));
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        }
        Err(AuthError::Internal(detail)) => {
            let body = Envelope::failure("Auth error", Some(json!({ "detail": detail })));
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        }
    }
}

fn is_bypass_path(path: &str) -> bool {
    let extra = std::env::var("AUTH_BYPASS_PATHS").unwrap_or_default();
    let paths: Vec<&str> = extra
        .split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    paths.iter().any(|p| *p == path)
}
