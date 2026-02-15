use std::{env, net::SocketAddr};

use axum::Router;
use dotenvy::dotenv;
use tokio::net::TcpListener;
use tracing_subscriber::{fmt, EnvFilter};

mod db;
mod models;
mod routes;

#[tokio::main]
async fn main() {
    dotenv().ok();

    fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = db::create_pool(&database_url)
        .await
        .expect("failed to connect to database");

    let state = db::AppState { pool };
    let app: Router = routes::router(state);

    let addr: SocketAddr = "0.0.0.0:8080".parse().expect("valid address");
    tracing::info!(%addr, "starting server");

    let listener = TcpListener::bind(addr)
        .await
        .expect("failed to bind address");
    axum::serve(listener, app).await.expect("server failed");
}
