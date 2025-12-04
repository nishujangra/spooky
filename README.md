# Spooky

**The first production-ready HTTP/3 load balancer written in Rust**

Spooky is built for the modern web: QUIC-native, fast, and designed for environments where nginx's experimental HTTP/3 support isn't enough.

---

## Why Spooky?

---

## Features

- HTTP/3 (QUIC) protocol
- Load balancing: weight-based, least connection, IP hash, round-robin
- Health monitoring with configurable intervals
- TLS support with custom certificates
- Dynamic weight updates via `/metric` endpoints

## Quick Start

```bash
# Build
cargo build

# Run
cargo run --bin spooky

# Want sample http3 server on port 7001
cargo run --bin server -- --port 7001

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

ELv2 - see [LICENSE](LICENSE.md)