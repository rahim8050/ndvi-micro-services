# NDVI Service (Rust microservices)

This repository hosts Rust microservices that back NDVI and weather endpoints. Django remains the public gateway; these services are internal backends with separate databases.

## What’s here

- **NDVI service** (default container/binary)
  - Postgres-backed ingestion API
  - Port `8081` by default
- **Weather service**
  - Weather APIs backed by Open-Meteo and NASA POWER
  - Port `8090` by default
- **Shared crate** for response envelope, auth, and throttling

## Repository layout

```
crates/common/        # shared envelope, auth, throttling
services/ndvi/        # NDVI service implementation
services/weather/     # Weather service implementation
src/main.rs           # root binary delegates to services/ndvi
db/init.sql           # NDVI schema
scripts/              # canary and smoke tooling
docs/                 # phase docs and contracts
```

## API contract (current)

All endpoints return a consistent envelope:

```json
{
  "status": 0,
  "message": "OK",
  "data": {},
  "errors": null
}
```

### NDVI

- `POST /api/v1/ndvi` – ingest NDVI sample (201 Created)
- `GET /api/v1/` – API discovery
- `GET /healthz` – liveness
- `GET /metrics` – Prometheus metrics

### Weather

- `GET /api/v1/weather/current/` – current conditions
- `GET /api/v1/weather/daily/` – daily forecasts
- `GET /api/v1/weather/weekly/` – weekly aggregates

## Auth + throttling

Auth is required for all endpoints (JWT or API key):

- `Authorization: Bearer <token>`
- `X-API-Key: <key>`

Environment variables:

- `JWT_SIGNING_KEY` (required when auth enabled)
- `DJANGO_API_KEY_PEPPER` (required for API key validation)
- `JWT_ISSUER`, `JWT_AUDIENCE` (optional)
- `WEATHER_PROVIDER_DEFAULT` (`open_meteo` or `nasa_power`)
- `OPEN_METEO_BASE_URL`, `NASA_POWER_BASE_URL`
- `WEATHER_NASA_POWER_COMMUNITY`, `NASA_POWER_DAILY_LAG_DAYS`
- `WEATHER_MAX_RANGE_DAYS`, `WEATHER_DEFAULT_TZ`
- `AUTH_DISABLED=true` (local/dev or smoke tests only)
- `AUTH_BYPASS_PATHS=/healthz,/metrics` (optional allowlist)
- `THROTTLE_ENABLED`, `THROTTLE_ANON_RATE`, `THROTTLE_USER_RATE`, `API_KEY_THROTTLE_RATE`

## Local development

### NDVI service (Docker)

```sh
docker compose up --build
```

Notes:
- `docker-compose.yml` uses `AUTH_DISABLED=true` for local runs.
- Postgres is mapped to host port `5433`.

### Weather service (local)

```sh
export MYSQL_DATABASE_URL="mysql://user:pass@host:3306/db"
export DJANGO_API_KEY_PEPPER="(set securely)"
export JWT_SIGNING_KEY="(set securely)"
cargo run -p weather-service
```

## CI

GitHub Actions runs:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

## Phase docs

- Phase 0: `docs/phase0_contract_freeze.md`
- Phase 1: `docs/phase1_multi_service.md`
- Phase 2: `docs/phase2_weather_migration.md`
- Phase 3: `docs/phase3_gateway_cutover.md`
- Overview: `docs/phases_overview.md`

## Contributing

- Read `AGENTS.md` for required behavior and safety rules.
- Do not log or commit secrets.
- Keep response contracts stable unless versioned.
