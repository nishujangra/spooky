# How to Configure Spooky as a Reverse Proxy

This guide walks through building a working `config.yaml` for Spooky as a production reverse proxy, explaining every section and its trade-offs.

Use [Configuration Reference](../configuration/reference.md) for the canonical field-by-field schema and [Configuration Examples](../configuration/examples.md) for complete deployment templates.

---

## Config File Location

Spooky loads config from the path given to `--config`:

```bash
spooky --config /etc/spooky/config.yaml
```

If `--config` is not provided, it falls back to `/etc/spooky/config.yaml`. Spooky exits at startup if the file is missing, unreadable, or fails validation.

---

## Minimal Working Config

The absolute minimum to get Spooky running as a reverse proxy:

```yaml
version: 1

listen:
  tls:
    cert: /etc/spooky/certs/fullchain.pem
    key:  /etc/spooky/certs/privkey.pem

upstream:
  default:
    route:
      path_prefix: "/"
    backends:
      - id: "backend1"
        address: "127.0.0.1:8080"
```

This listens on `0.0.0.0:9889`, forwards all traffic to `127.0.0.1:8080` over HTTPS (cleartext backend needs `http://` prefix — see below), and uses all defaults.

---

## Section-by-Section Guide

### version

Always set to `1`. Future schema changes bump this number.

```yaml
version: 1
```

---

### listen

Defines where Spooky accepts client connections.

```yaml
listen:
  protocol: http3      # only supported value
  address: "0.0.0.0"  # bind all interfaces; use "127.0.0.1" for localhost-only
  port: 443            # use 9889 for unprivileged; 443 requires root or CAP_NET_BIND_SERVICE
  tls:
    cert: /etc/spooky/certs/fullchain.pem
    key:  /etc/spooky/certs/privkey.pem
```

Spooky also automatically starts a **bootstrap TLS listener** on the same address/port for HTTP/1.1 and HTTP/2 clients. This is how browsers connect before they learn about HTTP/3 via the `Alt-Svc` header. You do not configure it separately — it shares the same cert/key.

Protocol boundary:
- native ingress is HTTP/3 only
- HTTP/3 `Upgrade` / `Connection: upgrade` requests are rejected explicitly
- bootstrap HTTP/1.1 may proxy WebSocket upgrades
- native H3 does not currently support WebSocket-style upgrade semantics

**Binding port 443:**
```bash
# Option A: run as root (drops privileges after binding — see security section)
sudo spooky --config /etc/spooky/config.yaml

# Option B: grant CAP_NET_BIND_SERVICE
sudo setcap cap_net_bind_service=+ep /usr/bin/spooky
spooky --config /etc/spooky/config.yaml
```

**mTLS (client certificates):**
```yaml
listen:
  tls:
    cert: /etc/spooky/certs/fullchain.pem
    key:  /etc/spooky/certs/privkey.pem
    client_auth:
      enabled: true
      require_client_cert: true   # reject connections without a client cert
      ca_file: /etc/spooky/certs/client-ca.pem
```

---

### listeners (multi-listener)

Use `listeners` instead of `listen` when you need multiple independent ports:

```yaml
listeners:
  - protocol: http3
    address: "0.0.0.0"
    port: 443
    tls:
      cert: /etc/spooky/certs/public-fullchain.pem
      key:  /etc/spooky/certs/public-privkey.pem
  - protocol: http3
    address: "10.0.0.1"
    port: 8443
    tls:
      cert: /etc/spooky/certs/internal-fullchain.pem
      key:  /etc/spooky/certs/internal-privkey.pem
```

When `listeners` is set, the top-level `listen` block is ignored. Each listener gets its own worker group and bootstrap TLS listener. All listeners share the same upstream routing table.

---

### upstream_tls

Controls how Spooky verifies backends' TLS certificates. Defaults are safe — keep them unless your backends use a private CA.

```yaml
upstream_tls:
  verify_certificates: true   # always verify backend TLS (default)
  strict_sni: true            # send backend hostname as SNI (default)
  ca_file: null               # set if backends use a private CA
  ca_dir: null                # directory of PEM CA bundles (alternative to ca_file)
```

