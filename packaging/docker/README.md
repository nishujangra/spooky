# Docker Packaging Bootstrap

This directory contains the initial Docker packaging layout for Impulse.

## Files
- `Dockerfile`: production-style multi-stage build for Impulse (`impulse` binary + slim runtime image).
- `Dockerfile.dev`: development build image for local iteration.
- `config.docker.yaml`: container-friendly config with metrics and the control API disabled by default.
- `docker-compose.yml`: local validation stack for the packaged image.
- `scripts/build-image.sh`: helper to build the image.
- `scripts/smoke-test.sh`: helper to verify startup and the secure default port exposure.

## Build
From `impulse/` root:

```bash
./packaging/docker/scripts/build-image.sh
```

Custom tag:

```bash
./packaging/docker/scripts/build-image.sh impulse:my-tag
```

## Run (Single Container)
From `impulse/` root:

```bash
docker run --rm \
  --name impulse-packaging \
  -p 9889:9889/udp \
  -p 9889:9889/tcp \
  -v "$(pwd)/packaging/docker/config.docker.yaml:/etc/impulse/config.yaml:ro" \
  -v "$(pwd)/certs:/etc/impulse/certs:ro" \
  impulse:packaging
```

## Run (Compose)
From `impulse/` root:

```bash
docker compose -f packaging/docker/docker-compose.yml up -d --build
docker compose -f packaging/docker/docker-compose.yml logs -f impulse
```

Stop:

```bash
docker compose -f packaging/docker/docker-compose.yml down
```

## Smoke Test
From `impulse/` root:

```bash
./packaging/docker/scripts/smoke-test.sh
```

What this validates:
- Image builds and starts.
- The secure default keeps optional observability endpoints disabled and unpublished.
- Container logs show runtime startup.

## Notes
- `config.docker.yaml` uses `http://127.0.0.1:8080` as upstream placeholder; startup and observability checks work without that backend being present, but proxied request success requires a reachable backend.
- Certificates are mounted from the repo `certs/` directory. Replace with production certificates in real deployments.
