use axum::{
    extract::State,
    http::{header, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use ndvi_common::Envelope;
use serde_json::json;

use crate::{db::AppState, metrics, models::NdviInput};

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

async fn index() -> impl IntoResponse {
    let body = Envelope::success(
        "NDVI Service",
        json!({"message": "POST JSON to /api/v1/ndvi to ingest samples"}),
    );
    (StatusCode::OK, Json(body))
}

async fn healthz() -> impl IntoResponse {
    let body = Envelope::success("ok", json!({"status": "ok"}));
    (StatusCode::OK, Json(body))
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
    let body = Envelope::success("ok", json!({"endpoints": ["/api/v1/ndvi"]}));
    (StatusCode::OK, Json(body))
}

async fn ndvi_info() -> impl IntoResponse {
    let body = Envelope::success(
        "ok",
        json!({"message": "POST JSON to /api/v1/ndvi to ingest NDVI samples"}),
    );
    (StatusCode::OK, Json(body))
}

async fn create_ndvi(State(state): State<AppState>, Json(payload): Json<NdviInput>) -> Response {
    if let Err(message) = payload.validate() {
        let body = Envelope::failure("Validation error", Some(json!({ "detail": message })));
        return (StatusCode::BAD_REQUEST, Json(body)).into_response();
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
        Ok(_) => {
            let body = Envelope::success("Created", json!({"status": "created"}));
            (StatusCode::CREATED, Json(body)).into_response()
        }
        Err(err) => {
            tracing::error!(error = ?err, "failed to insert ndvi sample");
            let body =
                Envelope::failure("Database error", Some(json!({"detail": "insert_failed"})));
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        }
    }
}
