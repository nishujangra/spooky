# Production Readiness

This page is the canonical statement of what Impulse supports in production today, what requires extra rollout discipline, and what still remains outside the safe default operating envelope.

## Current Assessment

Impulse is a **beta HTTP/3-first edge runtime**. It is suitable for **controlled production rollout** when operators use:

- staged activation and explicit rollback
- protected Control API access
- observability and alerting before traffic expansion
- canary or bounded-slice rollout patterns

Impulse is not yet a general-purpose replacement for every reverse-proxy or API-gateway use case.

## Strong Production Areas

These areas are strong enough for controlled production use today:

- downstream HTTP/3 ingress over QUIC
- downstream bootstrap HTTP/1.1 and HTTP/2 ingress
- upstream HTTP/2 forwarding for `https://` backends and HTTP/1.1 forwarding for `http://` backends
- deterministic host, path, and method routing
- active and passive backend health handling
- load balancing with round-robin, random, consistent-hash, least-connections, latency-aware, and sticky-CID behavior
- downstream TLS termination with SNI certificate selection
- upstream TLS verification controls and custom trust roots
- overload handling through inflight limits, queue caps, adaptive admission, brownout, and circuit-open rejection
- scoped rate limiting plus distributed quota enforcement with burst and sustained contracts
- Prometheus metrics, structured logs, tracing, and Control API runtime views
- graceful drain, bounded shutdown, retained runtime generations, and rollback

## Production-Capable With Rollout Discipline

These capabilities are usable in production, but operators should apply them through staged rollout and explicit validation:

- runtime config activation through `POST /admin/runtime/validate`, `/preview`, and `/activate`
- rollback through retained runtime generations and `POST /admin/runtime/rollback`
- certificate-only reload for new handshakes through `POST /admin/runtime/reload-certs`
- DNS refresh and backend client rotation
- retry, hedge, and circuit-breaker policy
- watchdog-driven restart workflows
- packet sharding, worker pinning, and other host-tuning options

These are production features, but they still need disciplined rollout and observability rather than blind automation.

## Restart-Required Areas

Operators should plan a drain-aware restart or instance replacement workflow for changes in these areas:

- listener removal or bind-address changes
- control-plane or metrics bind changes
- logging sink settings such as log format and file-output shape
- tracing startup configuration
- control-plane thread-count changes

`log.level` remains live-reloadable. Do not group it with restart-only logging settings.

## Important Product Boundaries

The main product boundaries today are:

- no upstream HTTP/3 forwarding mode
- no orchestration-native service discovery beyond DNS refresh
- no broad request-mirroring or advanced traffic-splitting engine
- no generic policy engine beyond the current auth, quota, rate-limit, and overload surfaces
- no interactive OIDC session-login flow or broad gateway-style auth orchestration

## Good Fit Today

Impulse is a good fit when:

- you want HTTP/3 at the edge
- your upstreams speak HTTP/2 or HTTP/1.1
- one team can own routing, TLS, rollout, and incident response
- you are comfortable using a file-driven config source with staged activation
- you can keep close observability on latency, overload, quota, and backend health

## Weaker Fit Today

Impulse is a weaker fit when you need:

- a rich dynamic control plane with per-object mutation APIs
- broad multi-tenant platform policy orchestration
- very wide upstream protocol compatibility
- deep service-mesh style discovery and interior control-plane integration

## Minimum Operator Bar

Do not expand rollout beyond a controlled slice unless all of the following are true:

- rollback has been tested recently
- Control API access is protected
- dashboards and alerts are live and verified
- backend health and latency behavior have been observed under real traffic
- the team knows which changes activate live and which require a restart

## Maturity Gates Before Broader Adoption

The main gates before a broader production-grade claim are:

1. Longer-horizon operational validation of activation, rollback, and restart workflows under churn.
2. Continued parser and protocol hardening, including deeper fuzzing.
3. Broader maturity of service discovery, policy depth, and control-plane ergonomics.
4. Further decomposition of concentrated runtime code and more long-horizon fleet history.

## Related Pages

- [Production Deployment](../deployment/production.md)
- [Reload and Drain](reload-and-drain.md)
- [Deployment Patterns](deployment-patterns.md)
- [Feature Matrix](../reference/feature-matrix.md)
- [Limitations](../reference/limitations.md)
