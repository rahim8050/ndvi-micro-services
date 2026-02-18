use axum::{body::Body, extract::MatchedPath, http::Request, middleware::Next, response::Response};
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounterVec, Opts, TextEncoder};

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

pub async fn metrics_middleware(req: Request<Body>, next: Next) -> Response {
    let method = req.method().as_str().to_owned();
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| req.uri().path().to_owned());
    let response = next.run(req).await;
    let status = response.status().as_u16().to_string();

    HTTP_REQUESTS
        .with_label_values(&[method.as_str(), path.as_str(), status.as_str()])
        .inc();

    response
}

pub fn render_metrics() -> Result<String, prometheus::Error> {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer)?;
    Ok(String::from_utf8(buffer).unwrap_or_default())
}
