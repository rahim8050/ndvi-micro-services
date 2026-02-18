use std::{env, net::SocketAddr, sync::Arc};

use axum::{middleware, Router};
use dotenvy::dotenv;
use ndvi_common::auth::{ApiKeyConfig, AuthState, MySqlApiKeyValidator};
use ndvi_common::throttle::ThrottleLayer;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tracing_subscriber::{fmt, EnvFilter};

mod db;
mod routes;

#[tokio::main]
async fn main() {
    dotenv().ok();

    fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let database_url = env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL must be set");
    let pool = db::create_pool(&database_url)
        .await
        .expect("failed to connect to database");

    let config = ApiKeyConfig::from_env().expect("DJANGO_API_KEY_PEPPER must be set");
    let api_key_validator = Some(Arc::new(MySqlApiKeyValidator {
        pool: pool.clone(),
        config,
    }));
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
        .unwrap_or(8090);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(%addr, "starting weather service");

    let listener = TcpListener::bind(addr)
        .await
        .expect("failed to bind address");
    axum::serve(listener, app).await.expect("server failed");
}
