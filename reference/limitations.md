# Limitations

This page lists the most important current product limits so operators and contributors do not have to infer them from scattered documents.

## Architectural Limits

- Spooky is centered on **HTTP/3 downstream and HTTP/2 upstream**.
- It is not yet a broad multi-protocol reverse proxy in the same class as older general-purpose incumbents.
- It is not yet a dynamic control-plane-driven proxy platform.

## Configuration And Control Plane Limits

- Full configuration hot reload is not implemented.
- Dynamic route updates are not implemented as a first-class runtime feature.
- Dynamic upstream membership changes are limited to DNS refresh rather than a richer control-plane API.
- There is no transactional apply, generation diff, rollback, or staged config activation model.

## Protocol Limits

- Upstream HTTP/1.1 forwarding is not implemented as a first-class path.
- Upstream HTTP/3 forwarding is not implemented.
- CONNECT support exists only as a constrained policy feature, not as a broad proxy capability.
- WebSocket and upgrade handling are limited and are not yet a full-feature parity surface.

## Traffic-Management Limits

- No route-level weighted traffic splitting.
- No request mirroring or shadow traffic.
- No built-in fault injection layer.
- No full request/response rewrite/filter pipeline.

## Security And Policy Limits

- No JWT validation.
- No OIDC or full auth gateway feature set.
- No external authorization filter.
- No RBAC or generic policy engine.
- No WAF or advanced request-inspection layer.

## Platform And Ecosystem Limits

- No Kubernetes-native control plane or operator.
- No xDS-style fleet management.
- No plugin or extension model.
- No service-mesh positioning or mesh-native runtime integration.

## Engineering Limits

- The central edge runtime remains concentrated in a very large module.
- This increases change risk and makes long-term feature growth harder.
- Some docs and operational guidance still need tighter separation between stable behavior and future intent.

## What These Limits Mean In Practice

Spooky is a strong candidate when:

- HTTP/3 edge performance and correctness are primary goals
- the upstream environment is compatible with HTTP/2
- rollout discipline and restarts are acceptable for config changes
- the deployment does not require rich traffic policy or auth gateway features

Spooky is a poor fit today when:

- live config mutation is mandatory
- upstream protocol breadth is required
- advanced API gateway behavior is required
- a rich dynamic control plane is expected
- a plugin/filter ecosystem is required

## Related Pages

- [Feature Matrix](feature-matrix.md)
- [Production Readiness](../operations/production-readiness.md)
- [Roadmap](../roadmap.md)
