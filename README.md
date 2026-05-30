Spooky is an open-source HTTP/3 (QUIC) edge reverse proxy written in Rust that terminates QUIC connections and forwards traffic to HTTP/2 backends.

---

## Where to start

### Operator — install, configure, run in production

| Document | What you'll find |
|---|---|
| [Installation](getting-started/installation.md) | Debian package, build from source, system requirements, TLS certificate layout |
| [Docker](getting-started/docker.md) | Container image, Compose bootstrap, smoke-test scripts |
| [Configuration Reference](configuration/reference.md) | Every config key, type, default, and constraint in one place |
| [TLS Setup](configuration/tls.md) | Certificate generation, mTLS client auth, key ownership and permissions |
| [Production Deployment](deployment/production.md) | Systemd unit, privilege drop, sysctl tuning, canary rollout guidance |

### Developer — understand the architecture, contribute

| Document | What you'll find |
|---|---|
| [Architecture Overview](architecture/overview.md) | Design principles, data-plane topology, sharded ingress model |
| [Component Breakdown](architecture/components.md) | Per-crate responsibilities, inter-crate boundaries, key types |
| [Load Balancing](user-guide/load-balancing.md) | All six algorithms with characteristics, use-case guidance, and config examples |
| [Contributing Guide](https://github.com/Supernova-Labs-Org/spooky/blob/master/CONTRIBUTING.md) | Dev setup, build commands, test matrix, PR conventions |

### Reference — config schema, API, benchmarks, changelog

| Document | What you'll find |
|---|---|
| [Configuration Reference](configuration/reference.md) | Authoritative schema reference for every configuration block |
| [API Overview](api/overview.md) | Metrics endpoint, control API (health, ready, runtime), bearer auth |
| [Benchmarks](benchmarks/load.md) | Load test results: throughput, latency percentiles, test environment |
| [Roadmap](roadmap.md) | Planned features, GA exit criteria, known limitations |
| [Changelog](changelog.md) | Version history with added, fixed, and changed entries |

---

## Status

| Field | Value |
|---|---|
| Version | v0.1.1-beta |
| Maturity | Beta |
| License | GPLv3 |

Beta means core proxying, routing, load balancing, and health-check features are implemented and actively validated, but the project remains pre-GA — extended soak validation and broader failure-mode hardening are still in progress.

Controlled production rollout is supported. See [release-maturity.md](release-maturity.md) for operator expectations, environment guidance, and GA exit criteria.

---

## Quick reference

### Minimal working config

```yaml
version: 1

listen:
  address: "0.0.0.0"
  port: 9889
  tls:
    cert: /etc/spooky/certs/fullchain.pem
    key: /etc/spooky/certs/privkey.pem

upstream:
  default:
    route:
      path_prefix: "/"
    backends:
      - id: backend1
        address: "127.0.0.1:8080"
        health_check:
          path: "/health"
          interval: 5000
```

Backends are verified HTTPS by default. To forward to a cleartext HTTP backend, set `upstream_tls.verify_certificates: false` and be aware that a warning is logged at startup. The full schema is in [configuration/reference.md](configuration/reference.md).

### Common commands

**Start the proxy:**
```bash
spooky --config /etc/spooky/config.yaml
```

**Test an HTTP/3 connection** (requires curl built with HTTP/3 support):
```bash
curl --http3-only -k \
  --resolve proxy.example.com:9889:127.0.0.1 \
  https://proxy.example.com:9889/health
```

**Check health and readiness** (control API, default port 9902):
```bash
curl http://127.0.0.1:9902/health
curl http://127.0.0.1:9902/ready
```

### Log levels

Spooky accepts both its own names and the standard equivalents in `log.level` or `RUST_LOG`.

| Spooky name | Standard equivalent | Verbosity |
|---|---|---|
| `whisper` | `trace` | Everything, including internal QUIC events |
| `haunt` | `debug` | Per-request routing, backend selection, health transitions |
| `spooky` | `info` | Startup, shutdown, configuration summary (default) |
| `scream` | `warn` | Recoverable errors, degraded-mode events |
| `poltergeist` | `error` | Fatal or unrecoverable conditions |
| `silence` | `off` | No output |

Set per-crate verbosity with `RUST_LOG` (e.g., `RUST_LOG=spooky_edge=haunt,info`). Output format is controlled by `log.format: plain | json`.

---

## External standards

- [RFC 9000 — QUIC: A UDP-Based Multiplexed and Secure Transport](https://www.rfc-editor.org/rfc/rfc9000.html)
- [RFC 9114 — HTTP/3](https://www.rfc-editor.org/rfc/rfc9114.html)
- [RFC 9113 — HTTP/2](https://www.rfc-editor.org/rfc/rfc9113.html)
