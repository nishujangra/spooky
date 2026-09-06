#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
COMPOSE_FILE="${ROOT_DIR}/packaging/docker/docker-compose.yml"

echo "Starting Impulse Docker packaging smoke test"
docker compose -f "${COMPOSE_FILE}" up -d --build

cleanup() {
  echo "Stopping smoke-test stack"
  docker compose -f "${COMPOSE_FILE}" down
}
trap cleanup EXIT

echo "Waiting for the container to remain running..."
for _ in {1..30}; do
  if docker compose -f "${COMPOSE_FILE}" ps --status running --services \
    | grep -qx "impulse"; then
    break
  fi
  sleep 1
done

if ! docker compose -f "${COMPOSE_FILE}" ps --status running --services \
  | grep -qx "impulse"; then
  echo "Impulse container did not remain running"
  docker compose -f "${COMPOSE_FILE}" logs --tail=120 impulse
  exit 1
fi

echo "Container is running (metrics and control API are disabled by default)"

for optional_port in 9901 9902; do
  if docker compose -f "${COMPOSE_FILE}" port impulse "${optional_port}" >/dev/null 2>&1; then
    echo "Optional observability port ${optional_port} must not be published by the default stack"
    exit 1
  fi
done

docker compose -f "${COMPOSE_FILE}" logs --tail=120 impulse
echo "Smoke test passed"
