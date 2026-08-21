# Deployment Patterns

This page describes the deployment shapes that fit Impulse best today and the rollout patterns that keep operations safe.

## Best-Fit Product Patterns

### HTTP/3 Edge to HTTP/2 or HTTP/1.1 Service Tier

Best current fit.

Use this when:

- clients need HTTP/3 at the edge
- services speak HTTP/2, HTTP/1.1, or a mix of both
- you want explicit admission, quota, overload, and backend-health controls

### Single-Team Edge Tier

Good fit when one team owns:

- route and upstream configuration
- TLS and certificate rotation
- host tuning and capacity
- rollout and incident response

### Finite Multi-Service Platform Edge

Good fit when:

- a platform team owns a bounded set of services
- config is rendered and activated through controlled automation
- there is strong observability and clear rollback ownership

## Best-Fit Rollout Patterns

### Canary or Bounded Traffic Slice

Recommended default rollout model.

Use it when:

- you need to validate config or binary changes gradually
- you can keep a rollback path warm
- you want to limit blast radius during beta operations

### Active-Active Edge Pool

Recommended for production availability.

Use it when:

- multiple edge nodes can serve the same traffic class
- your upstream load balancer or traffic manager can remove draining or unhealthy nodes
- you want binary upgrades and restart-required changes without whole-fleet impact

### Blue-Green or Node-Replacement Rollout

Recommended for:

- binary upgrades
- restart-required config changes
- high-assurance operational windows

Use it when replacing nodes is safer than mutating them in place.

## Weaker-Fit Patterns

### Dynamic Fleet-Managed Multi-Tenant Platform

Weaker fit today because:

- config is file-driven rather than a rich object-level control plane
- policy depth is still narrower than a broad platform proxy
- service discovery is DNS-based rather than orchestration-native

### Broad Legacy Compatibility Proxy

Weaker fit today because:

- Impulse is strongest on HTTP/3 edge ingress and H2 or H1 upstream execution
- very broad legacy protocol breadth is not the main product strength

### Full API Gateway Replacement

Partial fit today because Impulse already ships:

- per-upstream auth
- scoped rate limiting
- distributed quota
- observability and operator-facing control surfaces

It is still weaker than a full gateway platform because:

- request and response transformation depth is limited
- there is no broad generic policy engine
- auth-provider chaining and gateway orchestration remain narrower

## Recommended Rollout Shape

1. Start with one service, one route family, or one bounded traffic slice.
2. Keep a known-good config generation and known-good binary ready for rollback.
3. Use staged activation for runtime-managed config changes.
4. Use cert reload only for cert-only changes.
5. Use drain-aware restart or node replacement for restart-required changes.
6. Expand only after latency, backend health, overload, and quota behavior remain stable.
