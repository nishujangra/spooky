# Feature Matrix

This matrix describes the current feature surface from the source tree as it exists today.

Legend:

- `Done`: implemented and clearly present in the codebase
- `Partial`: implemented with important caveats or limited scope
- `Missing`: not implemented as a first-class feature

## Protocols

| Area | Status | Notes |
| --- | --- | --- |
| Downstream HTTP/3 | `Done` | Native QUIC/H3 ingress path |
| Downstream HTTP/1.1 | `Done` | Via bootstrap TLS listener |
| Downstream HTTP/2 | `Done` | Via bootstrap TLS listener |
| Upstream HTTP/2 | `Done` | Used for `https://` backends |
| Upstream HTTP/1.1 | `Done` | Used for `http://` backends; mixed H1/H2 pools supported |
| Upstream HTTP/3 | `Missing` | Not implemented |
| gRPC trailers | `Done` | Integration coverage exists |
| Broad WebSocket support | `Partial` | Limited bootstrap-side behavior only |
| General CONNECT proxying | `Partial` | Policy exists, not a broad general-purpose CONNECT platform |

## Routing

| Area | Status | Notes |
| --- | --- | --- |
| Host routing | `Done` | Exact and wildcard matching |
| Path-prefix routing | `Done` | Longest-prefix semantics |
| Method-aware routing | `Done` | Deterministic tie-breaking |
| Deterministic route selection | `Done` | Explicitly defended in implementation and tests |
| Header-based routing | `Missing` | Not a route matcher today |
| Query-based routing | `Missing` | Not a route matcher today |
| Cookie-based routing | `Missing` | Not a route matcher today |
| Weighted route splitting | `Missing` | No route-level traffic policy engine |

## Load Balancing

| Area | Status | Notes |
| --- | --- | --- |
| Round-robin | `Done` | Implemented |
| Random | `Done` | Implemented |
| Consistent-hash | `Done` | Weighted ring rebuild on membership changes |
| Least-connections | `Done` | Implemented |
| Latency-aware | `Done` | EWMA-like scoring plus inflight signal |
| Sticky CID | `Done` | Implemented as a selection mode |
| Weighted backends | `Done` | Implemented |
| Canary rollout controls | `Missing` | No first-class release traffic controls |
| Request mirroring | `Missing` | Not implemented |
| Fault injection | `Missing` | Not implemented |

## TLS And Trust

| Area | Status | Notes |
| --- | --- | --- |
| Downstream TLS termination | `Done` | Core capability |
| SNI certificate selection | `Done` | Multiple certs with fallback behavior |
| Downstream client-auth | `Done` | Optional and required modes on bootstrap listener |
| Upstream TLS verification | `Done` | Safe-by-default when using HTTPS backends |
| Custom upstream CA file | `Done` | Implemented |
| Custom upstream CA dir | `Done` | Implemented |
| TLS cert hot reload | `Done` | New handshakes only |
| Full TLS/runtime live reconfiguration | `Done` | Cert reload & broad runtime exists |

## Resilience And Safety

| Area | Status | Notes |
| --- | --- | --- |
| Active health checks | `Done` | Implemented |
| Passive health signals | `Done` | Implemented |
| Circuit breaker | `Done` | Implemented |
| Retry budget | `Done` | Implemented |
| Hedging | `Done` | Implemented with restrictions |
| Brownout | `Done` | Implemented |
| Adaptive admission | `Done` | Implemented |
| Route queue caps | `Done` | Implemented |
| Global inflight limits | `Done` | Implemented |
| Per-upstream inflight limits | `Done` | Implemented |
| Per-backend inflight limits | `Done` | Implemented |
| Rate limiting | `Done` | scoped (Route/Client/Tenant/Token), returns 429 |

## Control Plane And Discovery

| Area | Status | Notes |
| --- | --- | --- |
| Health endpoint | `Done` | Implemented |
| Readiness endpoint | `Done` | Implemented |
| Runtime status endpoint | `Done` | Implemented |
| Restart endpoint | `Done` | Implemented |
| Cert reload endpoint | `Done` | Implemented |
| Full config hot reload | `Partial` | Startup-owned settings + listner removal, bind still need restart |
| Dynamic route updates | `Done via config reload` | routing index rebuilt + swapped |
| Dynamic upstream membership API | `Missing` | No first-class API |
| DNS refresh | `Done` | Implemented for hostname-based backends |
| Rich service discovery | `Missing` | No Kubernetes/xDS/Consul-class discovery |

## Policy, Security, And Platform Features

| Area | Status | Notes |
| --- | --- | --- |
| Header mutation for forwarding policy | `Done` | Host and forwarded-header policy exists |
| Generic request/response rewrite engine | `Missing` | Not a broad filter system |
| API key auth | `Done` | Per-upstream, local fast path |
| JWT validation | `Done` | Local HS256 validation, per-upstream |
| RBAC / policy engine | `Partial` | Scope/role requirements enforced against JWT claims only |
| External auth integration | `Done` | Async HTTP subrequest per upstream; non-blocking, fail-open/fail-closed configurable |
| OIDC / auth gateway | `Partial` | Discovery + token introspection; no JWKS/local signature validation, no interactive login/session-cookie flows |
| WAF capabilities | `Missing` | Not implemented |
| Plugin / extension model | `Missing` | Not implemented |

## Observability And Ops

| Area | Status | Notes |
| --- | --- | --- |
| Prometheus metrics | `Done` | Rich built-in metrics |
| Structured logging | `Done` | Plain and JSON formats |
| OTLP tracing hooks | `Done` | Optional |
| Packaging for Docker | `Done` | Present |
| Packaging for Debian/systemd | `Done` | Present |
| Benchmark suite | `Done` | Dedicated crate and scripts |
| Production runbook maturity | `Partial` | Present, but should be expanded and tightened |

## Practical Summary

Spooky is strongest today as:

- an HTTP/3-first edge proxy
- a deterministic routing and balancing layer with scheme-driven H1/H2 upstream transport
- a proxy with strong resource-bound and teardown behavior

Spooky is not yet strongest as:

- a general-purpose legacy protocol proxy
- a dynamic fleet-managed control-plane-driven proxy
- a fully featured API gateway
- a highly extensible filter platform

## Related Pages

- [Production Readiness](../operations/production-readiness.md)
- [Limitations](limitations.md)
- [Security Model](../concepts/security-model.md)