Semantics:
- Hostname backends are verified against the configured backend hostname.
- IP-literal backends are verified against the configured IP identity.
- `strict_sni: false` disables only the SNI extension; certificate verification still stays enabled unless `verify_certificates: false`.
- `verify_certificates: false` disables upstream certificate validation entirely and should only be used in trusted environments.

To trust a private CA:
```yaml
upstream_tls:
  verify_certificates: true
  strict_sni: true
  ca_file: /etc/spooky/certs/internal-ca.pem
```

For cleartext HTTP backends, use `http://` in the backend address instead of disabling verification:
```yaml
backends:
  - id: "backend1"
    address: "http://127.0.0.1:8080"   # explicit cleartext opt-out
```

---

### upstream

The core of the config. Each key is a named upstream pool.

```yaml
upstream:
  api_pool:                           # pool name (any string, no spaces)
    load_balancing:
      type: round-robin               # algorithm for this pool

    route:
      host: "api.example.com"         # optional: match Host header
      path_prefix: "/api"             # match requests starting with /api

    host_policy:
      mode: pass-through              # how to set Host on the upstream request

    forwarded_headers:
      mode: append                    # how to handle X-Forwarded-For

    backends:
      - id: "api-01"
        address: "10.0.1.10:8443"
        weight: 100
        health_check:
          path: "/health"
          interval: 5000
```

#### Route matching

Routes are matched by longest path prefix. Ties are broken by: host-specific > wildcard host > host-agnostic, then method-specific > any-method, then lexicographic upstream name. Ambiguous routes (same host + path + method) are rejected at startup.

```yaml
# Most specific — both host and path
route:
  host: "api.example.com"
  path_prefix: "/v2"

# Wildcard host — matches any subdomain
route:
  host: "*.example.com"
  path_prefix: "/api"

# Path only — matches any host
route:
  path_prefix: "/static"

# Catch-all — use "/" as last resort
route:
  path_prefix: "/"
```

#### Load balancing algorithms

| Type | When to use |
|------|-------------|
| `round-robin` | Equal backends, stateless requests |
| `random` | Simple equal distribution |
| `least-connections` | Backends with variable latency |
| `consistent-hash` | Sticky routing by header/cookie/query |
| `latency-aware` | Automatically shift traffic to faster backends |
| `sticky-cid` | Sticky routing by QUIC Connection ID |

For `consistent-hash` and `sticky-cid`, specify the key source:
```yaml
load_balancing:
  type: consistent-hash
  key: "header:x-user-id"    # or: cookie:session, query:user_id, path, authority
```

#### Backend address formats

```yaml
backends:
  - address: "10.0.1.10:8443"               # HTTPS (default, verified)
  - address: "https://10.0.1.10:8443"       # explicit HTTPS
  - address: "http://10.0.1.10:8080"        # cleartext HTTP (insecure, logs warning)
  - address: "backend.internal.example"     # HTTPS on port 443 (hostname only)
  - address: "[::1]:8443"                   # IPv6 with brackets
```

#### Backend weights

Weights are relative. A backend with weight `200` gets twice the traffic of one with weight `100`.

```yaml
backends:
  - id: "primary"
    address: "10.0.1.10:8443"
    weight: 200    # ~67% of traffic
  - id: "secondary"
    address: "10.0.1.11:8443"
    weight: 100    # ~33% of traffic
```

#### Health checks

Without a `health_check` block, the backend starts healthy and stays healthy (no active polling). Add one to detect failures:

```yaml
health_check:
  path: "/health"          # GET this path
  interval: 5000           # poll every 5 seconds
  timeout_ms: 1000         # fail after 1 second with no response
  failure_threshold: 3     # 3 consecutive failures → mark unhealthy
  success_threshold: 2     # 2 consecutive successes → mark healthy again
  cooldown_ms: 5000        # wait 5 seconds after marking unhealthy before re-polling
```

#### Host policy

Controls the `Host`/`:authority` header sent to the upstream backend.

```yaml
host_policy:
  mode: pass-through   # forward the client's Host as-is (default)

host_policy:
  mode: rewrite
  host: "internal-api.example.com"   # replace with this static value

host_policy:
  mode: upstream       # use the backend's own hostname
```

#### Forwarded headers

Controls `X-Forwarded-For` and related forwarding headers.

