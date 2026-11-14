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
    models::{
        ComputeRequest, NdviInput, PreprocessRequest, SpectralRequest, StatsRequest, StatsResponse,
    },
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
        .route("/api/v1/stats", post(stats))
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
            None,
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

    // Optional farm-polygon mask (True = inside). Validate length up
    // front and materialize as a row-major Array2; absence keeps the
    // legacy whole-bbox behaviour.
    let geometry_mask = payload.mask;
    if let Some(ref m) = geometry_mask {
        if m.len() != expected_len {
            let body = Envelope::failure(
                "Invalid input dimensions",
                Some(json!({
                    "detail": format!(
                        "expected {} elements for mask, got {}",
                        expected_len,
                        m.len()
                    )
                })),
            );
            return (StatusCode::BAD_REQUEST, Json(body)).into_response();
        }
    }
    let mask_arr = match geometry_mask {
        Some(m) => match Array2::from_shape_vec((payload.height, payload.width), m) {
            Ok(arr) => Some(arr),
            Err(err) => {
                let body =
                    Envelope::failure("Shape error", Some(json!({"detail": err.to_string()})));
                return (StatusCode::BAD_REQUEST, Json(body)).into_response();
            }
        },
        None => None,
    };

    let inc_angle_deg = payload.inc_angle_deg.unwrap_or(40.0);
    let index_type = payload.index_type.clone();
    let alpha = payload.alpha.unwrap_or(0.70);
    let beta = payload.beta.unwrap_or(-0.30);
    let gamma = payload.gamma.unwrap_or(0.50);

    // L-band indices (HH/HV from NISAR GCOV) vs C-band (VV/VH).
    let is_lband = matches!(payload.index_type.as_str(), "L_RVI" | "NISAR_FLOOD");
    let compute_task = if is_lband {
        let hh = match payload.hh {
            Some(hh) => hh,
            None => {
                let body = Envelope::failure(
                    "Missing bands",
                    Some(json!({
                        "detail": format!(
                            "{} requires hh and hv arrays",
                            payload.index_type
                        )
                    })),
                );
                return (StatusCode::BAD_REQUEST, Json(body)).into_response();
            }
        };
        let hv = match payload.hv {
            Some(hv) => hv,
            None => {
                let body = Envelope::failure(
                    "Missing bands",
                    Some(json!({
                        "detail": format!(
                            "{} requires hh and hv arrays",
                            payload.index_type
                        )
                    })),
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
            run_pipeline_sar(hh_raw, hv_raw, inc_angle_deg, &index_type, mask_arr)
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
                mask_arr,
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

/// Compute sample statistics (mode) over a numeric series offloaded
/// from Django for faster processing than pure-Python counting.
async fn stats(Json(payload): Json<StatsRequest>) -> Response {
    let start = std::time::Instant::now();
    let values = payload.values;
    let round_decimals = payload.round_decimals;

    let result =
        tokio::task::spawn_blocking(move || compute_mode_stats(&values, round_decimals)).await;

    let stats = match result {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(error = ?err, "blocking stats task failed");
            let body = Envelope::failure(
                "Internal Error",
                Some(json!({"detail": "stats computation failed"})),
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
        }
    };

    let mut stats = stats;
    stats.processing_ms = start.elapsed().as_secs_f64() * 1000.0;

    (
        StatusCode::OK,
        Json(Envelope::success(
            "OK",
            serde_json::to_value(&stats).unwrap_or_default(),
        )),
    )
        .into_response()
}

/// Frequency-based mode. Optional decimal rounding collapses float noise.
fn compute_mode_stats(values: &[f64], round_decimals: Option<u32>) -> StatsResponse {
    use std::collections::HashMap;

    let round = |v: f64| -> f64 {
        match round_decimals {
            Some(d) if d <= 12 => {
                let f = 10f64.powi(d as i32);
                (v * f).round() / f
            }
            _ => v,
        }
    };

    let mut counts: HashMap<u64, (f64, u64)> = HashMap::new();
    // Map bit-pattern -> (display value, count) so -0.0/0.0 unify via round().
    let mut sample_count = 0u64;

    for &raw in values {
        if !raw.is_finite() {
            continue;
        }
        let r = round(raw);
        let key = r.to_bits();
        let entry = counts.entry(key).or_insert((r, 0));
        entry.1 += 1;
        sample_count += 1;
    }

    if counts.is_empty() {
        return StatsResponse {
            mode: None,
            mode_count: 0,
            sample_count: 0,
            unique_count: 0,
            multimodal: false,
            modes: vec![],
            processing_ms: 0.0,
        };
    }

    let max_count = counts.values().map(|(_, c)| *c).max().unwrap_or(0);
    let mut modes: Vec<f64> = counts
        .values()
        .filter(|(_, c)| *c == max_count)
        .map(|(v, _)| *v)
        .collect();
    modes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    StatsResponse {
        mode: modes.first().copied(),
        mode_count: max_count,
        sample_count,
        unique_count: counts.len() as u64,
        multimodal: modes.len() > 1,
        modes,
        processing_ms: 0.0,
    }
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

#[cfg(test)]
mod stats_tests {
    use super::compute_mode_stats;

    #[test]
    fn mode_exact_match() {
        let res = compute_mode_stats(&[1.0, 2.0, 2.0, 3.0], None);
        assert_eq!(res.mode, Some(2.0));
        assert_eq!(res.mode_count, 2);
        assert!(!res.multimodal);
        assert_eq!(res.sample_count, 4);
    }

    #[test]
    fn mode_rounding_collapses_float_noise() {
        let res = compute_mode_stats(&[-11.8500001, -11.8499999, -12.0], Some(2));
        assert_eq!(res.mode, Some(-11.85));
        assert_eq!(res.mode_count, 2);
        assert_eq!(res.unique_count, 2);
    }

    #[test]
    fn mode_multimodal_tie() {
        let res = compute_mode_stats(&[1.0, 1.0, 2.0, 2.0], None);
        assert!(res.multimodal);
        assert_eq!(res.modes, vec![1.0, 2.0]);
        assert_eq!(res.mode_count, 2);
    }

    #[test]
    fn mode_empty_and_nonfinite() {
        let res = compute_mode_stats(&[], Some(2));
        assert_eq!(res.mode, None);
        assert_eq!(res.sample_count, 0);

        let res = compute_mode_stats(&[f64::NAN, f64::INFINITY, 1.0], Some(2));
        assert_eq!(res.mode, Some(1.0));
        assert_eq!(res.sample_count, 1);
    }
}
