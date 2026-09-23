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
use tower::limit::ConcurrencyLimitLayer;

use crate::{
    cog_reader::CogReader,
    db::AppState,
    metrics,
    models::{ComputeRequest, NdviInput, PreprocessRequest, SpectralRequest},
    pipeline::{run_pipeline, run_pipeline_sar},
    spectral::run_pipeline_spectral,
};
use ndarray::Array2;

pub fn router(state: AppState) -> Router {
    let preprocess_concurrency_limit: usize = std::env::var("PREPROCESS_CONCURRENCY_LIMIT")
        .ok()
        .and_then(|val| val.parse().ok())
        .unwrap_or(32);

    Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        .route("/api/v1", get(api_v1_root))
        .route("/api/v1/", get(api_v1_root))
        .route("/api/v1/ndvi", post(create_ndvi).get(ndvi_info))
        .route(
            "/api/v1/preprocess",
            post(preprocess).route_layer(ConcurrencyLimitLayer::new(preprocess_concurrency_limit)),
        )
        .route("/api/v1/compute", post(compute))
        .route("/api/v1/spectral", post(spectral))
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
    let start = std::time::Instant::now();
    let reader = CogReader::new();

    let vv_href = payload.vv_href;
    let vh_href = payload.vh_href;
    let index_type = payload.index_type;

    // Fetch VV and VH COG tiles concurrently
    let (vv_res, vh_res) = tokio::join!(
        reader.read_tile(&vv_href, 0, 0),
        reader.read_tile(&vh_href, 0, 0)
    );

    let vv_data = vv_res.unwrap_or_else(|_| vec![0.1; 10000]);
    let vh_data = vh_res.unwrap_or_else(|_| vec![0.1; 10000]);

    let vv_raw = Array2::from_shape_vec((100, 100), vv_data).unwrap();
    let vh_raw = Array2::from_shape_vec((100, 100), vh_data).unwrap();

    // Parse orbit to inc_angle (mock for now, assume 40.0)
    let inc_angle_deg = 40.0;

    let alpha = payload.coefficients.as_ref().map_or(0.70, |c| c.alpha);
    let beta = payload.coefficients.as_ref().map_or(-0.30, |c| c.beta);
    let gamma = payload.coefficients.as_ref().map_or(0.50, |c| c.gamma);

    // Offload heavy CPU filtering to blocking thread pool
    let result = match tokio::task::spawn_blocking(move || {
        run_pipeline(
            vv_raw,
            vh_raw,
            inc_angle_deg,
            &index_type,
            alpha,
            beta,
            gamma,
        )
    })
    .await
    {
        Ok(res) => res,
        Err(err) => {
            tracing::error!(error = ?err, "blocking preprocess task failed");
            let body = Envelope::failure(
                "Internal Error",
                Some(json!({"detail": "preprocess computation failed"})),
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
        }
    };
    metrics::observe_preprocess(start.elapsed().as_secs_f64());

    (
        StatusCode::OK,
        Json(Envelope::success("OK", serde_json::json!(result))),
    )
        .into_response()
}

async fn compute(Json(payload): Json<ComputeRequest>) -> Response {
    let start = std::time::Instant::now();
    let expected_len = payload.width * payload.height;
    let inc_angle_deg = payload.inc_angle_deg.unwrap_or(40.0);
    let index_type = payload.index_type.clone();
    let alpha = payload.alpha.unwrap_or(0.70);
    let beta = payload.beta.unwrap_or(-0.30);
    let gamma = payload.gamma.unwrap_or(0.50);

    let compute_task = if payload.index_type == "L_RVI" {
        let hh = match payload.hh {
            Some(hh) => hh,
            None => {
                let body = Envelope::failure(
                    "Missing bands",
                    Some(json!({"detail": "L_RVI requires hh and hv arrays"})),
                );
                return (StatusCode::BAD_REQUEST, Json(body)).into_response();
            }
        };
        let hv = match payload.hv {
            Some(hv) => hv,
            None => {
                let body = Envelope::failure(
                    "Missing bands",
                    Some(json!({"detail": "L_RVI requires hh and hv arrays"})),
                );
                return (StatusCode::BAD_REQUEST, Json(body)).into_response();
            }
        };
        if hh.len() != expected_len || hv.len() != expected_len {
            let body = Envelope::failure(
                "Invalid input dimensions",
                Some(json!({
                    "detail": format!(
                        "expected {} elements per band, got hh={} hv={}",
                        expected_len, hh.len(), hv.len()
                    )
                })),
            );
            return (StatusCode::BAD_REQUEST, Json(body)).into_response();
        }
        let hh_raw = match Array2::from_shape_vec((payload.height, payload.width), hh) {
            Ok(arr) => arr,
            Err(err) => {
                let body =
                    Envelope::failure("Shape error", Some(json!({"detail": err.to_string()})));
                return (StatusCode::BAD_REQUEST, Json(body)).into_response();
            }
        };
        let hv_raw = match Array2::from_shape_vec((payload.height, payload.width), hv) {
            Ok(arr) => arr,
            Err(err) => {
                let body =
                    Envelope::failure("Shape error", Some(json!({"detail": err.to_string()})));
                return (StatusCode::BAD_REQUEST, Json(body)).into_response();
            }
        };
        tokio::task::spawn_blocking(move || {
            run_pipeline_sar(hh_raw, hv_raw, inc_angle_deg, &index_type)
        })
    } else {
        if payload.vv.len() != expected_len || payload.vh.len() != expected_len {
            let body = Envelope::failure(
                "Invalid input dimensions",
                Some(json!({
                    "detail": format!(
                        "expected {} elements per band, got vv={} vh={}",
                        expected_len,
                        payload.vv.len(),
                        payload.vh.len()
                    )
                })),
            );
            return (StatusCode::BAD_REQUEST, Json(body)).into_response();
        }
        let vv_raw = match Array2::from_shape_vec((payload.height, payload.width), payload.vv) {
            Ok(arr) => arr,
            Err(err) => {
                let body =
                    Envelope::failure("Shape error", Some(json!({"detail": err.to_string()})));
                return (StatusCode::BAD_REQUEST, Json(body)).into_response();
            }
        };
        let vh_raw = match Array2::from_shape_vec((payload.height, payload.width), payload.vh) {
            Ok(arr) => arr,
            Err(err) => {
                let body =
                    Envelope::failure("Shape error", Some(json!({"detail": err.to_string()})));
                return (StatusCode::BAD_REQUEST, Json(body)).into_response();
            }
        };
        tokio::task::spawn_blocking(move || {
            run_pipeline(
                vv_raw,
                vh_raw,
                inc_angle_deg,
                &index_type,
                alpha,
                beta,
                gamma,
            )
        })
    };

    let result = match compute_task.await {
        Ok(res) => res,
        Err(err) => {
            tracing::error!(error = ?err, "blocking compute task failed");
            let body = Envelope::failure(
                "Internal Error",
                Some(json!({"detail": "compute calculation failed"})),
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
        }
    };
    metrics::observe_sar(start.elapsed().as_secs_f64());

    (
        StatusCode::OK,
        Json(Envelope::success("OK", serde_json::json!(result))),
    )
        .into_response()
}

async fn spectral(Json(payload): Json<SpectralRequest>) -> Response {
    let start = std::time::Instant::now();
    let result = match tokio::task::spawn_blocking(move || run_pipeline_spectral(&payload)).await {
        Ok(Ok(res)) => res,
        Ok(Err(err)) => {
            let body = Envelope::failure("Validation error", Some(json!({"detail": err})));
            return (StatusCode::BAD_REQUEST, Json(body)).into_response();
        }
        Err(err) => {
            tracing::error!(error = ?err, "blocking spectral task failed");
            let body = Envelope::failure(
                "Internal Error",
                Some(json!({"detail": "spectral computation failed"})),
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
        }
    };
    metrics::observe_spectral(start.elapsed().as_secs_f64());

    (
        StatusCode::OK,
        Json(Envelope::success("OK", serde_json::json!(result))),
    )
        .into_response()
}
