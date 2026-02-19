# Phase 1 - Multi-service layout, shared auth/throttle, NDVI envelope

Date: 2026-02-19

## Scope (Phase 1)

Establish a multi-crate layout inside `ndvi-service`, add shared auth + throttling, and align NDVI responses with the Django response envelope. Weather endpoints are stubbed (501) to preserve contracts while migration proceeds.

## Completed work

### Repository layout

```
crates/common/        # shared envelope, auth, throttling
services/ndvi/        # NDVI service (moved from root src/)
services/weather/     # Weather service (stub endpoints)
src/main.rs           # root binary delegates to services/ndvi
```

### Shared envelope and auth/throttle

- Added `ndvi-common` crate with:
  - `Envelope<T>` response wrapper (`status` 0/1, `message`, `data`, `errors`)
  - Auth middleware supporting:
    - JWT (Authorization: Bearer)
    - API key (X-API-Key) against Django MySQL (PBKDF2)
  - Rate limiting via governor:
    - Anon: 100/min
    - User: 1000/min
    - API key: 10/min (configurable)

### NDVI service changes

- NDVI handlers moved to `services/ndvi`.
- All NDVI endpoints return the response envelope.
- Validation errors return 400 with envelope.
- DB errors return 500 with envelope.
- Auth + throttling layers wired in.

### Weather service (stub)

- Added `services/weather` with:
  - `/api/v1/weather/current/`
  - `/api/v1/weather/daily/`
  - `/api/v1/weather/weekly/`
- Returns 501 Not Implemented with envelope until migration is complete.

### Docker and CI adjustments

- Dockerfile builds the workspace and ships the root binary.
- `docker-compose.yml` uses `AUTH_DISABLED=true` for local/dev.
- Canary smoke script initializes `ndvi_samples` table and disables auth for smoke tests.
- CI still runs `fmt`, `clippy`, and `test` across the workspace.

## Environment variables (Phase 1)

Required in production:

- `DATABASE_URL` (Postgres for NDVI)
- `MYSQL_DATABASE_URL` (MySQL for API key validation and weather)
- `JWT_SIGNING_KEY` (required when auth enabled)
- `DJANGO_API_KEY_PEPPER` (required when API key auth enabled)

Optional:

- `JWT_ISSUER`, `JWT_AUDIENCE`
- `AUTH_DISABLED` (only for local/dev or canary smoke tests)
- `AUTH_BYPASS_PATHS` (comma-separated allowlist)
- `THROTTLE_ENABLED`, `THROTTLE_ANON_RATE`, `THROTTLE_USER_RATE`, `API_KEY_THROTTLE_RATE`
- `PORT` (NDVI default 8081, weather default 8090)

## Contracts

- Response envelope:
  - success: `status = 0`
  - failure: `status = 1`
- Auth headers:
  - `Authorization: Bearer <token>`
  - `X-API-Key: <key>`

## Open items for Phase 2

- Implement weather endpoints with real data and MySQL queries.
- Add per-route scoped throttles if needed (login/token/etc.).
- Update `Cargo.lock` once network access is available in CI/local.
