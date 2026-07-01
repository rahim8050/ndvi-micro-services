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

use crate::{db::AppState, metrics, models::{NdviInput, PreprocessRequest}, cog_reader::CogReader, pipeline::run_pipeline};
use ndarray::Array2;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        .route("/api/v1", get(api_v1_root))
        .route("/api/v1/", get(api_v1_root))
        .route("/api/v1/ndvi", post(create_ndvi).get(ndvi_info))
        .route("/api/v1/preprocess", post(preprocess))
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

async fn preprocess(Json(payload): Json<PreprocessRequest>) -> Response {
    let reader = CogReader::new();
    
    // In a real app we'd convert lat/lon bbox to tile x/y
    // For now we mock the read
    let vv_data = reader.read_tile(&payload.vv_href, 0, 0).await.unwrap_or_else(|_| vec![0.1; 10000]);
    let vh_data = reader.read_tile(&payload.vh_href, 0, 0).await.unwrap_or_else(|_| vec![0.1; 10000]);
    
    let vv_raw = Array2::from_shape_vec((100, 100), vv_data).unwrap();
    let vh_raw = Array2::from_shape_vec((100, 100), vh_data).unwrap();
    
    // Parse orbit to inc_angle (mock for now, assume 40.0)
    let inc_angle_deg = 40.0;
    
    let result = run_pipeline(vv_raw, vh_raw, inc_angle_deg, &payload.index_type);
    
    (StatusCode::OK, Json(Envelope::success("OK", serde_json::json!(result)))).into_response()
}
