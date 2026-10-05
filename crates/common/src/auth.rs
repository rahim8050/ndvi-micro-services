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
/// Header carrying the Django gateway's service-to-service token.
pub const SERVICE_AUTH_HEADER: &str = "x-service-authorization";
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

/// Identity expected on tokens minted by the Django gateway for internal calls.
/// Uses the same signing key as [`JwtConfig`]; only iss/aud differ.
#[derive(Debug, Clone)]
pub struct ServiceJwtConfig {
    pub issuer: String,
    pub audience: String,
}

impl ServiceJwtConfig {
    pub fn from_env() -> Self {
        let issuer =
            std::env::var("SERVICE_JWT_ISSUER").unwrap_or_else(|_| "django-gateway".to_string());
        let audience =
            std::env::var("SERVICE_JWT_AUDIENCE").unwrap_or_else(|_| "internal-rust".to_string());
        Self { issuer, audience }
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
    Jwt {
        subject: String,
    },
    ApiKey(ApiKeyInfo),
    /// Authenticated gateway service (Django → Rust internal hop).
    Service {
        subject: String,
        orig_subject: Option<String>,
    },
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
            // subject is already "service:<name>"; keep the key verbatim.
            AuthKind::Service { subject, .. } => subject.clone(),
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
    #[allow(dead_code)]
    exp: usize,
}

#[derive(Debug, Deserialize)]
struct ServiceClaims {
    sub: String,
    #[allow(dead_code)]
    exp: usize,
    /// Original end-client subject, forwarded for audit purposes.
    #[serde(default)]
    orig_sub: Option<String>,
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

/// Validate the Django gateway's service token (stateless, HS256, local key).
/// Requires the service-specific `iss`/`aud`; client JWTs cannot satisfy this
/// and service tokens cannot satisfy [`validate_jwt`] when iss/aud are set.
pub fn validate_service_jwt(
    token: &str,
    jwt_config: &JwtConfig,
    service_config: &ServiceJwtConfig,
) -> Result<AuthContext, AuthError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[service_config.issuer.as_str()]);
    validation.set_audience(&[service_config.audience.as_str()]);

    let key = DecodingKey::from_secret(jwt_config.signing_key.as_bytes());
    let data = decode::<ServiceClaims>(token, &key, &validation)
        .map_err(|_| AuthError::Invalid("invalid_service_jwt"))?;

    Ok(AuthContext {
        kind: AuthKind::Service {
            subject: data.claims.sub,
            orig_subject: data.claims.orig_sub,
        },
    })
}

