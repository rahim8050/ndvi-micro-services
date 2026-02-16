use axum::{
    extract::State,
    http::{header, StatusCode},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;

use crate::{db::AppState, metrics, models::NdviInput};

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        .route("/api/v1", get(api_v1_root))
        .route("/api/v1/", get(api_v1_root))
        .route("/api/v1/ndvi", post(create_ndvi).get(ndvi_info))
        .route_layer(middleware::from_fn(metrics::metrics_middleware))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8"/>
    <title>NDVI Service</title>
  </head>
  <body>
    <h1>NDVI Service</h1>
    <p>POST JSON to <code>/api/v1/ndvi</code> to ingest samples.</p>
  </body>
</html>"#,
    )
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

async fn metrics_handler() -> Response {
    match metrics::render_metrics() {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, metrics::METRICS_CONTENT_TYPE)],
            body,
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "failed to render metrics");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn api_v1_root() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "endpoints": ["/api/v1/ndvi"]
        })),
    )
}

async fn ndvi_info() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "message": "POST JSON to /api/v1/ndvi to ingest NDVI samples"
        })),
    )
}

async fn create_ndvi(State(state): State<AppState>, Json(payload): Json<NdviInput>) -> Response {
    if let Err(message) = payload.validate() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse { error: message }),
        )
            .into_response();
    }

    let NdviInput {
        farm_id,
        timestamp,
        mean,
        min,
        max,
        source,
        geometry,
    } = payload;

    let result = sqlx::query(
        "INSERT INTO ndvi_samples (farm_id, timestamp, mean, min, max, source, geometry) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(farm_id)
    .bind(timestamp)
    .bind(mean)
    .bind(min)
    .bind(max)
    .bind(source.as_deref())
    .bind(geometry)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "failed to insert ndvi sample");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "database error".to_string(),
                }),
            )
                .into_response()
        }
    }
}
