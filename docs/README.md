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
| [Production Readiness](operations/production-readiness.md) | Canonical statement of what is production-ready today and what still blocks GA |
| [Limitations](reference/limitations.md) | The current hard product limits, without marketing language |

### Architecture — understand the runtime and subsystem ownership

| Document | What you'll find |
|---|---|
| [Architecture Overview](architecture/overview.md) | Design principles, data-plane topology, sharded ingress model |
| [Component Breakdown](architecture/components.md) | Per-crate responsibilities, inter-crate boundaries, key types |
| [Codebase Map](development/codebase-map.md) | Current crate/module map and where major logic lives |
| [Development Invariants](development/invariants.md) | Core runtime invariants, ownership assumptions, and rules the code depends on |
| [Public API Surface Inventory](public-api-surface-inventory.md) | Current canonical public surfaces, hidden internals, and remaining intentional exports |

### Control Plane and Operations — runtime control, observability, and failure handling

| Document | What you'll find |
|---|---|
| [API Overview](api/overview.md) | Metrics endpoint and control-plane HTTP surfaces at a high level |
| [Control API Reference](reference/control-api-reference.md) | Endpoint-by-endpoint control API contract |
| [Metrics Reference](reference/metrics-reference.md) | Metric names, labels, and exported runtime signals |
| [Operations Overview](operations/overview.md) | Operator map for deployment, sizing, tuning, and failure handling |
| [Runbook](operations/runbook.md) | Day-2 operational procedures and troubleshooting flow |
| [Failure Modes](operations/failure-modes.md) | Expected degraded behaviors and what they mean operationally |
| [Sizing and Capacity](operations/sizing-and-capacity.md) | Capacity planning and scaling guidance |

### Protocol, traffic, and policy reference

| Document | What you'll find |
|---|---|
| [Load Balancing](user-guide/load-balancing.md) | Current balancing strategies, selection behavior, and config examples |
| [HTTP/3](protocols/http3.md) | HTTP/3 behavior and protocol-specific operational notes |
| [QUIC](protocols/quic.md) | QUIC transport behavior, constraints, and terminology |
| [Security Model](concepts/security-model.md) | Current trust boundaries, admin-plane assumptions, and missing security layers |
| [Terminology](reference/terminology.md) | Canonical definitions for listener, upstream, backend, route, drain, and related terms |

### Developer — contribute safely against the current architecture

| Document | What you'll find |
|---|---|
| [Contributing Guide](https://github.com/Supernova-Labs-Org/spooky/blob/master/CONTRIBUTING.md) | Dev setup, build commands, test matrix, PR conventions |
| [Development Overview](development/overview.md) | Contributor-oriented guide to working in the repo |
| [Testing Strategy](development/testing-strategy.md) | Contract, regression, and parity test expectations |
| [Benchmarking](development/benchmarking.md) | Benchmark crate, micro/macro suites, and regression-gate workflow |
| [Adding Features](development/adding-features.md) | Expectations for new features against the current architecture |

### Reference — schema, maturity, roadmap, and release state

| Document | What you'll find |
|---|---|
| [Configuration Reference](configuration/reference.md) | Authoritative schema reference for every configuration block |
| [Feature Matrix](reference/feature-matrix.md) | Strict feature-by-feature inventory of what is done, partial, and missing |
| [Reference Overview](reference/overview.md) | Index of the strict reference material and product limits |
| [Roadmap](roadmap.md) | Planned features, GA exit criteria, known limitations |
| [Changelog](changelog.md) | Version history with added, fixed, and changed entries |

---

## Status

| Field | Value |
|---|---|
| Version | v0.3.1-beta |
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
curl -k https://127.0.0.1:9902/health
curl -k https://127.0.0.1:9902/ready
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
