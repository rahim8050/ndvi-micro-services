# AGENTS.md - NDVI Service Agent Guide

This document defines **agent roles, responsibilities, and constraints** for the NDVI Rust microservice repository. It is the single reference for how automated agents and developers should operate safely, consistently, and in a production-grade manner.

---

## 1) Purpose & Scope

Agents in this repository must **prioritize safety, determinism, and minimal change**. They should improve and maintain the service while avoiding unsafe operations (e.g., leaking secrets, breaking API contracts, or modifying infra without approval).

### Agent Roles

| Agent | Purpose | Scope (Allowed) | Limitations (Not Allowed) |
|---|---|---|---|
| **Service Dev Agent** | Implement API features and internal logic | Rust code, endpoints, validation, DB access | No breaking API changes without versioning; no secret exposure |
| **Ops/Canary Agent** | Validate runtime health and canary deploys | Docker, GitHub Actions, scripts | Must not hit production without explicit approval |
| **CI/CD Agent** | Maintain workflows, lint/test gates | `.github/workflows`, build/test scripts | No bypassing required checks |
| **Security Agent** | Enforce secure practices and review risks | Config hardening, secret handling, logging audits | Must not request or log secrets |

### Expected Behavior

- Be **explicit** about changes and their impact.
- Keep **changes small and isolated**.
- Prefer existing patterns and modules.
- Use **typed, validated inputs** and clear error responses.

### Safe vs Unsafe Actions (Examples)

**Safe**
- Add a new handler under `src/routes.rs` with validation and tests.
- Use `DATABASE_URL` and `PORT` from environment variables.
- Add a CI check that runs `cargo fmt` and `cargo clippy`.

**Unsafe**
- Logging request bodies that may include sensitive user data.
- Hardcoding DB credentials or API tokens in the repo.
- Modifying deployment behavior without tests or review.

---

## 2) Security & Environment

### Secrets & Credentials

- **Never** commit or print secrets (API keys, tokens, passwords).
- Use environment variables for sensitive values:
  - `DATABASE_URL` (required)
  - `PORT` (default 8081)
  - `RUST_LOG` (optional)
- For CI, use GitHub Secrets (e.g., `GITHUB_TOKEN` for GHCR).

### Network & External Access

- Avoid contacting external services without explicit instructions.
- Any production endpoints must be accessed **only with approval**.
- Prefer local Docker networks for canary checks.

### Filesystem

- Operate only within the repository root.
- Do not delete unrelated files or user changes.
- Do not access outside paths without explicit permission.

---

## 3) Operational Guidelines

### API Endpoints (Current)

| Method | Path | Purpose | Auth |
|---|---|---|---|
| `POST` | `/api/v1/ndvi` | Ingest NDVI sample | Required |
| `GET` | `/healthz` | Liveness check | Required |
| `GET` | `/metrics` | Prometheus metrics | Required |
| `GET` | `/api/v1/` | API discovery | Required |

> Assume all endpoints are **authenticated**. Examples below use `Authorization: Bearer $NDVI_API_TOKEN`.

### Database

- Primary table: `ndvi_samples`.
- Use **parameterized queries** (`sqlx::query(...).bind(...)`).
- If schema changes are needed, update `db/init.sql` and any migrations.

### Allowed Dependencies

Use existing core libraries unless a new dependency is justified:

- Runtime: `axum`, `tokio`
- Data/DB: `sqlx`, `serde`, `serde_json`, `uuid`, `chrono`
- Logging/metrics: `tracing`, `tracing-subscriber`, `prometheus`, `once_cell`

### Logging & Errors

- Use `tracing` for logs.
- Do not log secrets or full request bodies.
- Return:
  - `201 Created` on success
  - `400 Bad Request` for validation errors
  - `500 Internal Server Error` for DB or server failures

### Rate Limiting

No rate limiter is currently enforced. If adding one:
- Prefer `tower`/`tower-governor`.
- Make limits configurable via environment variables.
- Add tests to verify enforcement and error responses.

---

## 4) CI/CD & Pre-Commit

### Local Pre-Commit (Required)

Run before pushing:

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

### CI Workflows

- `.github/workflows/ci.yml` - formatting, clippy, test.
- `.github/workflows/ndvi-canary.yml` - GHCR build/push + canary smoke tests.

Agents should:
- Never bypass required checks.
- Keep CI secrets in GitHub Secrets or environment variables.
- Avoid printing environment variables or tokens in logs.

---

## 5) Step-by-Step Example (Safe Agent Usage)

### Safe Workflow Checklist

1. Review the current state and requirements.
2. Make the smallest change needed.
3. Run local checks (`fmt`, `clippy`, `test`).
4. Update docs if behavior changes.

### CRUD-Like Interaction (NDVI Endpoint)

> Assume `NDVI_API_TOKEN` is provided via environment and never printed.

**Create** (ingest an NDVI sample):

```sh
export NDVI_API_TOKEN="(set in your shell; do not echo)"

curl -fsS -X POST "http://127.0.0.1:8081/api/v1/ndvi" \
  -H "Authorization: Bearer $NDVI_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "farm_id": "00000000-0000-0000-0000-000000000001",
    "timestamp": "2025-01-01T00:00:00Z",
    "mean": 0.5,
    "min": 0.4,
    "max": 0.6,
    "source": "agent-example",
    "geometry": null
  }'
```

**Read** (confirm service is healthy and metrics updated):

```sh
curl -fsS -H "Authorization: Bearer $NDVI_API_TOKEN" \
  "http://127.0.0.1:8081/healthz"

curl -fsS -H "Authorization: Bearer $NDVI_API_TOKEN" \
  "http://127.0.0.1:8081/metrics" | rg http_requests_total
```

**Update/Delete**: Not supported by the current API.  
Agents must **not** attempt to add these without explicit product approval and versioning.

---

## 6) Maintenance & Extensibility

### Extending Features Safely

- Add new API versions under `/api/v2/...` to avoid breaking clients.
- Place new route handlers in `src/routes.rs` or a dedicated module if large.
- Keep data models in `src/models.rs` and DB logic in `src/db.rs`.
- Update `db/init.sql` and add migrations for schema changes.

### Naming & Versioning

- Modules: `snake_case.rs` (e.g., `metrics.rs`).
- Public types: `PascalCase`.
- Environment variables: `UPPER_SNAKE_CASE`.
- Use semantic versioning for externally visible API changes.

### Contract Safety

- Preserve existing response shapes and status codes.
- Add tests for any new validation or DB behavior.
- Update documentation when endpoints or payloads change.

---

## 7) Formatting & Style

### Code

- Rust: `rustfmt` (via `cargo fmt`).
- Lint: `cargo clippy -- -D warnings`.
- Keep functions small and focused.
- Avoid non-ASCII unless already present in the file.

### Docs

- Use clear headings and bullet lists.
- Provide examples using fenced `sh` or `rust` blocks.
- Never include real secrets or tokens.
