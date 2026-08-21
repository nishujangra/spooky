# Configuration Examples

This page collects complete deployment-oriented examples. Use it together with the [Configuration Reference](reference.md), which remains the canonical schema and semantics document.

For distributed quota examples and migration guidance, see
[Distributed Quota](../operations/distributed-quota.md).

## How To Use These Examples

Use these examples as starting points, not as copy-paste truth for every environment.

- start with the smallest example that matches your deployment shape
- change addresses, certificates, and admin credentials first
- validate operational implications before promoting a local example into production

## Example Selection Guide

| If you need... | Start with... |
| --- | --- |
| a local first run | Example 1 |
| one upstream in production | Example 2 |
| multiple upstreams with different routing | Example 3 |
| multiple listeners with different bind identities | Example 4 |
| downstream client certificate auth | Example 5 |
| a private CA for upstream trust | Example 6 |
| static asymmetric JWT verification | Example 7 |
| remote JWKS validation | Example 8 |

## Example 1: Minimal Local Development

```yaml
version: 1

listen:
  address: "127.0.0.1"
  port: 9889
  tls:
    cert: certs/proxy-fullchain.pem
    key: certs/proxy-key-pkcs8.pem

upstream:
  default:
    route:
      path_prefix: "/"
    backends:
      - id: "backend1"
        address: "http://127.0.0.1:8080"

upstream_tls:
  verify_certificates: false
  strict_sni: false
```

Use this shape for local iteration only. It opts into cleartext upstream traffic explicitly with `http://`.

Common mistake:

- copying this example into production without restoring upstream TLS verification and stronger admin-surface protection

## Example 2: Single-Upstream Production

```yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 443
  tls:
    cert: /etc/spooky/certs/fullchain.pem
    key: /etc/spooky/certs/privkey.pem

upstream_tls:
  verify_certificates: true
  strict_sni: true

upstream:
  default:
    load_balancing:
      type: round-robin
    route:
      path_prefix: "/"
    backends:
      - id: "app-1"
        address: "app.internal.example:8443"
        weight: 100
        health_check:
          path: "/health"
          interval: 5000
          timeout_ms: 1000
          failure_threshold: 3
          success_threshold: 2
          cooldown_ms: 5000

security:
  privileges:
    enabled: true
    user: "spooky"
    group: "spooky"

observability:
  metrics:
    enabled: true
    address: "127.0.0.1"
    port: 9901
    path: "/metrics"
  control_api:
    enabled: true
    address: "127.0.0.1"
    port: 9902
    auth_token: "replace-with-strong-token"
```

Use this when:

- one upstream handles most traffic
- you want the simplest host deployment that is still production-oriented

## Example 3: Multi-Upstream Production

```yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 443
  tls:
    cert: /etc/spooky/certs/fullchain.pem
    key: /etc/spooky/certs/privkey.pem

upstream_tls:
  verify_certificates: true
  strict_sni: true

upstream:
  api:
    load_balancing:
      type: consistent-hash
      key: "header:x-user-id"
    route:
      host: "api.example.com"
      path_prefix: "/"
    backends:
      - id: "api-1"
        address: "api-a.internal.example:8443"
        weight: 100
      - id: "api-2"
        address: "api-b.internal.example:8443"
        weight: 100

  web:
    load_balancing:
      type: latency-aware
    route:
      host: "www.example.com"
      path_prefix: "/"
    backends:
      - id: "web-1"
        address: "web-a.internal.example:8443"
        weight: 100
      - id: "web-2"
        address: "web-b.internal.example:8443"
        weight: 100

load_balancing:
  type: round-robin
```

Use this when:

- different hosts or paths must route to different upstreams
- the application needs different load-balancing strategies per upstream

## Example 4: Multi-Listener Deployment

```yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 443
  tls:
    cert: /etc/spooky/certs/public-fullchain.pem
    key: /etc/spooky/certs/public-privkey.pem

listeners:
  - protocol: http3
    address: "0.0.0.0"
    port: 443
    tls:
      cert: /etc/spooky/certs/public-fullchain.pem
      key: /etc/spooky/certs/public-privkey.pem
  - protocol: http3
    address: "10.0.0.10"
    port: 8443
    tls:
      cert: /etc/spooky/certs/internal-fullchain.pem
      key: /etc/spooky/certs/internal-privkey.pem

upstream:
  default:
    route:
      path_prefix: "/"
    backends:
      - id: "backend1"
        address: "backend.internal.example:8443"
```

The top-level `listen` field is always required by the schema. When `listeners[]` is non-empty, runtime normalization uses `listeners[]` and the top-level `listen` block is superseded.

Common mistake:

- expecting the top-level `listen` block to stay active alongside `listeners[]`

## Example 5: Bootstrap Listener Client Auth

```yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 443
  tls:
    cert: /etc/spooky/certs/fullchain.pem
    key: /etc/spooky/certs/privkey.pem
    client_auth:
      enabled: true
      require_client_cert: true
      ca_file: /etc/spooky/certs/client-ca.pem

upstream:
  default:
    route:
      path_prefix: "/"
    backends:
      - id: "backend1"
        address: "backend.internal.example:8443"
```

