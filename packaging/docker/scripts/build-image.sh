#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
IMAGE_TAG="${1:-impulse:packaging}"

echo "Building image ${IMAGE_TAG} from ${ROOT_DIR}"
docker build \
  --file "${ROOT_DIR}/packaging/docker/Dockerfile" \
  --tag "${IMAGE_TAG}" \
  "${ROOT_DIR}"

echo "Build complete: ${IMAGE_TAG}"
