use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use ndvi_common::Envelope;
use serde_json::json;

use crate::db::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/api/v1/weather/current/", get(not_implemented))
        .route("/api/v1/weather/daily/", get(not_implemented))
        .route("/api/v1/weather/weekly/", get(not_implemented))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    let envelope = Envelope::success("ok", json!({"status": "ok"}));
    (StatusCode::OK, Json(envelope))
}

async fn metrics() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        "weather_service_up 1\n",
    )
}

async fn not_implemented() -> Response {
    let envelope = Envelope::failure(
        "Not Implemented",
        Some(json!({"detail": "migration pending"})),
    );
    (StatusCode::NOT_IMPLEMENTED, Json(envelope)).into_response()
}
