# Feature Matrix

Use this page to check whether a feature is implemented, partially implemented, or missing.

Status legend:

- `Done`: implemented and supported in the current product
- `Partial`: implemented with important limits or caveats
- `Missing`: not implemented as a first-class feature

## Quick Lookup

| If you need to check | Start here |
| --- | --- |
| protocol support | [Protocols](#protocols) |
| route matching | [Routing](#routing) |
| balancing strategy | [Load Balancing](#load-balancing) |
| TLS or trust behavior | [TLS And Trust](#tls-and-trust) |
| resilience and protection features | [Resilience And Safety](#resilience-and-safety) |
| runtime control and discovery | [Control Plane And Discovery](#control-plane-and-discovery) |
| auth, policy, and platform features | [Policy, Security, And Platform Features](#policy-security-and-platform-features) |
| observability and packaging | [Observability And Ops](#observability-and-ops) |

## Runtime Foundations

These items are not user-facing features, but they are important product foundations.

| Area | Status | Notes |
| --- | --- | --- |
| Request admission and policy evaluation | `Done` | Shared flow covers auth, scoped rate limits, quota, brownout, overload, and rejection mapping |
| Route resolution and backend selection pipeline | `Done` | Shared route match, upstream lookup, LB strategy, backend selection, and resolution telemetry flow |
| Canonical load-balancing key resolution | `Done` | Cookie/query/header/bearer/CID/peer-IP extraction plus shared fallback behavior |
| Shared upstream error classification | `Done` | Timeout/transport/TLS/protocol normalization with shared health and retry interpretation |
| Canonical request building | `Done` | Host policy, forwarded-header policy, and shared H1/H2 request construction live behind `bridge` |
| Canonical response normalization | `Done` | Hop-header stripping, trailer handling, bodyless/no-content shaping, and bootstrap/QUIC parity |
| External auth decision layer | `Done` | Timeout/failure-mode handling, allowlist filtering, mutation safety, deny/challenge/redirect mapping |
| Runtime generation model | `Done` | Startup-owned, generation-owned, and shared runtime ownership boundaries are formalized |
| Unified backend lifecycle model | `Done` | DNS refresh, health observations, request feedback, pool placement, and lifecycle snapshots are coordinated |
| Canonical transport façade | `Done` | Edge depends on transport execution contract instead of H1/H2 pool details |
| Listener/runtime façade split | `Done` | `quic_listener` and bootstrap paths are decomposed into orchestration-oriented modules |
| Public API and visibility hardening | `Done` | Canonical crate facades and reduced accidental exports across edge, config, bridge, lb, transport, and errors |

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
| Weighted backends | `Partial` | Supported for consistent-hash, sticky-CID, round-robin, and random; least-connections and latency-aware reject custom weights |
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
| Rate limiting and quota | `Done` | Scoped local rules plus distributed quota policy with burst and sustained contracts and Redis-backed counters |

## Control Plane And Discovery

| Area | Status | Notes |
| --- | --- | --- |
| Health endpoint | `Done` | Implemented |
| Readiness endpoint | `Done` | Implemented |
| Runtime status endpoint | `Done` | Implemented |
| Restart endpoint | `Done` | Implemented |
| Cert reload endpoint | `Done` | Implemented |
| Full config hot reload | `Partial` | Broad runtime swap exists; startup-owned settings and bind/topology changes still require restart or explicit compatibility handling |
| Dynamic route updates | `Partial` | Supported through runtime activation or reload, not through a dedicated per-route mutation API |
| Dynamic upstream membership API | `Missing` | No first-class API |
| DNS refresh | `Done` | Implemented for hostname-based backends |
| Rich service discovery | `Missing` | No Kubernetes/xDS/Consul-class discovery |

## Policy, Security, And Platform Features

| Area | Status | Notes |
| --- | --- | --- |
| Header mutation for forwarding policy | `Done` | Host and forwarded-header policy exists |
| Generic request/response rewrite engine | `Missing` | Not a broad filter system |
| API key auth | `Done` | Per-upstream, local fast path |
| JWT validation | `Done` | Local `HS256`/`RS256`/`ES256` validation, per-upstream; explicit algorithm allowlist |
| JWKS key sources | `Done` | Static PEM/JWK keys or remote JWKS URL, background refresh, rollover overlap, last-known-good retention |
| RBAC / policy engine | `Partial` | Scope/role requirements enforced against JWT claims only |
| External auth integration | `Done` | Async HTTP subrequest per upstream with fail-open or fail-closed behavior |
| OIDC / auth gateway | `Partial` | Discovery and token introspection only; no interactive login or session-cookie flows. Local signature validation is available through JWT `jwks_url` |
| WAF capabilities | `Missing` | Not implemented |
| Plugin / extension model | `Missing` | Not implemented |

## Observability And Ops

| Area | Status | Notes |
| --- | --- | --- |
| Prometheus metrics | `Done` | Rich built-in metrics |
| Structured logging | `Done` | Plain and JSON formats |
| Canonical observability vocabulary | `Done` | Shared reason slugs align metrics, logs, and control-plane snapshot fields |
| Control-plane runtime snapshots | `Done` | Backend lifecycle, runtime generation, watchdog, TLS, and coarse metrics summary exposed through shared runtime views |
| Admin-plane RBAC | `Done` | `viewer` / `operator` / `admin` enforced per control API route; `401` and `403` are distinct |
| Control API mTLS | `Done` | `disabled` / `optional` / `required`, with a client verifier scoped to the control API endpoint |
| Admin audit event stream | `Done` | Stable JSON schema over a dedicated log target or file sink; attempt/result pairs for mutating actions |
| Admin source-IP allowlisting | `Done` | CIDR gate runs before credential validation; TCP peer address only |
| OTLP tracing hooks | `Done` | Optional |
| Packaging for Docker | `Done` | Present |
| Packaging for Debian/systemd | `Done` | Present |
| Benchmark suite | `Done` | Dedicated crate and scripts |
| Production runbook maturity | `Partial` | Present, but still being tightened |

## Related Pages

- [Production Readiness](../operations/production-readiness.md)
- [Limitations](limitations.md)
- [Security Model](../concepts/security-model.md)
