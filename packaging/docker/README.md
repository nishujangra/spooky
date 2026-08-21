# Docker Packaging Bootstrap

This directory contains the initial Docker packaging layout for Impulse.

## Files
- `Dockerfile`: production-style multi-stage build for Impulse (`spooky` binary + slim runtime image).
- `Dockerfile.dev`: development build image for local iteration.
- `config.docker.yaml`: container-friendly config (binds to `0.0.0.0`, exposes metrics/control API).
- `docker-compose.yml`: local validation stack for the packaged image.
- `scripts/build-image.sh`: helper to build the image.
- `scripts/smoke-test.sh`: helper to run a startup + health + metrics smoke test.

## Build
From `spooky/` root:

```bash
./packaging/docker/scripts/build-image.sh
```

Custom tag:

```bash
./packaging/docker/scripts/build-image.sh spooky:my-tag
```

## Run (Single Container)
From `spooky/` root:

```bash
docker run --rm \
  --name spooky-packaging \
  -p 9889:9889/udp \
  -p 9889:9889/tcp \
  -p 9901:9901 \
  -p 9902:9902 \
  -v "$(pwd)/packaging/docker/config.docker.yaml:/etc/spooky/config.yaml:ro" \
  -v "$(pwd)/certs:/etc/spooky/certs:ro" \
  spooky:packaging
```

## Run (Compose)
From `spooky/` root:

```bash
docker compose -f packaging/docker/docker-compose.yml up -d --build
docker compose -f packaging/docker/docker-compose.yml logs -f spooky
```

Stop:

```bash
docker compose -f packaging/docker/docker-compose.yml down
```

## Smoke Test
From `spooky/` root:

```bash
./packaging/docker/scripts/smoke-test.sh
```

What this validates:
- Image builds and starts.
- Control API health endpoint responds at `https://127.0.0.1:9902/health`.
- Metrics endpoint responds at `http://127.0.0.1:9901/metrics`.
- Container logs show runtime startup.

## Notes
- `config.docker.yaml` uses `http://127.0.0.1:8080` as upstream placeholder; startup and observability checks work without that backend being present, but proxied request success requires a reachable backend.
- Certificates are mounted from the repo `certs/` directory. Replace with production certificates in real deployments.
