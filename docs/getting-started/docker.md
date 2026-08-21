# Docker Installation

This page is the fastest way to run Impulse in containers and verify health, metrics, and first proxied traffic.

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/) 24+ (or Docker Desktop)
- [Docker Compose](https://docs.docker.com/compose/install/) v2 plugin (bundled with Docker Desktop)

## Choose Your Docker Path

- Want the fastest container evaluation path: use the provided Compose stack plus a small demo backend
- Want to run only the Impulse container: use the single-container commands later in this page
- Want full host and production guidance: use [Production Deployment](../deployment/production.md)

## Quick Start with Docker Compose

The fastest working container path is:

1. use the provided Compose stack
2. point the default upstream at a demo backend
3. verify first traffic, health, and metrics

**1. Clone the repository:**

```bash
git clone https://github.com/Supernova-Labs-Org/impulse.git
cd impulse
```

**2. Use the repo development certificates for local testing.**

The packaged Compose file already mounts `certs/proxy-cert.pem` and `certs/proxy-key-pkcs8.pem` from the repository.

For real deployments, replace them with your own certificate material and follow [TLS Setup](../configuration/tls.md).

**3. Start a small demo backend:**

```bash
docker run -d --name impulse-demo-backend --rm -p 8080:80 nginx:alpine
```

**4. Edit the config to point at that backend:**

Open `packaging/docker/config.docker.yaml` and replace the upstream address:

```yaml
upstream:
  default:
    backends:
      - id: "default-backend"
        address: "http://host.docker.internal:8080"
```

If you are on Linux, replace `host.docker.internal` with a reachable host-gateway address or run the backend in the same Compose project and use its service name.

Also replace the control API token:

```yaml
observability:
  control_api:
    auth_token: "replace-with-strong-token"   # <-- change this
```

**5. Start the stack:**

```bash
docker compose -f packaging/docker/docker-compose.yml up -d --build
```

**6. Verify health, metrics, and first traffic:**

```bash
# Health check
curl -k --http1.1 https://127.0.0.1:9902/health

# Metrics
curl http://127.0.0.1:9901/metrics

# First proxied request
curl --http3-only -k https://127.0.0.1:9889/
```

**Stop the stack:**

```bash
docker compose -f packaging/docker/docker-compose.yml down
docker rm -f impulse-demo-backend 2>/dev/null || true
```

## Running a Single Container

If you prefer to manage the container directly:

```bash
docker build -t impulse:latest -f packaging/docker/Dockerfile .

docker run -d \
  --name impulse \
  -p 9889:9889/udp \
  -p 9889:9889/tcp \
  -p 9901:9901 \
  -p 9902:9902 \
  -v "$(pwd)/packaging/docker/config.docker.yaml:/etc/impulse/config.yaml:ro" \
  -v "$(pwd)/certs:/etc/impulse/certs:ro" \
  --restart unless-stopped \
  impulse:latest
```

## Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| 9889 | UDP + TCP | QUIC / HTTP3 proxy listener |
| 9901 | TCP | Prometheus metrics endpoint |
| 9902 | TCP | Control API (health, ready, admin) |

## Using a Custom Config

Mount your own config file instead of the default:

```bash
docker run -d \
  --name impulse \
  -p 9889:9889/udp -p 9889:9889/tcp \
  -p 9901:9901 -p 9902:9902 \
  -v "/path/to/your/config.yaml:/etc/impulse/config.yaml:ro" \
  -v "/path/to/your/certs:/etc/impulse/certs:ro" \
  --restart unless-stopped \
  impulse:latest
```

See `packaging/docker/config.docker.yaml` for the packaged container reference config.

## Building the Image

A helper script is provided to build and tag the image:

```bash
# Default tag: impulse:packaging
./packaging/docker/scripts/build-image.sh

# Custom tag
./packaging/docker/scripts/build-image.sh impulse:1.0.0
```

## Smoke Test

Run the bundled smoke test to verify the image builds, starts, and responds correctly:

```bash
./packaging/docker/scripts/smoke-test.sh
```

This validates:
- Image builds and the container starts cleanly
- Control API health endpoint responds at `https://127.0.0.1:9902/health`
- Metrics endpoint responds at `http://127.0.0.1:9901/metrics`
- Container logs show a clean runtime startup

## Logs

```bash
# Follow live logs
docker logs -f impulse

# With Compose
docker compose -f packaging/docker/docker-compose.yml logs -f impulse
```

By default, the container logs to stdout/stderr. To persist logs to a file, set in your config:

```yaml
log:
  file:
    enabled: true
    path: /var/log/impulse/impulse.log
```

And mount a volume for `/var/log/impulse/`.

## Upgrading

```bash
# Rebuild the image from latest source
docker compose -f packaging/docker/docker-compose.yml up -d --build

# Or for a single container
docker build -t impulse:latest -f packaging/docker/Dockerfile .
docker rm -f impulse
docker run -d ...   # same run command as before
```

## What to Read Next

- [Quickstart](../tutorials/quickstart.md) - fastest local non-container first run
- [Installation](installation.md) - install Impulse directly on a host
- [Minimum Production](minimum-production.md) - minimum safe production posture
- [Production Deployment](../deployment/production.md) - full deployment guidance
