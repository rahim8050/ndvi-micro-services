# Phase 2 - Weather endpoint migration (implementation + parity)

Date: 2026-02-19

## Goal

Implement weather endpoints in Rust with functional parity to the Django service, while preserving the contract frozen in Phase 0.

## Scope

- Replace 501 stubs with real handlers for:
  - `/api/v1/weather/current/`
  - `/api/v1/weather/daily/`
  - `/api/v1/weather/weekly/`
- Use MySQL for weather data access (no schema changes unless explicitly approved).
- Preserve response envelope (`status`, `message`, `data`, `errors`).
- Keep auth (JWT + API key) and throttling enabled for all routes.

## Implementation checklist

### Data access

- Define models/DTOs for each weather response shape.
- Implement parameterized queries in `services/weather/src/db.rs`.
- Keep SQL and field names aligned with existing Django expectations.

### API handlers

- Implement handlers in `services/weather/src/routes.rs`.
- Validate inputs (lat/lon ranges, required params).
- Use consistent error mapping:
  - 400 for validation
  - 500 for DB/unknown

### Metrics + logging

- Add request metrics (per-path, status code).
- Log errors with `tracing` (no secrets, no raw tokens).

### Tests

- Add unit tests for parameter validation.
- Add integration tests against MySQL test container when possible.

## Rollout criteria

- Contract parity with Django (response shapes and status codes).
- Latency and error rate within agreed thresholds.
- Smoke tests pass in canary with auth enabled.

## Phase 2 outputs

- Weather endpoints return real data (no 501).
- Updated tests and docs for weather API behavior.
- CI green with `cargo fmt`, `clippy`, `test`.