This is the right shape when bootstrap TLS clients must authenticate with certificates.

## Example 6: Private CA Upstream Trust

```yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 443
  tls:
    cert: /etc/spooky/certs/fullchain.pem
    key: /etc/spooky/certs/privkey.pem

upstream_tls:
  verify_certificates: true
  strict_sni: true
  ca_file: /etc/spooky/certs/private-root-ca.pem

upstream:
  default:
    route:
      path_prefix: "/"
    backends:
      - id: "backend1"
        address: "backend.private.example:8443"
```

Use this when:

- the upstream certificate chain is not rooted in the public Web PKI
- one deployment needs stricter trust control than public default CA bundles

## Example 7: Static RS256 And ES256 JWT Keys

Pin verification to public keys you manage yourself. `secret` stays empty because
`HS256` is not in the allowlist — configuring both is rejected at startup.

```yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 443
  tls:
    cert: /etc/spooky/certs/fullchain.pem
    key: /etc/spooky/certs/privkey.pem

upstream:
  api_pool:
    route:
      path_prefix: "/"
    auth:
      jwt:
        secret: ""
        issuer: "https://issuer.example.com/"
        audience: "payments-api"
        allowed_algorithms: ["RS256", "ES256"]
        require_kid: true
        static_keys:
          - kind: pem
            kid: "rsa-2026-01"
            alg: "RS256"
            public_key_pem: |
              -----BEGIN PUBLIC KEY-----
              MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A...
              -----END PUBLIC KEY-----
          - kind: pem
            kid: "ec-2026-01"
            alg: "ES256"
            public_key_pem: |
              -----BEGIN PUBLIC KEY-----
              MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE...
              -----END PUBLIC KEY-----
        clock_skew_secs: 30
    backends:
      - id: "backend1"
        address: "backend.internal.example:8443"
```

Keys may also be supplied as JWK documents with `kind: jwk` and a `jwk` string field
instead of `public_key_pem`. RSA keys shorter than 2048 bits are rejected.

## Example 8: JWKS-Backed Validation With Strict Policy

Fetch signing keys from the issuer and enforce a strict issuer/audience/algorithm
policy. Multiple issuers and audiences use the plural fields; the singular
`issuer`/`audience` fields remain supported but cannot be combined with them.

```yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 443
  tls:
    cert: /etc/spooky/certs/fullchain.pem
    key: /etc/spooky/certs/privkey.pem

upstream:
  api_pool:
    route:
      path_prefix: "/"
    auth:
      jwt:
        secret: ""
        issuers:
          - "https://issuer.example.com/"
          - "https://issuer-eu.example.com/"
        audiences:
          - "payments-api"
          - "payments-api-internal"
        allowed_algorithms: ["RS256", "ES256"]
        require_kid: true
        jwks_url: "https://issuer.example.com/.well-known/jwks.json"
        jwks_refresh_interval_secs: 300
        jwks_request_timeout_ms: 2000
        jwks_cache_ttl_secs: 900
        jwks_stale_if_error_secs: 3600
        jwks_startup_behavior: require_ready
        clock_skew_secs: 30
      required_scopes:
        - "payments:read"
    backends:
      - id: "backend1"
        address: "backend.internal.example:8443"
```

`jwks_url` must be an absolute `https` URL. Omitting `issuers`/`audiences` entirely
disables those checks — signature and expiry are still enforced, but any issuer's
token signed by a trusted key is accepted, so set them in production.

### Startup Behavior Choices

`jwks_startup_behavior` decides what happens when the initial fetch fails:

| Value | Behavior |
| --- | --- |
| `require_ready` (default) | Startup fails with an error naming the endpoint and cache state. Use when the upstream must never serve traffic with unverifiable tokens. |
| `allow_degraded` | The process boots and retries in the background. JWT requests are rejected until keys load. Use when availability of other upstreams matters more than this one being immediately ready. |

Both reject tokens while keys are missing; they differ only in whether the process
starts at all.

## Example 9: Runtime Activation And Reload Posture

Impulse supports generation-based validation, preview, activation, rollback, and certificate-only reload. When planning operations:

- use `POST /admin/runtime/validate` to check a candidate configuration
- use `POST /admin/runtime/preview` to see the staged diff without touching the running runtime
- use `POST /admin/runtime/activate` to commit a runtime-managed config change
- use `POST /admin/runtime/rollback` to return to a retained runtime generation
- use `POST /admin/runtime/reload-certs` for listener certificate replacement on new handshakes
- use the legacy `POST /admin/runtime/reload` shortcut only when you intentionally want the older direct-apply behavior without preview
- plan a drain-and-restart workflow only for log format/file settings, tracing config, control-plane thread counts, and listener removal or bind-address changes

## Related Pages

- [Configuration Reference](reference.md)
- [TLS Setup](tls.md)
- [Production Readiness](../operations/production-readiness.md)
