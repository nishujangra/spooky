# Docker Installation

This page is the fastest way to run Impulse in containers and verify startup and first proxied traffic. Metrics and the control API are disabled by default for a safer container baseline.

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
3. verify startup and first proxied traffic

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

The shipped Docker config keeps the metrics and control API disabled and does
not publish their ports by default. If you need the control API, enable it and
configure a real token (or mTLS) before starting the stack; placeholder tokens
are rejected by configuration validation:

```yaml
observability:
  metrics:
    enabled: true
    address: "0.0.0.0"
  control_api:
    enabled: true
    address: "0.0.0.0"
    auth_token: "<generate-a-unique-secret>"
```

**5. Start the stack:**

```bash
docker compose -f packaging/docker/docker-compose.yml up -d --build
```

**6. Verify startup and first traffic:**

```bash
# The default Compose stack publishes only the proxy listener.
docker compose -f packaging/docker/docker-compose.yml ps

# First proxied request
curl --http3-only -k https://127.0.0.1:9889/
```

The default stack does not publish ports `9901` or `9902`, and the mounted
configuration keeps metrics and the control API disabled. To expose them for a
local diagnostic session, set both observability addresses to `0.0.0.0`, enable
the endpoints, configure a unique control-API token, and add these Compose port
mappings before starting the stack:

```yaml
ports:
  - "9901:9901"
  - "9902:9902"
```

Do not expose these ports on an untrusted network.

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
  -v "$(pwd)/packaging/docker/config.docker.yaml:/etc/impulse/config.yaml:ro" \
  -v "$(pwd)/certs:/etc/impulse/certs:ro" \
  --restart unless-stopped \
  impulse:latest
```

## Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| 9889 | UDP + TCP | QUIC / HTTP3 proxy listener |
| 9901 | TCP (optional) | Prometheus metrics endpoint; disabled/unpublished by default |
| 9902 | TCP (optional) | Control API (health, ready, admin); disabled/unpublished by default |

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

Run the bundled smoke test to verify the image builds, starts, and preserves the secure default port exposure:

```bash
./packaging/docker/scripts/smoke-test.sh
```

This validates:
- Image builds and the container remains running
- The secure default does not publish optional observability ports
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
