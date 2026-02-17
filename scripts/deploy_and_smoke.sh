#!/usr/bin/env bash
set -euo pipefail

IMAGE_TAG=${IMAGE_TAG:-}
REGISTRY=${REGISTRY:-}
REGISTRY_NAMESPACE=${REGISTRY_NAMESPACE:-}
IMAGE_NAME=${IMAGE_NAME:-ndvi-service}
CANARY_WEIGHT=${CANARY_WEIGHT:-}
CURL_IMAGE=${CURL_IMAGE:-curlimages/curl:8.6.0}

if [ -z "$IMAGE_TAG" ]; then
  echo "IMAGE_TAG is required" >&2
  exit 2
fi

if [ -n "$CANARY_WEIGHT" ]; then
  echo "Deploying canary at ${CANARY_WEIGHT}% traffic"
fi

IMAGE="${IMAGE_NAME}:${IMAGE_TAG}"
if [ -n "$REGISTRY" ]; then
  if [ -z "$REGISTRY_NAMESPACE" ]; then
    echo "REGISTRY_NAMESPACE is required when REGISTRY is set" >&2
    exit 2
  fi
  IMAGE="$REGISTRY/$REGISTRY_NAMESPACE/${IMAGE_NAME}:${IMAGE_TAG}"
  docker pull "$IMAGE"
fi

NETWORK="ndvi-canary"
DB="ndvi-canary-db"
APP="ndvi-canary-app"

if ! docker network inspect "$NETWORK" >/dev/null 2>&1; then
  docker network create "$NETWORK" >/dev/null
fi

if ! docker ps -a --format '{{.Names}}' | grep -qx "$DB"; then
  docker run -d --name "$DB" --network "$NETWORK" \
    -e POSTGRES_USER=ndvi \
    -e POSTGRES_PASSWORD=ndvi \
    -e POSTGRES_DB=ndvi \
    postgres:16 >/dev/null
elif ! docker ps --format '{{.Names}}' | grep -qx "$DB"; then
  docker start "$DB" >/dev/null
fi

ready=0
for _ in $(seq 1 30); do
  if docker exec "$DB" pg_isready -U ndvi >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done
if [ "$ready" -ne 1 ]; then
  echo "database not ready" >&2
  docker logs "$DB" --tail 200 || true
  exit 1
fi

if [ -f "db/init.sql" ]; then
  docker exec -i "$DB" psql -U ndvi -d ndvi < db/init.sql
else
  docker exec -i "$DB" psql -U ndvi -d ndvi <<'SQL'
CREATE TABLE IF NOT EXISTS ndvi_samples (
    id BIGSERIAL PRIMARY KEY,
    farm_id UUID NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    mean DOUBLE PRECISION NOT NULL,
    min DOUBLE PRECISION NOT NULL,
    max DOUBLE PRECISION NOT NULL,
    source TEXT,
    geometry JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
SQL
fi

if docker ps -a --format '{{.Names}}' | grep -qx "$APP"; then
  docker rm -f "$APP" >/dev/null
fi

docker run -d --name "$APP" --network "$NETWORK" \
  -e DATABASE_URL=postgres://ndvi:ndvi@${DB}:5432/ndvi \
  -e PORT=8081 \
  "$IMAGE" >/dev/null

curl_in_net() {
  docker run --rm --network "$NETWORK" "$CURL_IMAGE" "$@"
}

for _ in $(seq 1 30); do
  if curl_in_net -fsS "http://${APP}:8081/healthz" >/dev/null; then
    break
  fi
  sleep 1
done

if ! curl_in_net -fsS "http://${APP}:8081/healthz" >/dev/null; then
  echo "health check failed" >&2
  docker logs "$APP" --tail 200 || true
  exit 1
fi

curl_in_net -fsS "http://${APP}:8081/metrics" >/dev/null

payload='{"farm_id":"00000000-0000-0000-0000-000000000001","timestamp":"2025-01-01T00:00:00Z","mean":0.5,"min":0.4,"max":0.6,"source":"canary","geometry":null}'
http_code=$(curl_in_net -s -o /dev/null -w "%{http_code}" \
  -H "content-type: application/json" \
  -X POST "http://${APP}:8081/api/v1/ndvi" \
  -d "$payload")
if [ "$http_code" != "201" ]; then
  echo "unexpected status: $http_code" >&2
  docker logs "$APP" --tail 200 || true
  exit 1
fi