#[allow(clippy::double_must_use)]
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
    service_header: Option<&str>,
    auth_header: Option<&str>,
    api_key_header: Option<&str>,
    jwt_config: &JwtConfig,
    service_config: &ServiceJwtConfig,
    api_key_validator: Option<&dyn ApiKeyValidator>,
) -> Result<AuthContext, AuthError> {
    // Internal service token takes precedence; on any failure fall through to
    // the client credential so rollout stays zero-downtime.
    if let Some(header) = service_header {
        if let Some(token) = parse_bearer_token(header) {
            if let Ok(context) = validate_service_jwt(&token, jwt_config, service_config) {
                return Ok(context);
            }
        }
    }

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

pub fn service_auth_header(req: &http::Request<Body>) -> Option<String> {
    header_from_request(req, SERVICE_AUTH_HEADER)
}

#[derive(Clone)]
pub struct AuthState {
    pub enabled: bool,
    pub jwt: Option<JwtConfig>,
    pub service: ServiceJwtConfig,
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
                service: ServiceJwtConfig::from_env(),
                api_key_validator,
            });
        }
        let jwt = JwtConfig::from_env()?;
        Ok(Self {
            enabled: true,
            jwt: Some(jwt),
            service: ServiceJwtConfig::from_env(),
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

    let service_header = service_auth_header(&req);
    let auth_header = auth_header(&req);
    let api_key_header = api_key_header(&req);

    let result = authenticate_request(
        service_header.as_deref(),
        auth_header.as_deref(),
        api_key_header.as_deref(),
        jwt_config,
        &state.service,
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
    paths.contains(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::throttle::{ThrottleConfig, ThrottleLayer, ThrottleState};
    use axum::{middleware, routing::get, Router};
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde::Serialize;
    use tower::{ServiceBuilder, ServiceExt};

    const SECRET: &str = "test-signing-key-0123456789abcdef0123456789abcdef";

    #[derive(Serialize)]
    struct ServiceTestClaims {
        sub: String,
        iss: String,
        aud: String,
        exp: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        orig_sub: Option<String>,
    }

    #[derive(Serialize)]
    struct ClientTestClaims {
        sub: String,
        exp: usize,
    }

    fn jwt_config() -> JwtConfig {
        JwtConfig {
            signing_key: SECRET.to_string(),
            issuer: None,
            audience: None,
        }
    }

    fn service_config() -> ServiceJwtConfig {
        ServiceJwtConfig {
            issuer: "django-gateway".to_string(),
            audience: "internal-rust".to_string(),
        }
    }

    fn sign<T: Serialize>(claims: &T) -> String {
        encode(
            &Header::new(Algorithm::HS256),
            claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .expect("token encodes")
    }

    fn mint_service(iss: &str, aud: &str, exp: usize, orig_sub: Option<&str>) -> String {
        sign(&ServiceTestClaims {
            sub: "service:django".to_string(),
            iss: iss.to_string(),
            aud: aud.to_string(),
            exp,
            orig_sub: orig_sub.map(str::to_string),
        })
    }

    fn future() -> usize {
        Utc::now().timestamp() as usize + 300
    }

    fn past() -> usize {
        Utc::now().timestamp() as usize - 300
    }

    #[test]
    fn valid_service_token_yields_service_kind() {
        let token = mint_service("django-gateway", "internal-rust", future(), Some("42"));
        let ctx = validate_service_jwt(&token, &jwt_config(), &service_config())
            .expect("valid service token");
        assert_eq!(ctx.throttle_key(), "service:django");
        match ctx.kind {
            AuthKind::Service {
                subject,
                orig_subject,
            } => {
                assert_eq!(subject, "service:django");
                assert_eq!(orig_subject.as_deref(), Some("42"));
            }
            other => panic!("expected Service kind, got {other:?}"),
        }
    }

    #[test]
    fn service_token_rejects_wrong_issuer_audience_and_expiry() {
        let wrong_iss = mint_service("evil", "internal-rust", future(), None);
        assert!(validate_service_jwt(&wrong_iss, &jwt_config(), &service_config()).is_err());

        let wrong_aud = mint_service("django-gateway", "other-aud", future(), None);
        assert!(validate_service_jwt(&wrong_aud, &jwt_config(), &service_config()).is_err());

        let expired = mint_service("django-gateway", "internal-rust", past(), None);
        assert!(validate_service_jwt(&expired, &jwt_config(), &service_config()).is_err());

        let wrong_key = encode(
            &Header::new(Algorithm::HS256),
            &ServiceTestClaims {
                sub: "service:django".to_string(),
                iss: "django-gateway".to_string(),
                aud: "internal-rust".to_string(),
                exp: future(),
                orig_sub: None,
            },
            &EncodingKey::from_secret(b"not-the-configured-key"),
        )
        .expect("token encodes");
        assert!(validate_service_jwt(&wrong_key, &jwt_config(), &service_config()).is_err());
    }

    #[tokio::test]
    async fn service_header_takes_precedence_over_client_credential() {
        let service_token = mint_service("django-gateway", "internal-rust", future(), None);
        let client_token = sign(&ClientTestClaims {
            sub: "user-1".to_string(),
            exp: future(),
        });

        let ctx = authenticate_request(
            Some(&format!("Bearer {service_token}")),
            Some(&format!("Bearer {client_token}")),
            None,
            &jwt_config(),
            &service_config(),
            None,
        )
        .await
        .expect("authenticates");

        assert!(matches!(ctx.kind, AuthKind::Service { .. }));
    }

    #[tokio::test]
    async fn invalid_service_token_falls_back_to_client_credential() {
        let bad_service_token = mint_service("wrong-issuer", "internal-rust", future(), None);
        let client_token = sign(&ClientTestClaims {
            sub: "user-1".to_string(),
            exp: future(),
        });

        let ctx = authenticate_request(
            Some(&format!("Bearer {bad_service_token}")),
            Some(&format!("Bearer {client_token}")),
            None,
            &jwt_config(),
            &service_config(),
            None,
        )
        .await
        .expect("falls back to client JWT");

        assert!(matches!(
            ctx.kind,
            AuthKind::Jwt { ref subject } if subject == "user-1"
        ));
    }

    #[tokio::test]
    async fn no_credentials_at_all_is_missing() {
        let err = authenticate_request(None, None, None, &jwt_config(), &service_config(), None)
            .await
            .expect_err("missing credentials");
        assert!(matches!(err, AuthError::Missing));
    }

    async fn service_handler(req: Request<Body>) -> Response {
        let seen = req
            .extensions()
            .get::<AuthContext>()
            .map(|ctx| matches!(ctx.kind, AuthKind::Service { .. }))
            .unwrap_or(false);
        if seen {
            StatusCode::OK.into_response()
        } else {
            StatusCode::IM_A_TEAPOT.into_response()
        }
    }

    fn layering_app() -> Router {
        let state = AuthState {
            enabled: true,
            jwt: Some(jwt_config()),
            service: service_config(),
            api_key_validator: None,
        };
        let throttle = ThrottleLayer::new(ThrottleState::new(ThrottleConfig {
            enabled: true,
            anon_rate: "100/min".to_string(),
            user_rate: "1000/min".to_string(),
            api_key_rate: "600/min".to_string(),
            service_rate: "6000/min".to_string(),
        }));
        // Same order as services: auth outer, throttle inner.
        Router::new().route("/", get(service_handler)).layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn_with_state(state, auth_middleware))
                .layer(throttle),
        )
    }

    #[tokio::test]
    async fn inner_throttle_layer_sees_auth_context_after_layer_swap() {
        let token = mint_service("django-gateway", "internal-rust", future(), None);
        let request = Request::builder()
            .uri("/")
            .header(SERVICE_AUTH_HEADER, format!("Bearer {token}"))
            .body(Body::empty())
            .expect("request builds");

        let response = layering_app().oneshot(request).await.expect("served");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unauthenticated_burst_never_hits_throttle_when_auth_is_outer() {
        // With auth outer, every unauthenticated request is rejected with 401
        // before the inner throttle layer counts it; with the old (buggy)
        // order the anon bucket would return 429 from request #101.
        let app = layering_app();
        for i in 0..150 {
            let request = Request::builder()
                .uri("/")
                .body(Body::empty())
                .expect("request builds");
            let response = app.clone().oneshot(request).await.expect("served");
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "request {i} should 401, not be throttled"
            );
        }
    }
}
