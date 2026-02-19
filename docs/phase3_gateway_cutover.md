# Phase 3 - Django gateway cutover and production hardening

Date: 2026-02-19

## Goal

Route NDVI + weather traffic through Django to the Rust services, with canary rollout, monitoring, and rollback procedures.

## Scope

- Switch Django gateway to call Rust NDVI + weather services.
- Keep databases separate:
  - NDVI -> Postgres
  - Weather -> MySQL
- Maintain auth + throttling policies.

## Implementation checklist

### Django integration

- Update internal client calls to target Rust service URLs.
- Preserve request/response envelope parity.
- Confirm auth headers (JWT + API key) are forwarded correctly.

### Deployment + canary

- Use canary rollout to 10% -> 50% -> 100%.
- Run smoke tests at each step.
- Ensure rollback procedures are documented and tested.

### Monitoring + alerts

- Validate Prometheus scraping (`/metrics`).
- Ensure alerts for availability + 5xx rate are active.
- Confirm logs are structured and error traces are visible.

### Cleanup

- Remove or deprecate Django endpoint implementations after full cutover.
- Update docs to reflect ownership of NDVI + weather endpoints in Rust.

## Phase 3 outputs

- Django gateway uses Rust services as the backend of record.
- Production-ready routing, canary, and alerting in place.
- Django no longer serves NDVI/Weather logic directly.
