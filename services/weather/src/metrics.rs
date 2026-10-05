use axum::{body::Body, extract::MatchedPath, http::Request, middleware::Next, response::Response};
use once_cell::sync::Lazy;
use prometheus::{Encoder, HistogramOpts, HistogramVec, IntCounterVec, Opts, TextEncoder};
use std::time::Instant;

pub const METRICS_CONTENT_TYPE: &str = "text/plain; version=0.0.4";

static HTTP_REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let opts = Opts::new("http_requests_total", "Total HTTP requests");
    let counter = IntCounterVec::new(opts, &["method", "path", "status"])
        .expect("http_requests_total metric can be created");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("http_requests_total can be registered");
    counter
});

static HTTP_REQUEST_DURATION: Lazy<HistogramVec> = Lazy::new(|| {
    let opts = HistogramOpts::new(
        "http_request_duration_seconds",
        "HTTP request duration in seconds",
    )
    .buckets(vec![
        0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ]);
    let histogram = HistogramVec::new(opts, &["method", "path", "status"])
        .expect("http_request_duration_seconds metric can be created");
    prometheus::default_registry()
        .register(Box::new(histogram.clone()))
        .expect("http_request_duration_seconds can be registered");
    histogram
});

pub async fn metrics_middleware(req: Request<Body>, next: Next) -> Response {
    let method = req.method().as_str().to_owned();
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| req.uri().path().to_owned());
    let start = Instant::now();
    let response = next.run(req).await;
    let status = response.status().as_u16().to_string();

    HTTP_REQUESTS
        .with_label_values(&[method.as_str(), path.as_str(), status.as_str()])
        .inc();
    HTTP_REQUEST_DURATION
        .with_label_values(&[method.as_str(), path.as_str(), status.as_str()])
        .observe(start.elapsed().as_secs_f64());

    response
}

pub fn render_metrics() -> Result<String, prometheus::Error> {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer)?;
    Ok(String::from_utf8(buffer).unwrap_or_default())
}
