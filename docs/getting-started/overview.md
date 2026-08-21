# Overview

This section is the fastest path to understanding Impulse, getting it running, and sending first traffic successfully.

Use this section when you need to answer:

- what Impulse is
- how to install it
- how to run it locally or in a container
- what a minimum safe production posture looks like

## Start Here

- Want the shortest first-run path: [Quickstart](../tutorials/quickstart.md)
- Need installation and runtime requirements: [Installation](installation.md)
- Want a container-based setup: [Docker](docker.md)
- Want the smallest production checklist: [Minimum Production](minimum-production.md)
- Need exact config keys and defaults: [Configuration Reference](../configuration/reference.md)

## What Impulse Is

Impulse is an HTTP/3 edge runtime and load balancer. It terminates QUIC at the edge, accepts compatibility traffic for HTTP/1.1 and HTTP/2 clients, and forwards requests to existing upstream services through its canonical request, routing, policy, and transport pipeline.

## What Impulse Does

Impulse is built to:

- terminate QUIC connections with TLS 1.3
- convert HTTP/3 streams into upstream requests
- distribute load across upstream backends with active health checks
- route requests based on path prefix and hostname patterns

## Architecture

```
HTTP/3 Client → QUIC/TLS → Impulse Edge → HTTP/2 → Backend Servers
```

**Core Components:**

- **Edge**: QUIC termination and HTTP/3 session management
- **Bridge**: Protocol conversion between HTTP/3 and HTTP/2
- **Transport**: HTTP/2 connection pooling and lifecycle management
- **Load Balancer**: Backend selection algorithms and health tracking
- **Router**: Path and host-based request routing

## Key Features

**Protocol Support**
- HTTP/3 and QUIC (RFC 9114, RFC 9000)
- TLS 1.3 with certificate chain validation
- Scheme-driven upstream connectivity: HTTP/2 for `https://` backends and HTTP/1.1 for `http://` backends

**Load Balancing**
- Random distribution
- Round-robin rotation (default)
- Consistent hashing with weighted virtual nodes
- Per-upstream strategies with optional global fallback default

**Routing**
- Path prefix matching with longest-match selection
- Host-based routing
- Multiple upstreams with independent configurations

**Health Management**
- Active HTTP health checks with configurable intervals
- Automatic backend removal on failure threshold
- Cooldown periods for recovery

## System Requirements

**Runtime Requirements:**
- Rust 1.85 or later (edition 2024)
- Linux (runtime supported; macOS and Windows may compile but are not supported for production use)
- UDP port access for QUIC traffic
- 256MB RAM minimum (1GB recommended for production)

**Build Dependencies:**

```bash
# Ubuntu/Debian
sudo apt install cmake build-essential pkg-config

# macOS
brew install cmake pkg-config
```

## Quick Start

```bash
# Clone and build
git clone https://github.com/Supernova-Labs-Org/impulse.git
cd impulse
cargo build --release

# Generate certificates
make certs-selfsigned

# Start proxy
./target/release/impulse --config config/config.development.yaml
```

## Configuration Example

Impulse uses YAML configuration with validation at startup:

```yaml
version: 1

listen:
  protocol: http3
  port: 9889
  address: "0.0.0.0"
  tls:
    cert: "certs/proxy-cert.pem"
    key: "certs/proxy-key-pkcs8.pem"

upstream:
  api_backend:
    load_balancing:
      type: "round-robin"
    route:
      path_prefix: "/api"
    backends:
      - id: "api-1"
        address: "127.0.0.1:8001"
        weight: 100
        health_check:
          path: "/health"
          interval: 5000

  default_backend:
    load_balancing:
      type: "random"
    route:
      path_prefix: "/"
    backends:
      - id: "default-1"
        address: "127.0.0.1:8080"
        weight: 100

log:
  level: info
```

## Testing Connectivity

Verify the proxy is functioning with an HTTP/3 client:

```bash
curl --http3-only -k \
  --resolve proxy.example.com:9889:127.0.0.1 \
  https://proxy.example.com:9889/api/health
```

## Project Status

**Impulse is in beta.** Core features are implemented and functional, and the project is suitable for controlled production rollout. The project is still pre-GA, so expect continued hardening and targeted breaking changes where needed.

Currently working:

- QUIC termination and HTTP/3 support
- upstream HTTP/2 forwarding for `https://` backends and HTTP/1.1 forwarding for `http://` backends
- Multiple load balancing algorithms
- Active health checking with automatic recovery
- Path, host, and method-aware routing with named upstreams

See [Release Maturity](../release-maturity.md) for beta scope and GA promotion criteria.

## Next Steps

- [Installation Guide](installation.md) - complete installation instructions
- [Configuration Reference](../configuration/reference.md) - exact configuration schema and defaults
- [TLS Setup](../configuration/tls.md) - certificate generation and trust configuration
- [Production Deployment](../deployment/production.md) - deployment and rollout guidance
- [Operations Overview](../operations/overview.md) - where to go once you are ready to operate Impulse
- [Troubleshooting](../troubleshooting/common-issues.md) - common failure signatures and checks
