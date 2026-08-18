Spooky is a modern edge runtime for high-trust APIs. This documentation set is organized so readers can quickly answer five questions:

- what Spooky is
- where to start
- where to deploy and operate it
- where to troubleshoot issues
- where exact product reference lives

---

## Start Here

| Goal | Go to |
| --- | --- |
| Understand the product | [README](../README.md) and [Getting Started Overview](getting-started/overview.md) |
| Install and run Spooky | [Getting Started](getting-started/overview.md) |
| Prepare for deployment | [Operations Overview](operations/overview.md) and [Production Deployment](deployment/production.md) |
| Troubleshoot issues | [Common Issues](troubleshooting/common-issues.md) and [Runbook](operations/runbook.md) |
| Find exact supported behavior | [Reference Overview](reference/overview.md) |

## Documentation Paths

### Operator — install, configure, run in production

| Document | What you'll find |
|---|---|
| [Installation](getting-started/installation.md) | Debian package, build from source, system requirements, TLS certificate layout |
| [Docker](getting-started/docker.md) | Container image, Compose bootstrap, smoke-test scripts |
| [Configuration Reference](configuration/reference.md) | Every config key, type, default, and constraint in one place |
| [TLS Setup](configuration/tls.md) | Certificate generation, mTLS client auth, key ownership and permissions |
| [Production Deployment](deployment/production.md) | Systemd unit, privilege drop, sysctl tuning, canary rollout guidance |
| [Production Readiness](operations/production-readiness.md) | Canonical statement of what is production-ready today and what still blocks GA |
| [Operations Overview](operations/overview.md) | Main entry point for deployment, rollout, observability, and failure handling |
| [Troubleshooting](troubleshooting/common-issues.md) | Symptom-driven diagnostics and operator checks |
| [Limitations](reference/limitations.md) | The current hard product limits, without marketing language |

### Architecture — understand the runtime and subsystem ownership

| Document | What you'll find |
|---|---|
| [Architecture Overview](architecture/overview.md) | Architecture entry point, shared product flow, ingress model, and runtime boundaries |
| [Request Lifecycle](architecture/request-lifecycle.md) | Canonical flow from intake through admission, routing, transport, and outcome recording |
| [Bootstrap vs QUIC](architecture/bootstrap-vs-quic.md) | Exact boundary between the native HTTP/3 path and the compatibility ingress path |
| [Transport Boundary](architecture/transport.md) | What transport owns, what edge owns, and how H1/H2 execution stays hidden behind one facade |
| [Backend Lifecycle](architecture/backend-lifecycle.md) | Backend identity, resolution, health, membership, and operator-visible lifecycle state |
| [Runtime Generation Model](architecture/runtime-generation.md) | How runtime reload, active generations, and shared services work |
| [Component Breakdown](architecture/components.md) | Per-crate responsibilities, inter-crate boundaries, key types |
| [Distributed Quota Contract](architecture/quota-policy-contract.md) | Semantic contract for quota semantics, selector composition, and distributed counter behavior |
| [Codebase Map](development/codebase-map.md) | Current crate/module map and where major logic lives |
| [Development Invariants](development/invariants.md) | Core runtime invariants, ownership assumptions, and rules the code depends on |
| [Public API Surface Inventory](public-api-surface-inventory.md) | Current canonical public surfaces, hidden internals, and remaining intentional exports |

### Control API and Operations — runtime control, observability, and failure handling

| Document | What you'll find |
|---|---|
| [API Overview](api/overview.md) | Metrics endpoint and Control API surfaces at a high level |
| [Control API Reference](reference/control-api-reference.md) | Endpoint-by-endpoint control API contract |
| [Metrics Reference](reference/metrics-reference.md) | Metric names, labels, and exported runtime signals |
| [Operations Overview](operations/overview.md) | Operator map for deployment, sizing, tuning, and failure handling |
| [Distributed Quota](operations/distributed-quota.md) | Distributed quota policy examples, Redis setup, degraded-mode guidance, and operator interpretation |
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
| [Reference Overview](reference/overview.md) | Main entry point for exact behavior, product limits, and authoritative reference pages |
| [Configuration Reference](configuration/reference.md) | Authoritative schema reference for every configuration block |
| [Feature Matrix](reference/feature-matrix.md) | Strict feature-by-feature inventory of what is done, partial, and missing |
| [Roadmap](roadmap.md) | Planned features, GA exit criteria, known limitations |
| [Changelog](changelog.md) | Version history with added, fixed, and changed entries |

---

## Status

| Field | Value |
|---|---|
| Version | v0.5.1-beta |
| Maturity | Beta |
| License | GPLv3 |

Beta means core proxying, routing, load balancing, and health-check features are implemented and actively validated, but the project remains pre-GA — extended soak validation and broader failure-mode hardening are still in progress.

Controlled production rollout is supported. See [release-maturity.md](release-maturity.md) for operator expectations, environment guidance, and GA exit criteria.

---

## Quick reference

If you are in a hurry:

- first run: [getting-started/overview.md](getting-started/overview.md)
- production deployment: [deployment/production.md](deployment/production.md)
- incident response: [operations/runbook.md](operations/runbook.md)
- troubleshooting: [troubleshooting/common-issues.md](troubleshooting/common-issues.md)
- exact support surface: [reference/feature-matrix.md](reference/feature-matrix.md)

For the canonical examples and exact commands:

- working config snippets: [configuration/examples.md](configuration/examples.md)
- full config semantics: [configuration/reference.md](configuration/reference.md)
- Control API and metrics examples: [api/overview.md](api/overview.md)
- log levels and logging config: [configuration/reference.md](configuration/reference.md#logging-configuration)

---

## External standards

- [RFC 9000 — QUIC: A UDP-Based Multiplexed and Secure Transport](https://www.rfc-editor.org/rfc/rfc9000.html)
- [RFC 9114 — HTTP/3](https://www.rfc-editor.org/rfc/rfc9114.html)
- [RFC 9113 — HTTP/2](https://www.rfc-editor.org/rfc/rfc9113.html)
