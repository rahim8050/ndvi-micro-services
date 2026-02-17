# Phase 0 Contract Freeze (weather-apis -> Rust migration)

This document captures the current public API contract from the Django
gateway (`~/projects/weather-apis`) for NDVI and Weather endpoints.
It is the source of truth for Phase 0 and must be matched exactly by
the Rust services.

Sources:
- `weather-apis/openapi.json`
- `weather-apis/config/api/responses.py`
- `weather-apis/config/settings.py`

## Public API base path

- All endpoints are under `/api/v1/` and must remain unchanged.
- Django remains the public gateway.

## NDVI endpoints (Django public contract)

- `GET /api/v1/farms/{farm_id}/ndvi/latest/`
- `GET /api/v1/farms/{farm_id}/ndvi/raster.png`
- `POST /api/v1/farms/{farm_id}/ndvi/raster/queue`
- `POST /api/v1/farms/{farm_id}/ndvi/refresh/`
- `GET /api/v1/farms/{farm_id}/ndvi/timeseries/`
- `GET /api/v1/ndvi/jobs/{job_id}/`

## Weather endpoints (Django public contract)

- `GET /api/v1/weather/current/`
- `GET /api/v1/weather/daily/`
- `GET /api/v1/weather/weekly/`

## Response envelope (MUST MATCH)

Defined in `weather-apis/config/api/responses.py`:

```json
{
  "status": 0,
  "message": "OK",
  "data": {},
  "errors": null
}
```

- Success: `status = 0`
- Failure: `status = 1`
- Do not rename fields or remove `errors` without explicit approval.

## Authentication (required)

Django uses both JWT and API key authentication:

- JWT: `Authorization: Bearer <token>`
- API key: `X-API-Key: <key>`

Rust services must accept both and remain compatible with the gateway.

## Throttling (replicate in Rust)

From `weather-apis/config/settings.py`:

- anon: `100/min`
- user: `1000/min`
- register: `5/min`
- login: `10/min`
- token_refresh: `20/min`
- password_reset: `5/min`
- password_reset_confirm: `10/min`
- api_key: `API_KEY_THROTTLE_RATE` (default `10/min`)
- nextcloud_hmac: `API_KEY_THROTTLE_RATE` (default `10/min`)

## Databases (unchanged)

- NDVI: Postgres (Rust `ndvi-service` uses Postgres)
- Weather: MySQL (Django uses MySQL; Rust weather service must use MySQL)

## Notes

- The contract above must not change during the migration.
- Any deviation is a breaking change and must be explicitly approved.
