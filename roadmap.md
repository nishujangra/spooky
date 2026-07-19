# Roadmap

This roadmap is intentionally practical. It is organized around what most increases production trust and product value, not around speculative breadth.

## Current Position

Spooky is strongest today as:

- an HTTP/3-first edge proxy
- a deterministic H3-to-H2 routing and balancing layer
- a proxy with strong resource-bound, teardown, and overload behavior

Spooky is not yet strongest today as:

- a dynamic control-plane-driven fleet proxy
- a broad protocol-compatibility proxy
- a full API gateway
- an extensible filter platform

## Near-Term Priorities

These are the highest-value areas for the next phase of maturity.

### 1. Complete Configuration Hot Reload

Full config hot reload is **shipped** (`POST /admin/runtime/reload`): routes, upstreams, backends,
timeouts and limits, resilience policies, and observability endpoint changes apply live via an
atomic runtime swap. The remaining work is to close the restart-only gaps:

- listener removal and bind-address changes (listener *addition* is already live)
- startup-owned settings: log file/format, tracing config, control-plane thread counts
  (`log.level` already reloads live)

### 2. Dynamic Config Safety

Add a stronger control-plane model with:

- validation before apply
- dry-run support
- config diff visibility
- atomic activation
- rollback to a known-good generation

### 3. Edge Runtime Refactor

Break the large edge runtime into smaller subsystems so future work is safer:

- ingress worker layer
- connection/CID management
- request validation
- routing and backend selection
- admission and overload control
- upstream dispatch
- response streaming
- drain and shutdown control

### 4. Security Hardening

Increase trust in the critical-path parser and protocol handling with:

- fuzzing
- deeper negative-case coverage
- tighter admin-plane guidance
- explicit trust-boundary validation

## Medium-Term Priorities

These areas make Spooky far more competitive as a general production reverse proxy.

### 5. Broader Upstream Compatibility

- first-class upstream HTTP/1.1 support (shipped in v0.3.0-beta)
- better CONNECT handling
- broader WebSocket and upgrade support

### 6. Traffic-Management Depth

- weighted route splitting
- request mirroring
- richer release controls
- better policy-driven request routing

### 7. Operator Features

- distributed / cross-instance rate limiting (scoped per-instance rate limiting already ships)
- stronger capacity guidance
- more complete runbooks and alerts
- better runtime visibility for why requests were shed, retried, or rerouted

### 8. Auth And Policy Features

- JWKS / asymmetric JWT validation (RS256/ES256) — local JWT validation ships today but is HS256-only
- stronger route-level policy controls and layered/chained auth providers

_Already shipped (previously listed here as future): scoped rate limiting (route/client/tenant/token),
local HS256 JWT validation with scope/role RBAC, and external auth via HTTP subrequest or OIDC._

## Longer-Term Competitive Priorities

These areas are what move Spooky from “strong specialized edge proxy” toward “top-tier proxy platform.”

### 9. Discovery And Platform Integration

- richer service discovery beyond DNS refresh
- better Kubernetes-native deployment integration
- stronger fleet-management story

### 10. Extensibility

- a safe extension model
- clearer internal subsystem boundaries that make feature growth sustainable

### 11. Ecosystem Proof

- interoperability validation across more clients and upstream stacks
- broader production history
- stronger release-process guarantees

## Non-Goals Today

The following are not current core strengths and should not be assumed:

- full service-mesh control-plane behavior
- built-in WAF capability
- full API-gateway parity with dedicated gateway products
- broad plugin ecosystem

## Related Pages

- [Production Readiness](operations/production-readiness.md)
- [Feature Matrix](reference/feature-matrix.md)
- [Limitations](reference/limitations.md)