```yaml
forwarded_headers:
  mode: overwrite    # replace with client IP only (default — trust no inbound chain)

forwarded_headers:
  mode: append       # append client IP to existing chain (use behind another trusted proxy)

forwarded_headers:
  mode: preserve     # pass inbound chain unchanged (no client IP added)
```

#### Request validation and protocol semantics

- `:authority` and `Host` must match when both are present.
- `CONNECT` requires `host:port` authority and is denied unless explicitly allowed by policy.
- `HEAD` responses are headers-only downstream, even if the upstream emitted a body.
- HTTP/3 rejects `Upgrade`-style requests; use the bootstrap HTTP/1.1 path for WebSocket upgrades.

Example protocol policy:

```yaml
resilience:
  protocol:
    allow_0rtt: false
    early_data_safe_methods: ["GET", "HEAD"]
    enforce_authority_host_match: true
    allow_connect: true
    connect_allowed_ports: [443]
    connect_allowed_authorities:
      - "proxy.internal.example:443"
```

#### Per-upstream TLS override

Override the global `upstream_tls` for one upstream:

```yaml
api_pool:
  tls:
    verify_certificates: true
    strict_sni: true
    ca_file: /etc/spooky/certs/api-internal-ca.pem
```

---

### log

```yaml
log:
  level: info        # trace | debug | info | warn | error | off
  format: json       # json (structured, for log collectors) | plain (human-readable)
  file:
    enabled: false   # set true to write to a file instead of stderr
    path: /var/log/spooky/spooky.log
```

Use `json` format in production (parseable by Loki, Elasticsearch, Datadog). Use `plain` during development.

---

### performance

Key values to tune for production:

```yaml
performance:
  worker_threads: 4              # match CPU cores (or cores - 1)
  reuseport: true                # required when worker_threads > 1

  global_inflight_limit: 8192    # max concurrent requests across all upstreams
  per_backend_inflight_limit: 128

  backend_timeout_ms: 5000       # total upstream response timeout
  backend_connect_timeout_ms: 1000
  max_request_body_bytes: 10485760     # 10 MiB
  max_response_body_bytes: 104857600   # 100 MiB
```

**Timeout ordering constraint (validated at startup):**
```
backend_connect_timeout_ms
  <= backend_timeout_ms
  <= backend_body_idle_timeout_ms
  <= backend_body_total_timeout_ms
  <= backend_total_request_timeout_ms
```

---

### observability

```yaml
observability:
  metrics:
    enabled: true
    address: "127.0.0.1"   # expose only on loopback (safer)
    port: 9901
    path: "/metrics"       # Prometheus scrape endpoint

  control_api:
    enabled: true
    address: "127.0.0.1"
    port: 9902
    auth_token: "replace-with-strong-token"   # required when enabled
    health_path: "/health"
    ready_path:  "/ready"
    runtime_path: "/admin/runtime"
    restart_path: "/admin/runtime/restart"
    reload_certs_path: "/admin/runtime/reload-certs"
```

Check readiness:
```bash
curl -k https://127.0.0.1:9902/ready
curl -k https://127.0.0.1:9902/health
```

Reload listener certificates without restarting the process:
```bash
curl -X POST \
  -H 'Authorization: Bearer replace-with-strong-token' \
  https://127.0.0.1:9902/admin/runtime/reload-certs
```

---

### security

When Spooky starts as root (for port 443), it drops privileges after binding:

```yaml
security:
  privileges:
    enabled: true
    user: "spooky"    # drop to this user after binding
    group: "spooky"   # drop to this group after binding
```

Create the system user:
```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin spooky
sudo chown -R spooky:spooky /etc/spooky /var/log/spooky
```

---

## Validating Your Config

Spooky runs full validation on startup and exits with a clear error message. To validate without starting:

```bash
spooky --config /etc/spooky/config.yaml --validate
```

Common validation errors and fixes:

| Error | Fix |
|-------|-----|
| `listen.tls requires either cert/key or certificates entries` | Add cert and key paths |
| `Ambiguous route matcher detected` | Two upstreams have same host+path+method — make them distinct |
| `backend_connect_timeout_ms must be <= backend_timeout_ms` | Fix timeout ordering |
| `worker_threads > 1 requires reuseport=true` | Add `reuseport: true` |
| `Cannot open listen.tls.cert` | Wrong path or file missing |
| `Upstream has no backends` | Add at least one backend per upstream |
