# Production Readiness

This page is the canonical statement of what the project currently supports in production, what remains partial, and what is still missing.

## Current Assessment

Spooky is a **beta HTTP/3 edge reverse proxy** with a strong core data plane, broad correctness and regression coverage, and clear operational ambition. It is suitable for **controlled production rollout** when the operator keeps rollback, monitoring, and staged traffic expansion in place.

Spooky is **not yet a fully mature general-purpose reverse proxy platform**. The main constraints are:

- full configuration hot reload is not implemented
- upstream forwarding is scheme-driven: HTTP/2 for `https://` backends, HTTP/1.1 for `http://` backends
- dynamic control-plane capability is limited
- first-class auth, rate-limiting, and policy features are not yet present
- service discovery is limited to DNS refresh rather than a richer orchestration-native model

## Production-Ready Today

The following areas are considered strong enough for controlled production use:

- downstream HTTP/3 ingress over QUIC
- downstream HTTP/1.1 and HTTP/2 bootstrap ingress
- upstream HTTP/2 forwarding (`https://` backends) and HTTP/1.1 forwarding (`http://` backends)
- deterministic host/path/method routing
- active and passive backend health handling
- load balancing with round-robin, random, consistent-hash, least-connections, latency-aware, and sticky-CID behavior
- downstream TLS termination with SNI certificate selection
- bootstrap listener client-auth support
- upstream TLS verification controls and custom trust roots
- overload handling through inflight limits, queue caps, adaptive admission, and brownout logic
- graceful drain and bounded shutdown behavior
- Prometheus metrics and control-plane health/readiness/runtime endpoints

## Production-Capable With Caveats

The following capabilities exist, but operators should treat them as features that still need careful rollout discipline:

- certificate reload for new handshakes, without a full process restart
- watchdog-driven recovery hooks
- DNS-based backend refresh and backend client rotation
- retry budget, circuit breaker, and hedging controls
- packet sharding, worker pinning, and other host-tuning features

These areas are usable, but their surrounding operational model is not yet as mature as top-tier long-established proxies.

## Not Yet Production-Complete

The following gaps are the most important reasons Spooky is not yet at general-availability maturity:

- no full config hot reload for routes, upstreams, limits, or policies
- no transactional config apply, staged activation, or rollback API
- no upstream HTTP/3 forwarding mode
- no broad request mirroring, canary traffic splitting, or advanced traffic policy engine
- no first-class rate limiting framework
- no built-in JWT, OIDC, external auth, API key, or RBAC layer
- no broad plugin or extension system
- no orchestration-native service discovery beyond DNS polling

## Recommended Operator Stance

Use Spooky today when all of the following are true:

- you want an HTTP/3-first edge proxy
- your backends speak HTTP/2 (`https://`) or HTTP/1.1 (`http://`), or a mix of both
- you are comfortable with staged rollout and explicit rollback procedures
- you do not require a large dynamic control plane yet
- you can keep close operational visibility on the system

Do not position Spooky today as:

- a full Envoy-class dynamic proxy platform
- a drop-in NGINX replacement for every protocol and legacy deployment shape
- a complete API gateway or auth gateway
- a broad service-mesh data plane with mesh-native control-plane integration

## Readiness Gates Before Broader Adoption

The most important gates before calling the project broadly production-grade are:

1. Full configuration hot reload.
2. Dynamic route and upstream updates without restart.
3. Refactoring of the oversized edge runtime into smaller subsystems.
4. Fuzzing and deeper parser/protocol hardening.
5. First-class rate limiting and auth/policy features.
6. Better operator guidance for rollout, recovery, and ongoing operations.

## Related Pages

- [Feature Matrix](../reference/feature-matrix.md)
- [Limitations](../reference/limitations.md)
- [Security Model](../concepts/security-model.md)
- [Production Deployment](../deployment/production.md)
- [Release Maturity](../release-maturity.md)
