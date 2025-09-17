# Spooky

High-performance load balancer and proxy server in Rust. Supports HTTP/3 (QUIC) and gRPC with intelligent routing and health monitoring.

## Features

- HTTP/3 (QUIC) and gRPC protocols
- Load balancing: weight-based, least connection, IP hash, round-robin
- Health monitoring with configurable intervals
- TLS support with custom certificates
- Dynamic weight updates via `/metric` endpoints

## Quick Start

```bash
# Build
cargo build

# Run with config
./target/debug/spooky --config config.yaml
```

## Configuration

```yaml
listen:
  protocol: http3
  port: 9889
  address: "0.0.0.0"
  tls:
    cert: /path/to/cert
    key: /path/to/key

backends:
  - id: "backend1"
    address: "10.0.1.100:8080"
    weight: 100
    health_check:
      path: "/health"
      interval: 5s

load_balancing:
  type: weight-based
```

You can generate certificates using [gen-cert.md](docs/gen-cert.md)

## Check if Proxy is Running

```bash
# Check UDP port 9889
sudo netstat -tulnp | grep 9889
sudo ss -tulnp | grep 9889
```

## License

AGPLv3 - see [LICENSE](LICENSE.md)