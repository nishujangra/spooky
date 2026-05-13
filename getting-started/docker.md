# Docker Installation

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/) 24+ (or Docker Desktop)
- [Docker Compose](https://docs.docker.com/compose/install/) v2 plugin (bundled with Docker Desktop)
- TLS certificate and key for the proxy listener (see [TLS Certificates](installation.md#tls-certificates))

## Quick Start with Docker Compose

The fastest way to get Spooky running is with the provided Compose stack.

**1. Clone the repository (or copy the packaging files):**

```bash
git clone https://github.com/Supernova-Labs-Org/spooky.git
cd spooky
```

**2. Place your TLS certificates:**

```bash
mkdir -p certs
cp /path/to/your/proxy-cert.pem     certs/proxy-cert.pem
cp /path/to/your/proxy-key-pkcs8.pem certs/proxy-key-pkcs8.pem
```

**3. Edit the config to point at your backend:**

Open `packaging/docker/config.docker.yaml` and replace the upstream address:

```yaml
upstream:
  default:
    backends:
      - id: "default-backend"
        address: "http://your-backend:8080"   # <-- change this
```

Also replace the control API token:

```yaml
observability:
  control_api:
    auth_token: "replace-with-strong-token"   # <-- change this
```

**4. Start the stack:**

```bash
docker compose -f packaging/docker/docker-compose.yml up -d --build
```

**5. Verify it is running:**

```bash
# Health check
curl -k https://127.0.0.1:9902/health

# Metrics
curl http://127.0.0.1:9901/metrics
```

**Stop the stack:**

```bash
docker compose -f packaging/docker/docker-compose.yml down
```

## Running a Single Container

If you prefer to manage the container directly:

```bash
docker build -t spooky:latest -f packaging/docker/Dockerfile .

docker run -d \
  --name spooky \
  -p 9889:9889/udp \
  -p 9889:9889/tcp \
  -p 9901:9901 \
  -p 9902:9902 \
  -v "$(pwd)/packaging/docker/config.docker.yaml:/etc/spooky/config.yaml:ro" \
  -v "$(pwd)/certs:/etc/spooky/certs:ro" \
  --restart unless-stopped \
  spooky:latest
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
  --name spooky \
  -p 9889:9889/udp -p 9889:9889/tcp \
  -p 9901:9901 -p 9902:9902 \
  -v "/path/to/your/config.yaml:/etc/spooky/config.yaml:ro" \
  -v "/path/to/your/certs:/etc/spooky/certs:ro" \
  --restart unless-stopped \
  spooky:latest
```

See `packaging/docker/config.docker.yaml` for a fully annotated reference config.

## Building the Image

A helper script is provided to build and tag the image:

```bash
# Default tag: spooky:packaging
./packaging/docker/scripts/build-image.sh

# Custom tag
./packaging/docker/scripts/build-image.sh spooky:1.0.0
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
docker logs -f spooky

# With Compose
docker compose -f packaging/docker/docker-compose.yml logs -f spooky
```

By default, the container logs to stdout/stderr. To persist logs to a file, set in your config:

```yaml
log:
  file:
    enabled: true
    path: /var/log/spooky/spooky.log
```

And mount a volume for `/var/log/spooky/`.

## Upgrading

```bash
# Rebuild the image from latest source
docker compose -f packaging/docker/docker-compose.yml up -d --build

# Or for a single container
docker build -t spooky:latest -f packaging/docker/Dockerfile .
docker rm -f spooky
docker run -d ...   # same run command as before
```
