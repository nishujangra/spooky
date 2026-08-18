# Limitations

This page lists the most important current product limits so operators and contributors do not have to infer them from scattered documents.

## Architectural Limits

- Spooky is centered on **HTTP/3 downstream** with scheme-driven upstream transport: `https://` backends use HTTP/2, `http://` backends use HTTP/1.1. Mixed deployments are supported.
- It is not yet a broad multi-protocol reverse proxy in the same class as older general-purpose incumbents.
- It is not yet a dynamic control-plane-driven proxy platform.

## Configuration And Control Plane Limits

- Full configuration hot reload exists for runtime-managed settings, but some startup-owned changes still require a restart.
- Dynamic route updates are not implemented as a first-class runtime feature outside config reload.
- Dynamic upstream membership changes are limited to DNS refresh rather than a richer control-plane API.
- Runtime activation is generation-based and supports validate, preview, activate, history, and rollback flows, but it is still file-backed rather than a granular per-object mutation API.
- Retained runtime generations are bounded rather than preserved indefinitely for deep history or fleet-wide change analysis.

## Protocol Limits

- Upstream HTTP/3 forwarding is not implemented.
- CONNECT support exists only as a constrained policy feature, not as a broad proxy capability.
- WebSocket and upgrade handling are limited and are not yet a full-feature parity surface.

## Traffic-Management Limits

- No route-level weighted traffic splitting.
- No request mirroring or shadow traffic.
- No built-in fault injection layer.
- No full request/response rewrite/filter pipeline.

## Security And Policy Limits

- JWT validation is local and per-upstream, covering `HS256`, `RS256`, and `ES256`. Other JOSE algorithms (`RS384`/`RS512`, `PS*`, `EdDSA`, ECDSA curves other than P-256) are not supported.
- JWKS support is direct-URL only; there is no discovery-document-based JWKS resolution, and the JWKS cache is process-local rather than shared across instances.
- A JWKS source removed or repointed by a config reload is not evicted from the in-memory key cache for the remaining process lifetime.
- When both static asymmetric keys and a `jwks_url` are configured and a token matches a key in each, the request is rejected as ambiguous rather than resolved by precedence.
- Request-path RBAC is limited to scope/role checks against JWT claims; there is no generic policy engine.
- Admin-plane RBAC is a fixed three-tier model (`viewer`, `operator`, `admin`) with per-route minimums; custom roles and per-route policy expressions are not supported.
- Control API mTLS has no CRL or OCSP revocation checking — a compromised client certificate remains valid until its CA material is rotated.
- When a bearer token and an mTLS identity are presented together, their roles are **unioned**, not intersected: a `viewer` token with an `admin` certificate is treated as `admin`. Certificates cannot be used to constrain a token.
- An unrecognized `auth.identity_source.kind` is ignored and silently falls back to the default rather than failing config validation.
- The admin audit stream is per-process and local; there is no fleet-wide aggregation, delivery guarantee, or tamper-evidence.
- `ip_allowlist.trust_proxy_headers` is accepted in config but not honored — the source address is always the TCP peer, and proxy headers are never trusted.
- External auth (HTTP subrequest and OIDC) is implemented as a non-blocking async check per upstream, with configurable fail-open/fail-closed behavior; there is no interactive login or session-cookie flow.
- OIDC external auth covers discovery and token introspection only, and the discovery document is refetched on every request rather than cached. Local signature validation against an issuer's keys is available through JWT auth with `jwks_url`, not through the OIDC provider.
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
- the upstream environment speaks HTTP/2 (`https://` backends) or HTTP/1.1 (`http://` backends), or a mix of both
- the deployment can tolerate occasional restarts for startup-owned config changes
- the deployment does not require rich traffic policy or auth gateway features

Spooky is a poor fit today when:

- every config field must be live-reloadable with no restart boundary
- upstream protocol breadth is required
- advanced API gateway behavior is required
- a rich dynamic control plane is expected
- a plugin/filter ecosystem is required

## Related Pages

- [Feature Matrix](feature-matrix.md)
- [Production Readiness](../operations/production-readiness.md)
- [Roadmap](../roadmap.md)
