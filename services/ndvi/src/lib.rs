use std::{env, net::SocketAddr, sync::Arc};

use axum::{middleware, Router};
use dotenvy::dotenv;
use ndvi_common::auth::{ApiKeyConfig, AuthState, MySqlApiKeyValidator};
use ndvi_common::throttle::ThrottleLayer;
use sqlx::mysql::MySqlPoolOptions;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tracing_subscriber::{fmt, EnvFilter};

pub mod db;
pub mod metrics;
pub mod models;
pub mod routes;

pub async fn run() {
    dotenv().ok();

    fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = db::create_pool(&database_url)
        .await
        .expect("failed to connect to database");

    let api_key_validator = match env::var("MYSQL_DATABASE_URL") {
        Ok(mysql_url) => {
            let mysql_pool = MySqlPoolOptions::new()
                .max_connections(5)
                .connect(&mysql_url)
                .await
                .expect("failed to connect to mysql for api key validation");
            let config = ApiKeyConfig::from_env().expect("DJANGO_API_KEY_PEPPER must be set");
            Some(Arc::new(MySqlApiKeyValidator {
                pool: mysql_pool,
                config,
            }))
        }
        Err(_) => None,
    };
    let auth_state = AuthState::from_env(api_key_validator).expect("failed to configure auth");
    let throttle_layer = ThrottleLayer::from_env();

    let state = db::AppState { pool };
    let app: Router =
        routes::router(state).layer(ServiceBuilder::new().layer(throttle_layer).layer(
            middleware::from_fn_with_state(auth_state, ndvi_common::auth::auth_middleware),
        ));

    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8081);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(%addr, "starting server");

    let listener = TcpListener::bind(addr)
        .await
        .expect("failed to bind address");
    axum::serve(listener, app).await.expect("server failed");
}
