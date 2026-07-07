# Security Model

This page describes the current trust boundaries and security assumptions in the project as it exists today.

## Security Goals

Spooky is designed to:

- terminate downstream TLS for HTTP/3 and bootstrap TLS traffic
- validate and forward requests to configured upstreams with explicit trust settings
- bound resource consumption under malformed, slow, or overloaded traffic
- expose a small operator control surface with authentication

Spooky is not yet designed to be:

- a full web application firewall
- a complete authentication gateway
- a general-purpose policy engine

## Trust Boundaries

### Downstream Client To Spooky

Clients are untrusted. Spooky must:

- parse QUIC and HTTP/3 safely
- validate headers and pseudo-headers strictly
- bound header count and total header bytes
- enforce request-body limits and timeouts
- reject unsupported upgrade-style semantics
- avoid unbounded state growth from malformed packets or connection churn

### Spooky To Upstream Backends

Upstreams are trusted only according to explicit configuration.

- HTTPS upstreams are verified by default.
- SNI is sent by default in strict mode.
- Private trust roots can be configured with `ca_file` and `ca_dir`.
- Disabling upstream certificate verification is allowed, but should be treated as a break-glass mode rather than a normal production stance.

### Operator To Control Plane

The control API is privileged.

- It can expose runtime state.
- It can trigger restart behavior.
- It can trigger certificate reload.
- It must be treated as an admin surface, not a public endpoint.

## Downstream TLS Model

Spooky supports:

- default/fallback certificate identity
- SNI-specific certificates
- bootstrap listener client-auth with optional or required certificate modes

Important scope note:

- current client-auth coverage is centered on the bootstrap TLS listener path
- operators should verify whether their exact ingress shape requires stronger mTLS guarantees on every downstream path before broad rollout

## Upstream TLS Model

Upstream trust behavior is controlled by configuration.

Safe posture:

- `verify_certificates: true`
- `strict_sni: true`
- explicit custom CA material when using private PKI

Unsafe posture:

- `verify_certificates: false`
- public or shared-network upstreams with disabled verification

## Resource-Exhaustion Defense Model

Spooky includes multiple defensive layers intended to limit blast radius from abusive or unhealthy traffic:

- new-connection token bucket
- maximum active connection caps
- per-connection stream caps
- global and scoped inflight limits
- route queue caps
- request and response body caps
- body idle and total timeouts
- adaptive admission and brownout controls

These features are part of the project’s security posture because they reduce denial-of-service amplification inside the process.

## Request Authentication And Authorization Model

Spooky supports per-upstream request authentication, checked in this order:

- **API key**: a configured header is compared against a static key list. Local, synchronous, no network call.
- **JWT**: local HS256 signature and claim validation (issuer, audience, clock skew), plus optional scope/role checks against token claims. Local, synchronous, no network call.
- **External auth**: an async HTTP subrequest (generic HTTP or OIDC-shaped) sent to a configured auth endpoint, gated before upstream admission so the request never reaches the backend while auth is pending. Only one external auth provider is supported per upstream, and it cannot be combined with API key or JWT in the current version.

External auth details:

- The auth call runs on a dedicated HTTP client, isolated from upstream backend transport, inflight accounting, and health state — an auth outage cannot degrade backend routing.
- A decision maps to `Allow`, `Deny`, `Redirect`, or `Challenge`; only headers on an explicit allowlist are copied from the auth server's response into the response sent to the client.
- Failure mode (fail-open or fail-closed) is configured per provider. The default is fail-closed: a timeout or transport error denies the request rather than silently admitting it.
- OIDC mode uses discovery and token introspection to validate bearer tokens. It does not fetch or validate against JWKS, does not cache the discovery document (refetched per request), and does not implement interactive login or session-cookie flows.

## What Spooky Does Not Currently Provide

Spooky does not currently provide first-class:

- JWKS-based JWT validation or key rotation
- OIDC login flows (interactive/browser SSO) or session-cookie handling
- a generic RBAC/policy engine beyond scope/role checks on JWT claims
- WAF behavior
- deep content inspection
- extensible third-party auth/policy modules

## Recommended Deployment Security Posture

- keep the control API bound to loopback or a strongly isolated admin network
- use a strong control API token and rotate it as an administrative secret
- keep upstream certificate verification enabled in production
- run with least privilege after bind
- restrict filesystem write access to the minimum required paths
- monitor handshake failures, overload events, and unexpected restart activity

## Future Security Hardening Priorities

- deeper parser fuzzing
- stronger control-plane auditability
- broader documentation of mTLS behavior across all ingress paths
- explicit support boundaries for admin-plane deployment patterns
- stronger auth/policy features where the product direction requires them

## Related Pages

- [Production Readiness](../operations/production-readiness.md)
- [Limitations](../reference/limitations.md)
- [TLS Setup](../configuration/tls.md)
