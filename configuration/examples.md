# Configuration Examples

This page collects complete deployment-oriented examples. Use it together with the [Configuration Reference](reference.md), which remains the canonical schema and semantics document.

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

## Example 7: Current Reload Stance

Spooky supports **full configuration hot reload** via `POST /admin/runtime/reload`, alongside
certificate-only reload for new handshakes. When planning operations:

- use full config reload (`/admin/runtime/reload`) for route, upstream, backend, timeout, limit,
  resilience-policy, and `log.level` changes — these apply live via an atomic runtime swap, no restart
- use cert reload (`/admin/runtime/reload-certs`) for listener certificate replacement
- plan a drain-and-restart workflow only for log format/file settings, tracing config, control-plane
  thread counts, and listener removal or bind-address changes, which the reload endpoint rejects
- keep rollback and staged rollout procedures ready

## Related Pages

- [Configuration Reference](reference.md)
- [TLS Setup](tls.md)
- [Production Readiness](../operations/production-readiness.md)
