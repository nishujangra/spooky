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

echo "Waiting for health endpoint..."
for _ in {1..30}; do
  if curl -ksf "https://127.0.0.1:9902/health" >/dev/null; then
    break
  fi
  sleep 1
done

curl -ksf "https://127.0.0.1:9902/health" >/dev/null
echo "Control API health endpoint is reachable"

curl -sf "http://127.0.0.1:9901/metrics" | head -n 20
echo "Metrics endpoint is reachable"

docker compose -f "${COMPOSE_FILE}" logs --tail=120 spooky
echo "Smoke test passed"
