#!/usr/bin/env bash
set -euo pipefail

IMAGE_TAG=${IMAGE_TAG:-}
REGISTRY=${REGISTRY:-}
REGISTRY_NAMESPACE=${REGISTRY_NAMESPACE:-}
IMAGE_NAME=${IMAGE_NAME:-weather-service}
CANARY_WEIGHT=${CANARY_WEIGHT:-}
CURL_IMAGE=${CURL_IMAGE:-curlimages/curl:8.6.0}
AUTH_DISABLED=${AUTH_DISABLED:-true}
WEATHER_SMOKE_MODE=${WEATHER_SMOKE_MODE:-health}
WEATHER_SMOKE_PROVIDER=${WEATHER_SMOKE_PROVIDER:-open_meteo}

MYSQL_ROOT_PASSWORD=${MYSQL_ROOT_PASSWORD:-weather_root}
MYSQL_DATABASE=${MYSQL_DATABASE:-weather}
MYSQL_USER=${MYSQL_USER:-weather}
MYSQL_PASSWORD=${MYSQL_PASSWORD:-weather}

if [ -z "$IMAGE_TAG" ]; then
  echo "IMAGE_TAG is required" >&2
  exit 2
fi

if [ -n "$CANARY_WEIGHT" ]; then
  echo "Deploying weather canary at ${CANARY_WEIGHT}% traffic"
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
DB="weather-canary-db"
APP="weather-canary-app"

if ! docker network inspect "$NETWORK" >/dev/null 2>&1; then
  docker network create "$NETWORK" >/dev/null
fi

if ! docker ps -a --format '{{.Names}}' | grep -qx "$DB"; then
  docker run -d --name "$DB" --network "$NETWORK" \
    -e MYSQL_ROOT_PASSWORD="$MYSQL_ROOT_PASSWORD" \
    -e MYSQL_DATABASE="$MYSQL_DATABASE" \
    -e MYSQL_USER="$MYSQL_USER" \
    -e MYSQL_PASSWORD="$MYSQL_PASSWORD" \
    mysql:8.0 >/dev/null
elif ! docker ps --format '{{.Names}}' | grep -qx "$DB"; then
  docker start "$DB" >/dev/null
fi

ready=0
for _ in $(seq 1 30); do
  if docker exec "$DB" mysqladmin ping -h 127.0.0.1 -u root -p"$MYSQL_ROOT_PASSWORD" --silent >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done
if [ "$ready" -ne 1 ]; then
  echo "mysql not ready" >&2
  docker logs "$DB" --tail 200 || true
  exit 1
fi

if docker ps -a --format '{{.Names}}' | grep -qx "$APP"; then
  docker rm -f "$APP" >/dev/null
fi

docker run -d --name "$APP" --network "$NETWORK" \
  -e MYSQL_DATABASE_URL="mysql://${MYSQL_USER}:${MYSQL_PASSWORD}@${DB}:3306/${MYSQL_DATABASE}" \
  -e PORT=8090 \
  -e AUTH_DISABLED="$AUTH_DISABLED" \
  "$IMAGE" >/dev/null

curl_in_net() {
  docker run --rm --network "$NETWORK" "$CURL_IMAGE" "$@"
}

for _ in $(seq 1 30); do
  if curl_in_net -fsS "http://${APP}:8090/healthz" >/dev/null; then
    break
  fi
  sleep 1
done

if ! curl_in_net -fsS "http://${APP}:8090/healthz" >/dev/null; then
  echo "weather health check failed" >&2
  docker logs "$APP" --tail 200 || true
  exit 1
fi

curl_in_net -fsS "http://${APP}:8090/metrics" >/dev/null

if [ "$WEATHER_SMOKE_MODE" = "full" ]; then
  query="lat=1.0&lon=36.0&provider=${WEATHER_SMOKE_PROVIDER}"
  http_code=$(curl_in_net -s -o /dev/null -w "%{http_code}" \
    "http://${APP}:8090/api/v1/weather/current/?${query}")
  if [ "$http_code" != "200" ]; then
    echo "unexpected weather current status: $http_code" >&2
    docker logs "$APP" --tail 200 || true
    exit 1
  fi
fi
