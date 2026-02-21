# Phase 2 - Weather endpoint migration (implementation + parity)

Date: 2026-02-19

## Goal

Implement weather endpoints in Rust with functional parity to the Django service, while preserving the contract frozen in Phase 0.

## Scope

- Replace 501 stubs with real handlers for:
  - `/api/v1/weather/current/`
  - `/api/v1/weather/daily/`
  - `/api/v1/weather/weekly/`
- Use the same upstream providers as Django (Open-Meteo and NASA POWER).
- Preserve response envelope (`status`, `message`, `data`, `errors`).
- Keep auth (JWT + API key) and throttling enabled for all routes.

## Implementation checklist

### Data access

- Define models/DTOs for each weather response shape.
- Implement provider clients in `services/weather/src/providers/`.
- Keep provider parameters aligned with Django settings:
  - `OPEN_METEO_BASE_URL`
  - `NASA_POWER_BASE_URL`
  - `WEATHER_NASA_POWER_COMMUNITY`

### API handlers

- Implement handlers in `services/weather/src/routes.rs`.
- Validate inputs (lat/lon ranges, required params).
- Use consistent error mapping:
  - 400 for validation
  - 502 for upstream/provider failures

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

- Weather endpoints return real data (no 501), backed by Open-Meteo and NASA POWER.
- Updated tests and docs for weather API behavior.
- CI green with `cargo fmt`, `clippy`, `test`.

## Status

Complete (Phase 2 implemented in Rust weather service).
