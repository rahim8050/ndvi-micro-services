# NDVI migration phases overview

Date: 2026-02-19

## Phase list (authoritative)

| Phase | Title | Status | Output |
|---|---|---|---|
| 0 | Contract freeze | Complete | Frozen NDVI + weather contracts from Django |
| 1 | Multi-service + shared auth/throttle | Complete | Rust workspace, shared auth/throttle, NDVI envelope |
| 2 | Weather migration | Planned | Weather endpoints implemented in Rust |
| 3 | Gateway cutover | Planned | Django routes to Rust services with canary |

## How many phases remain?

Two planned phases remain: Phase 2 and Phase 3.

## End state (after Phase 3)

After Phase 3, the system will look like this:

- Django remains the public gateway and forwards NDVI + weather requests to Rust.
- NDVI endpoints are fully served by `services/ndvi` (Postgres).
- Weather endpoints are fully served by `services/weather` (MySQL).
- Shared auth and throttling are enforced in Rust.
- Metrics, alerts, and canary rollout are in place.
- CI enforces `fmt`, `clippy`, and tests across the workspace.

This is the production-ready target state for NDVI + weather services.
