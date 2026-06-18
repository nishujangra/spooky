# Configuration Reference

This is the canonical configuration document for Spooky. It should answer four questions for every setting:

- what field exists
- what values it accepts
- what its default is
- what runtime behavior it changes

Use [Configuration Examples](examples.md) for complete deployment patterns. Use this page when you need exact schema and semantics.

## Scope Of This Reference

This page covers:

- schema shape
- default values
- precedence and normalization rules
- validation behavior
- runtime meaning of major knobs

This page does not change the current product limits:

- full config hot reload is not implemented
- certificate reload covers new handshakes only
- upstream forwarding is centered on HTTP/2

## Reading This Reference

- Start with [Configuration Examples](examples.md) if you need a working template.
- Read [TLS Setup](tls.md) before configuring production certificates or private trust roots.
- Read [Production Readiness](../operations/production-readiness.md) if you are deciding whether the current operational model fits your rollout requirements.

## Configuration File Format

Spooky uses YAML configuration loaded with:

```bash
spooky --config /path/to/config.yaml
```

If `--config` is omitted, Spooky attempts `/etc/spooky/config.yaml`.

## Canonical Top-Level Shape

```yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 9889
  tls:
    cert: "/etc/spooky/certs/fullchain.pem"
    key: "/etc/spooky/certs/privkey.pem"

upstream_tls:
  verify_certificates: true
  strict_sni: true

upstream:
  default:
    route:
      path_prefix: "/"
    backends:
      - id: "backend1"
        address: "backend.internal.example:8443"
        weight: 100

log:
  level: info
  format: plain
```

## Top-Level Keys At A Glance

| Key | Required | Meaning |
| --- | --- | --- |
| `version` | No | Schema version; defaults to `1` |
| `listen` | Yes | Single-listener definition |
| `listeners` | No | Multi-listener override for the top-level `listen` block |
| `upstream_tls` | No | Global TLS policy for HTTPS backends |
| `upstream` | Yes | Named route and backend pools |
| `load_balancing` | No | Global fallback load-balancing policy |
| `log` | No | Logging policy |
| `performance` | No | Timeouts, limits, worker model, and buffer sizing |
| `resilience` | No | Admission, queueing, circuit breaker, retry, brownout, and protocol policy |
| `observability` | No | Metrics, control API, tracing, and related surfaces |
| `security` | No | Privilege-drop behavior |

## Runtime Normalization And Precedence

Spooky normalizes configuration into a single runtime model before it serves traffic.

Precedence and interpretation rules:

1. If `listeners[]` is non-empty, it is the only effective listener set.
2. The top-level `listen` block is used only when `listeners[]` is absent or empty.
3. Per-upstream TLS settings override global `upstream_tls`.
4. Per-upstream load-balancing settings override the top-level `load_balancing` fallback.
5. Certificate reload updates listener TLS material for future handshakes; it does not rewrite the already-running route or upstream model.

## Production-Safe Defaults

The configuration model is intentionally safe-by-default in several important areas:

- native ingress defaults to HTTP/3
- HTTPS upstreams verify certificates by default
- upstream SNI is enabled by default
- bootstrap listener TLS is always tied to configured listener identity
- request and response paths are bounded by explicit timeout and size controls

Treat the following settings as high-risk when changed:

- `upstream_tls.verify_certificates: false`
- broad increases to inflight or body-size limits without capacity validation
- enabling public exposure of the control API
- route or listener changes that rely on restart without a drain-and-rollback plan

## Complete Example Configurations

For complete examples, use [Configuration Examples](examples.md).

## Top-Level Configuration

## Top-Level Configuration

### version

Configuration schema version.

- Current version: `1`
- Supported versions: `1`
- Backward-compatibility policy: unsupported versions are rejected at load time, and version-specific migration hooks are used when introducing future schema versions.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `version` | integer | No | 1 | Configuration schema version |

### listen

Server listening configuration. Defines the protocol, address, and port for incoming client connections. Used as the single listener when `listeners` is absent or empty.

### listeners

Optional multi-listener array. When set, overrides the top-level `listen` block. Each entry is an independent listener with its own address, port, and TLS identity. Spooky spawns a separate QUIC worker group and bootstrap TLS listener per entry.

### Runtime Normalization And Precedence

Spooky normalizes configuration into one canonical runtime model before any listener starts.

Precedence rules:

1. `listeners[]` is the only effective listener set when it is non-empty.
2. The top-level `listen` block is only used when `listeners[]` is empty.
3. Listener TLS fallback order is:
   1. exact SNI match in `listen.tls.certificates`
   2. legacy `listen.tls.cert` + `listen.tls.key` when configured
   3. otherwise the first `listen.tls.certificates[]` entry becomes the default identity
4. Upstream TLS precedence is:
   1. `upstream.<name>.tls`
   2. global `upstream_tls`
5. Listener certificate reload updates listener TLS material for new handshakes through `observability.control_api.reload_certs_path` without restarting the process. Existing QUIC connections and existing bootstrap TLS sessions keep the certificate and client-auth state that they already negotiated.

Startup rejects ambiguous or contradictory combinations, including duplicate effective listener binds, duplicate normalized route matchers, partial legacy listener cert/key pairs, invalid or duplicate SNI `server_name` entries, `host_policy.host` outside `mode: rewrite`, and `CONNECT` routing/policy conflicts.

### upstream

Named upstream pool definitions. Each key represents a unique upstream pool with its own routing rules, load balancing strategy, and backend servers.

### load_balancing

Optional global fallback for upstream load balancing. If an upstream omits `upstream.<name>.load_balancing`, the top-level `load_balancing` value is applied to that upstream during config load.

### log

Logging configuration. Controls log level and output formatting.

## Default Values

The following table lists all default configuration values used when properties are not explicitly specified:

| Property | Default Value | Description |
|----------|---------------|-------------|
| `version` | `1` | Configuration format version |
| `listen.protocol` | `"http3"` | Native ingress protocol (HTTP/3 over QUIC); TLS bootstrap ingress for HTTP/1.1/2 compatibility is also active |
| `listen.port` | `9889` | Listening port |
| `listen.address` | `"0.0.0.0"` | Listening address |
| `listen.tls.cert` | optional | Legacy/default TLS certificate file path |
| `listen.tls.key` | optional | Legacy/default TLS private key file path |
| `listen.tls.certificates` | `[]` | Optional SNI certificate mappings (`server_name` + `cert` + `key`) |
| `upstream[].route.path_prefix` | none | Path prefix for routing (set explicitly; use `/` for catch-all) |
| `upstream[].backends[].weight` | `100` | Backend weight for load balancing |
| `upstream[].backends[].health_check.path` | `"/health"` | Health check endpoint |
| `upstream[].backends[].health_check.interval` | `5000` | Health check interval (ms) |
| `upstream[].backends[].health_check.timeout_ms` | `1000` | Health check timeout (ms) |
| `upstream[].backends[].health_check.failure_threshold` | `3` | Failures to mark unhealthy |
| `upstream[].backends[].health_check.success_threshold` | `2` | Successes to mark healthy |
| `upstream[].backends[].health_check.cooldown_ms` | `5000` | Cooldown after failure (ms) |
| `upstream[].load_balancing.type` | `"round-robin"` | Per-upstream load balancing algorithm |
| `log.level` | `"info"` | Logging verbosity level |
| `log.format` | `"plain"` | Log output format (`plain` or `json`) |
| `log.file.enabled` | `false` | Write logs to file instead of stderr |
| `log.file.path` | `"/var/log/spooky/spooky.log"` | Log file path (used when `log.file.enabled` is true) |
| `performance.new_connections_per_sec` | `2000` | Token-bucket refill rate for new QUIC connections (conns/sec) |
| `performance.new_connections_burst` | `500` | Burst capacity for new QUIC connections |
| `performance.max_active_connections` | `20000` | Hard cap on concurrently tracked active QUIC connections per worker |
| `performance.backend_dns_refresh_enabled` | `false` | Enable periodic control-plane DNS refresh for hostname-based upstream backends |
| `performance.backend_dns_refresh_interval_ms` | `30000` | Refresh interval for hostname-based backend DNS records |
| `performance.quic_max_idle_timeout_ms` | `5000` | QUIC idle timeout — connection closed after this many ms of inactivity |
| `performance.quic_initial_max_data` | `10000000` | Connection-level flow control window (bytes) |
| `performance.quic_initial_max_stream_data` | `1000000` | Per-stream flow control window (bytes) |
| `performance.quic_initial_max_streams_bidi` | `100` | Max concurrent bidirectional streams per connection |
| `performance.quic_initial_max_streams_uni` | `100` | Max concurrent unidirectional streams per connection |
| `performance.max_response_body_bytes` | `104857600` | Hard cap on upstream response body bytes per stream (100 MiB); streams exceeding this return 503 (`upstream response body too large`) |

## Listen Configuration

Configures the listening interface for incoming client connections. HTTP/3 requires TLS configuration.

### Properties

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `protocol` | string | No | `http3` | Native ingress protocol for the data plane (HTTP/3 over QUIC) |
| `address` | string | No | `0.0.0.0` | IP address to bind to |
| `port` | integer | No | `9889` | Port to bind to |
| `tls` | object | Yes | - | TLS configuration (required for HTTP/3) |

### Protocol Values

- `http3`: HTTP/3 over QUIC (recommended)

Spooky also exposes a TLS bootstrap ingress for HTTP/1.1 and HTTP/2 clients. This compatibility path is primarily used for browser interoperability and advertising `Alt-Svc` so clients can upgrade to HTTP/3. Backend selection on the bootstrap path uses the same route-resolution, load-balancing strategy, and health-aware eligibility rules as the native QUIC ingress.

### TLS Configuration

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `cert` | string | Conditionally | Legacy/default TLS certificate path. Required with `key` when no `certificates` entries are configured |
| `key` | string | Conditionally | Legacy/default TLS private key path. Required with `cert` when no `certificates` entries are configured |
| `certificates` | array | No | SNI certificate entries |
| `certificates[].server_name` | string | Yes | Exact SNI hostname (DNS name) to match |
| `certificates[].cert` | string | Yes | Certificate path for that SNI hostname |
| `certificates[].key` | string | Yes | Private key path for that SNI hostname |

Certificate selection order:

1. Exact SNI match in `listen.tls.certificates`.
2. Fallback to `listen.tls.cert`/`listen.tls.key` when configured.
3. If legacy pair is not configured, fallback to the first entry in `listen.tls.certificates`.

Operational notes:

- If SNI is missing or unmatched, Spooky serves the default identity rather than rejecting the handshake.
- `listen.tls.certificates[].server_name` must be covered by the mapped certificate SANs or startup fails.
- Spooky exports downstream certificate expiry gauges:
  - `spooky_downstream_tls_certificate_not_after_seconds`
  - `spooky_downstream_tls_certificate_days_remaining`
- Certificate reload affects new QUIC and bootstrap TLS handshakes only. Existing connections continue with the TLS session they already negotiated.
- Downstream TLS metrics also include:
  - `spooky_downstream_tls_handshake_failure_total{listener,reason}`
  - `spooky_downstream_tls_certificate_selection_total{listener,selection}`
  - `spooky_downstream_tls_alpn_total{listener,protocol}`
- Important `reason` labels are:
  - `missing_client_cert`
  - `invalid_client_cert`
  - `expired_client_cert`
  - `unknown_issuer`
  - `alpn`
  - `handshake`

### Examples

```yaml
# Standard HTTP/3 configuration
listen:
  protocol: http3
  address: "0.0.0.0"
  port: 9889
  tls:
    cert: "/etc/spooky/certs/server.crt"
    key: "/etc/spooky/certs/server.key"

# Localhost-only development
listen:
  protocol: http3
  address: "127.0.0.1"
  port: 9889
  tls:
    cert: "certs/localhost.crt"
    key: "certs/localhost.key"

# Multi-domain SNI certificates with legacy fallback
listen:
  protocol: http3
  address: "0.0.0.0"
  port: 9889
  tls:
    cert: "/etc/spooky/certs/default.crt"
    key: "/etc/spooky/certs/default.key"
    certificates:
      - server_name: "api.example.com"
        cert: "/etc/spooky/certs/api.crt"
        key: "/etc/spooky/certs/api.key"
      - server_name: "www.example.com"
        cert: "/etc/spooky/certs/www.crt"
        key: "/etc/spooky/certs/www.key"
```

### Multi-Listener Configuration

Use `listeners` instead of `listen` when you need multiple independent listeners — for example, a public-facing port and a private/internal port with different TLS identities.

`listeners` and `listen` share the same per-entry schema. When `listeners` is set, the top-level `listen` block is ignored for runtime listener selection and listener validation.

```yaml
# Single listener — use the top-level listen block (default)
listen:
  protocol: http3
  address: "0.0.0.0"
  port: 9889
  tls:
    cert: "/etc/spooky/certs/fullchain.pem"
    key: "/etc/spooky/certs/privkey.pem"

# Multi-listener — independent public and internal listeners
listeners:
  - protocol: http3
    address: "0.0.0.0"
    port: 9889
    tls:
      cert: "/etc/spooky/certs/public-fullchain.pem"
      key: "/etc/spooky/certs/public-privkey.pem"
  - protocol: http3
    address: "10.0.0.1"
    port: 9890
    tls:
      cert: "/etc/spooky/certs/internal-fullchain.pem"
      key: "/etc/spooky/certs/internal-privkey.pem"
```

Each listener entry shares the same upstream routing table — route matching, load balancing, and health checks are global across all listeners.

## Upstream Configuration

Upstream pools define groups of backend servers with routing rules and load balancing strategies. Each upstream pool is identified by a unique name and contains routing criteria, load balancing configuration, and backend definitions.

### Structure

```yaml
upstream:
  pool_name:
    load_balancing: <LoadBalancing>
    route: <RouteMatch>
    backends: [<Backend>]
```

### Properties

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `load_balancing` | object | No | round-robin | Per-upstream load balancing algorithm configuration |
| `route` | object | Yes | - | Route matching criteria |
| `backends` | array | Yes | - | List of backend servers |
| `host_policy` | object | No | `pass-through` | Controls how the `Host`/`:authority` header is set on upstream requests |
| `tls` | object | No | inherits `upstream_tls` | Per-upstream TLS policy override (verify_certificates, strict_sni, ca_file, ca_dir); wins over global `upstream_tls` when set |
| `forwarded_headers` | object | No | `overwrite` | Controls `X-Forwarded-For` forwarding behavior |

### Route Matching

Route matching determines which upstream pool handles a request. Routes are evaluated by longest-prefix matching across all configured upstreams, selecting the route with the most specific (longest) path prefix.

#### RouteMatch Properties

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `host` | string | No | - | Host matcher. Supports exact hosts (`api.example.com`) and leading-wildcard suffix patterns (`*.example.com`) |
| `path_prefix` | string | No | - | Path prefix to match (e.g., `/api`) |
| `method` | string | No | - | HTTP method to match (case-insensitive, e.g. `GET`, `POST`) |

Route matching rules:

1. If `host` is specified:
   - Exact form: request Host must match exactly (case-insensitive after normalization)
   - Wildcard form: `*.example.com` matches subdomains like `api.example.com`, but not the bare apex `example.com`
2. If `path_prefix` is specified, the request path must start with the prefix
3. If both are specified, both conditions must match
4. Routes are evaluated by longest-prefix matching - the route with the most specific (longest) path prefix is selected
5. For equal-length prefixes, ties are deterministic:
   - host-specific routes win over host-agnostic routes
   - exact-host matches win over wildcard-host matches
   - among wildcard matches, longer suffixes win (`*.a.example.com` beats `*.example.com`)
   - method-specific routes win over method-agnostic routes
   - then lexicographically smaller upstream name wins

#### Route Examples

```yaml
# Host-based routing
upstream:
  api_pool:
    route:
      host: "api.example.com"
    backends: [...]

  web_pool:
    route:
      host: "www.example.com"
    backends: [...]

# Wildcard host routing
upstream:
  tenant_pool:
    route:
      host: "*.example.com"
      path_prefix: "/api"
    backends: [...]

# Path-based routing
upstream:
  api_pool:
    route:
      path_prefix: "/api"
    backends: [...]

  admin_pool:
    route:
      path_prefix: "/admin"
    backends: [...]

  default_pool:
    route:
      path_prefix: "/"
    backends: [...]

# Combined host and path routing
upstream:
  api_v2_pool:
    route:
      host: "api.example.com"
      path_prefix: "/v2"
    backends: [...]

  api_v1_pool:
    route:
      host: "api.example.com"
      path_prefix: "/v1"
    backends: [...]
```

### Backend Configuration

Each backend represents an upstream server that can handle requests.

#### Backend Properties

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `id` | string | Yes | - | Unique identifier for the backend |
| `address` | string | Yes | - | Backend server address. Accepted forms: `host:port`, `host` (defaults to `https://host:443`), `https://host[:port]`, `http://host[:port]` |
| `weight` | integer | No | `100` | Load balancing weight (higher values receive more traffic) |
| `health_check` | object | No | - | Health check configuration. Omit to disable active health polling — backend starts and stays healthy. |

**Address format notes:**
- `host:port` or `host` — shorthand, treated as `https://host:port` (port defaults to `443`)
- `https://host[:port]` — TLS upstream; port defaults to `443` if omitted
- `http://host[:port]` — cleartext upstream over h2c; port defaults to `80` if omitted. HTTP/1.1 upstream is not yet supported.

#### Health Check Configuration

Health checks monitor backend availability and automatically remove unhealthy backends from the pool.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `path` | string | No | `/health` | HTTP path for health check requests |
| `interval` | integer | No | `5000` | Health check interval in milliseconds |
| `timeout_ms` | integer | No | `1000` | Health check timeout in milliseconds |
| `failure_threshold` | integer | No | `3` | Consecutive failures before marking unhealthy |
| `success_threshold` | integer | No | `2` | Consecutive successes before marking healthy |
| `cooldown_ms` | integer | No | `5000` | Cooldown period after marking unhealthy (milliseconds) |

Health check behavior:

1. Health checks are performed at the specified `interval`
2. A backend is marked unhealthy after `failure_threshold` consecutive failures
3. An unhealthy backend enters cooldown for `cooldown_ms` milliseconds
4. After cooldown, health checks resume
5. A backend is marked healthy after `success_threshold` consecutive successes

#### Backend Examples

```yaml
# Minimal backend — no health check (backend stays permanently healthy)
backends:
  - id: "backend1"
    address: "https://example.com"

# Minimal backend with health check
backends:
  - id: "backend1"
    address: "10.0.1.10:8080"
    health_check:
      path: "/health"

# Weighted backend with custom health checks
backends:
  - id: "backend1"
    address: "10.0.1.10:8080"
    weight: 100
    health_check:
      path: "/api/health"
      interval: 10000
      timeout_ms: 2000
      failure_threshold: 5
      success_threshold: 3
      cooldown_ms: 10000

  - id: "backend2"
    address: "10.0.1.11:8080"
    weight: 200
    health_check:
      path: "/api/health"
      interval: 10000

# Multiple backends with different health endpoints
backends:
  - id: "primary"
    address: "10.0.1.10:8080"
    weight: 150
    health_check:
      path: "/status"
      interval: 5000

  - id: "secondary"
    address: "10.0.1.11:8080"
    weight: 100
    health_check:
      path: "/healthz"
      interval: 5000
```

### Host Policy

Controls how the `Host` / `:authority` header is set on requests forwarded to the upstream.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `mode` | string | No | `pass-through` | Header rewrite mode: `pass-through`, `rewrite`, or `upstream` |
| `host` | string | No | - | Static host to use when `mode: rewrite`; rejected for other modes |

#### Modes

| Mode | Behavior |
|------|----------|
| `pass-through` | Forwards the original client `Host`/`:authority` unchanged to the upstream |
| `rewrite` | Replaces the host with the value of `host` (required when using this mode) |
| `upstream` | Uses the backend's own authority (hostname from the `address` field) |

#### Examples

```yaml
upstream:
  # Pass client host through as-is (default)
  api_pool:
    host_policy:
      mode: pass-through
    backends: [...]

  # Rewrite to a static host
  legacy_pool:
    host_policy:
      mode: rewrite
      host: "legacy-origin.internal.example"
    backends: [...]

  # Use the backend's own hostname
  direct_pool:
    host_policy:
      mode: upstream
    backends: [...]
```

### Forwarded Headers Policy

Controls how `X-Forwarded-For` and related forwarding headers are set on upstream requests.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `mode` | string | No | `overwrite` | Forwarding mode: `append`, `preserve`, or `overwrite` |

#### Modes

| Mode | Behavior |
|------|----------|
| `overwrite` | Replaces any inbound `X-Forwarded-For` with the client IP only (default) |
| `append` | Appends the client IP to the existing `X-Forwarded-For` chain |
| `preserve` | Passes the inbound `X-Forwarded-For` chain through unchanged without adding the client IP |

Use `append` in multi-hop deployments where the full client IP chain must be preserved. Use `overwrite` (default) when spooky is the first edge and inbound forwarded headers should not be trusted.

#### Examples

```yaml
upstream:
  # First edge — overwrite inbound XFF with real client IP (default)
  public_pool:
    forwarded_headers:
      mode: overwrite
    backends: [...]

  # Behind another trusted proxy — append to the existing chain
  internal_pool:
    forwarded_headers:
      mode: append
    backends: [...]

  # Pass the inbound chain through unchanged
  passthrough_pool:
    forwarded_headers:
      mode: preserve
    backends: [...]
```

### Per-Upstream TLS Policy

Each upstream can optionally override the global `upstream_tls` settings with its own TLS profile. When `tls` is omitted, the global `upstream_tls` block applies.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `verify_certificates` | bool | No | `true` | Verify upstream TLS certificates |
| `strict_sni` | bool | No | `true` | Send backend authority host as SNI |
| `ca_file` | string | No | - | Path to a PEM CA bundle for this upstream |
| `ca_dir` | string | No | - | Path to a directory of PEM CA bundles for this upstream |

This is useful when backends have heterogeneous trust requirements — for example, one upstream uses a private internal CA while another uses a public CA.

Verification semantics:

- Hostname backends verify the upstream certificate against the configured backend hostname.
- IP-literal backends verify against the configured IP identity.
- `strict_sni: false` disables only the SNI extension; verification still remains enabled unless `verify_certificates: false`.
- `verify_certificates: false` disables upstream certificate validation entirely.

#### Examples

```yaml
upstream_tls:
  verify_certificates: true   # global default
  strict_sni: true

upstream:
  # Uses global upstream_tls — no override needed
  public_pool:
    route:
      path_prefix: "/api"
    backends: [...]

  # Override: trust a private CA for this upstream only
  internal_pool:
    tls:
      verify_certificates: true
      strict_sni: true
      ca_file: "/etc/spooky/certs/internal-ca.pem"
    route:
      path_prefix: "/internal"
    backends: [...]

  # Override: disable verification for a trusted dev upstream
  dev_pool:
    tls:
      verify_certificates: false
      strict_sni: false
    route:
      path_prefix: "/dev"
    backends: [...]
```

## Load Balancing Configuration

Load balancing determines how requests are distributed across healthy backends within an upstream pool. Each pool configures its own strategy independently.

### Properties

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `type` | string | Yes | - | Load balancing algorithm |
| `key` | string | No | - | Optional key source for `consistent-hash` and `sticky-cid` (`header:<name>`, `cookie:<name>`, `query:<name>`, `path`, `authority`, `method`, `cid`) |

### Supported Algorithms

#### random

Selects a backend randomly from all healthy backends. Weight values are currently ignored.

```yaml
upstream:
  my_pool:
    load_balancing:
      type: "random"
```

#### round-robin

Distributes requests evenly across all healthy backends in sequential order. Weight values are currently ignored.

```yaml
upstream:
  my_pool:
    load_balancing:
      type: "round-robin"
```

#### consistent-hash

Routes requests using consistent hashing. By default it hashes request authority (if present), otherwise request path, otherwise HTTP method. Set `load_balancing.key` to override key derivation.

```yaml
upstream:
  my_pool:
    load_balancing:
      type: "consistent-hash"
      key: "header:x-user-id"
```

#### least-connections

Selects the healthy backend with the fewest active requests. Ties are deterministic by backend index order.

```yaml
upstream:
  my_pool:
    load_balancing:
      type: "least-connections"
```

#### latency-aware

Selects healthy backends using a latency score built from EWMA backend latency and active request pressure. Unsampled backends are probed first to avoid cold-start bias.

```yaml
upstream:
  my_pool:
    load_balancing:
      type: "latency-aware"
```

#### sticky-cid

Uses consistent hashing keyed by QUIC connection ID for connection-level stickiness. The same CID is routed to the same backend while healthy membership is stable.

```yaml
upstream:
  my_pool:
    load_balancing:
      type: "sticky-cid"
```

### Algorithm Selection

- Use `random` for simple stateless load distribution
- Use `round-robin` for even distribution across backends
- Use `consistent-hash` when session affinity or request consistency is required
- Use `least-connections` when backend load varies significantly across requests
- Use `latency-aware` when you want faster backends to absorb more traffic
- Use `sticky-cid` for QUIC-connection affinity without application-level stickiness keys

### Examples

```yaml
upstream:
  api_pool:
    load_balancing:
      type: "consistent-hash"
    route:
      path_prefix: "/api"
    backends: [...]

  default_pool:
    load_balancing:
      type: "round-robin"
    route:
      path_prefix: "/"
    backends: [...]
```

## Logging Configuration

Controls logging output, verbosity, and destination.

### Properties

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `level` | string | No | `info` | Log level |
| `format` | string | No | `plain` | Output format: `plain` (human-readable) or `json` (structured) |
| `file.enabled` | bool | No | `false` | Write logs to a file instead of stderr |
| `file.path` | string | No | `/var/log/spooky/spooky.log` | Log file path (used when `file.enabled` is `true`) |

### Log Levels

Log levels in order of increasing verbosity:

- `silence`: No logging output
- `poltergeist`: Error messages only
- `scream`: Warnings and errors
- `spooky`: Informational messages, warnings, and errors
- `haunt`: Debug information
- `whisper`: Trace-level debugging

Standard log level mapping:

- `silence` = off
- `poltergeist` = error
- `scream` = warn
- `spooky` = info
- `haunt` = debug
- `whisper` = trace

### Examples

```yaml
# stderr only (default)
log:
  level: info
  format: plain

# Write to file
log:
  level: info
  format: plain
  file:
    enabled: true
    path: /var/log/spooky/spooky.log

# Structured JSON logs (recommended for log pipelines)
log:
  level: info
  format: json

# Development — debug to stderr
log:
  level: haunt  # debug level
  format: plain

# Troubleshooting — trace to file
log:
  level: whisper  # trace level
  format: json
  file:
    enabled: true
    path: /tmp/spooky-trace.log
```

## Performance Configuration

Controls resource limits, tuning knobs, and connection-flood protection. All fields are optional and fall back to sane defaults.

### Properties

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `worker_threads` | integer | No | `1` | Number of polling worker threads |
| `control_plane_threads` | integer | No | `2` | Tokio worker threads for the control-plane runtime (startup, health checks, metrics, and other async control tasks) |
| `reuseport` | bool | No | `true` | Enable `SO_REUSEPORT`; required when `worker_threads > 1` |
| `pin_workers` | bool | No | `false` | Pin each worker thread to a dedicated CPU core |
| `global_inflight_limit` | integer | No | `4096` | Maximum concurrent in-flight requests across all upstreams |
| `per_upstream_inflight_limit` | integer | No | `1024` | Maximum concurrent in-flight requests per upstream pool |
| `per_backend_inflight_limit` | integer | No | `64` | Maximum concurrent in-flight requests per backend |
| `backend_timeout_ms` | integer | No | `2000` | Initial backend response timeout (ms) |
| `backend_connect_timeout_ms` | integer | No | `500` | Backend TCP/TLS handshake timeout (ms); must be ≤ `backend_timeout_ms` |
| `backend_body_idle_timeout_ms` | integer | No | `2000` | Idle timeout while streaming response body (ms); must be ≥ `backend_timeout_ms` |
| `backend_body_total_timeout_ms` | integer | No | `30000` | Maximum wait for first upstream body bytes (ms); after body progress, idle timeout governs chunk pacing |
| `backend_total_request_timeout_ms` | integer | No | `35000` | Hard deadline for an entire request round-trip (ms); must be ≥ `backend_body_total_timeout_ms` |
| `shutdown_drain_timeout_ms` | integer | No | `5000` | Graceful-shutdown drain timeout in ms; active connections are force-closed once this deadline is reached |
| `udp_recv_buffer_bytes` | integer | No | `8388608` | UDP socket receive buffer size (bytes) |
| `udp_send_buffer_bytes` | integer | No | `8388608` | UDP socket send buffer size (bytes) |
| `h2_pool_max_idle_per_backend` | integer | No | `256` | Maximum idle HTTP/2 connections kept open per backend |
| `h2_pool_idle_timeout_ms` | integer | No | `90000` | How long an idle H2 connection is kept before being closed (ms) |
| `backend_dns_refresh_enabled` | bool | No | `false` | Enable periodic DNS refresh for hostname-based upstream backends |
| `backend_dns_refresh_interval_ms` | integer | No | `30000` | Control-plane DNS refresh interval for hostname-based upstream backends (ms) |
| `new_connections_per_sec` | integer | No | `2000` | Steady-state rate at which new QUIC connections are accepted (token-bucket refill, connections/sec) |
| `new_connections_burst` | integer | No | `500` | Burst capacity above the steady-state rate; the bucket starts full so the first burst of legitimate connections always succeeds |
| `max_active_connections` | integer | No | `20000` | Hard cap on active QUIC connections per worker; unknown `Initial` packets are dropped once this cap is reached |
| `quic_max_idle_timeout_ms` | integer | No | `5000` | QUIC idle timeout in ms; connection is closed after this period of inactivity |
| `quic_initial_max_data` | integer | No | `10000000` | Connection-level QUIC flow control window in bytes |
| `quic_initial_max_stream_data` | integer | No | `1000000` | Per-stream QUIC flow control window in bytes; must be ≤ `quic_initial_max_data` |
| `quic_initial_max_streams_bidi` | integer | No | `100` | Maximum concurrent bidirectional QUIC streams per connection |
| `quic_initial_max_streams_uni` | integer | No | `100` | Maximum concurrent unidirectional QUIC streams per connection |
| `max_response_body_bytes` | integer | No | `104857600` | Hard cap on upstream response body bytes per stream; streams exceeding this return 503 (`upstream response body too large`) |

### Connection flood protection

`new_connections_per_sec` and `new_connections_burst` implement a token-bucket rate limiter on new QUIC connection accepts. The bucket starts full so legitimate burst traffic at startup is never penalised. Packets for **existing** connections are never affected by this limit — only unknown `Initial` packets that would create a new connection state entry are gated.

`max_active_connections` is a separate hard guardrail for total connection state. Use it to enforce deterministic memory limits under sustained handshake floods even when token-bucket limits allow temporary bursts.

```yaml
performance:
  new_connections_per_sec: 2000   # refill rate: 2 k new conns/sec
  new_connections_burst: 500      # allow a burst of up to 500 above the rate
  max_active_connections: 20000   # hard ceiling for concurrently tracked connections
```

Set `new_connections_burst` to `1` and `new_connections_per_sec` to a low value to aggressively throttle connection floods at the cost of rejecting legitimate concurrent handshakes.

### Examples

```yaml
# Single-worker, conservative limits
performance:
  worker_threads: 1
  global_inflight_limit: 1024
  new_connections_per_sec: 500
  new_connections_burst: 100

# High-throughput multi-worker setup
performance:
  worker_threads: 8
  reuseport: true
  pin_workers: true
  global_inflight_limit: 16384
  per_upstream_inflight_limit: 4096
  per_backend_inflight_limit: 256
  new_connections_per_sec: 10000
  new_connections_burst: 2000
```

## Resilience Configuration

Controls retry budgets, circuit breaking, hedging, adaptive admission, brownout shedding, route queuing, protocol policy, and the worker watchdog. All fields are optional and fall back to production-tuned defaults.

### adaptive_admission

Dynamically adjusts the global in-flight request limit based on observed backend latency.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `enabled` | bool | No | `true` | Enable adaptive admission control |
| `min_limit` | integer | No | `64` | Floor for the dynamic in-flight limit; must be > 0 |
| `max_limit` | integer | No | `performance.global_inflight_limit` | Optional ceiling for the adaptive in-flight limit; must be >= `min_limit` and <= `performance.global_inflight_limit` |
| `decrease_step` | integer | No | `16` | Amount to subtract from the limit on high-latency observation |
| `increase_step` | integer | No | `16` | Amount to add to the limit on healthy-latency observation |
| `high_latency_ms` | integer | No | `500` | Latency threshold (ms) above which the limit is decreased |

### circuit_breaker

Tracks consecutive failures per backend and opens the circuit to stop sending requests to a failing backend.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `enabled` | bool | No | `true` | Enable per-backend circuit breakers |
| `failure_threshold` | integer | No | `3` | Consecutive failures before opening the circuit |
| `open_ms` | integer | No | `30000` | How long (ms) the circuit stays open before probing |
| `half_open_max_probes` | integer | No | `1` | Probe requests allowed during half-open state |

### retry_budget

Limits retried requests as a fraction of primary requests to prevent retry amplification.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `enabled` | bool | No | `true` | Enable retry budget enforcement |
| `ratio_percent` | integer | No | `10` | Max retries as a percentage of primary requests (0–100) |
| `per_route_ratio_percent` | map | No | `{}` | Per-route overrides: `{ "/api": 5 }` |

### hedging

Fires a speculative second request to an alternate backend when the primary is slow.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `enabled` | bool | No | `false` | Enable request hedging |
| `delay_ms` | integer | No | `100` | Delay (ms) before firing the hedge; must be > 0 when `enabled` is true |
| `safe_methods` | list | No | `["GET","HEAD"]` | HTTP methods eligible for hedging |
| `route_allowlist` | list | No | `[]` | Routes eligible for hedging; empty means all routes |

### brownout

Brownout is a load-shedding mode that activates when the proxy is near capacity. When active, every incoming request whose upstream pool is **not** in `core_routes` is immediately rejected with `503 Service Unavailable` and a `Retry-After` header. Requests on core routes continue to be processed normally.

**How it works**

1. After each request is routed, Spooky samples the current global in-flight percent (active requests ÷ global limit × 100).
2. If the sample reaches `trigger_inflight_percent`, brownout activates.
3. Brownout stays active until the sample falls to or below `recover_inflight_percent`. The gap between the two thresholds is **hysteresis** — it prevents rapid oscillation when load is right at the boundary.
4. While active, `spooky_brownout_active` gauge is `1` and `spooky_overload_shed_by_reason_total{reason="brownout"}` increments for every shed request.

**Choosing `core_routes`**

`core_routes` is a list of upstream pool names (the `id` field under `upstreams[].pool`). Routes not in this list are shed during brownout.

- If `core_routes` is empty (the default), **all routes** are shed during brownout. This is safe but means brownout effectively becomes a full-stop — no requests get through.
- List only the routes that must keep working during a partial outage: authentication, payments, health checks. Avoid listing high-volume non-critical routes or you defeat the purpose of shedding.
- A route shed during brownout receives a `503` with the body `brownout active, non-core route shed` and a `Retry-After` hint. Clients that respect `Retry-After` will back off automatically.

**Interaction with other overload mechanisms**

Brownout runs after routing but before adaptive admission and circuit breakers. The order is:

1. **Brownout** — shed non-core routes immediately (no backend resource consumed)
2. **Adaptive admission** — dynamically cap total in-flight based on observed latency
3. **Per-upstream / per-backend inflight limits** — static caps per pool and backend
4. **Circuit breaker** — stop sending to a specific failing backend

If brownout is active and shedding load, adaptive admission will also begin to recover (inflight drops → limit rises). Once the in-flight percent falls to `recover_inflight_percent`, brownout deactivates and full traffic resumes. Set `recover_inflight_percent` at least 20–30 points below `trigger_inflight_percent` to give the system time to recover before re-admitting full traffic.

**Alerting**

Alert on `spooky_brownout_active == 1` for more than a brief window — sustained brownout means backends are under-provisioned or a downstream dependency is slow:

```yaml
- alert: SpookyBrownoutActive
  expr: spooky_brownout_active == 1
  for: 30s
  labels:
    severity: warning
  annotations:
    summary: "Spooky brownout active on {{ $labels.instance }}"
    description: "Non-core routes are being shed. Check backend latency and inflight metrics."
```

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `enabled` | bool | No | `true` | Enable brownout shedding |
| `trigger_inflight_percent` | integer | No | `90` | Inflight % at which brownout activates (0–100) |
| `recover_inflight_percent` | integer | No | `60` | Inflight % at which brownout deactivates; must be < `trigger_inflight_percent` |
| `core_routes` | list | No | `[]` | Upstream pool names exempt from shedding; empty means all routes are shed |

### route_queue

Per-route and global caps on queued (waiting) requests.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `default_cap` | integer | No | `512` | Per-route queue depth cap |
| `global_cap` | integer | No | `2048` | Total queue depth cap across all routes |
| `shed_retry_after_seconds` | integer | No | `1` | `Retry-After` header value (seconds) sent with 503 queue-shed responses |
| `caps` | map | No | `{}` | Per-route overrides: `{ "/api": 128 }` |

### protocol

Request validation and early-data policy.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `allow_0rtt` | bool | No | `false` | Accept 0-RTT early data |
| `early_data_safe_methods` | list | No | `["GET","HEAD"]` | Methods permitted in 0-RTT early data |
| `max_headers_count` | integer | No | `128` | Maximum number of request headers |
| `max_headers_bytes` | integer | No | `16384` | Maximum total size of request headers (bytes) |
| `enforce_authority_host_match` | bool | No | `true` | Reject requests where `:authority` differs from `Host` |
| `allow_connect` | bool | No | `false` | Enable CONNECT proxy tunneling |
| `connect_allowed_ports` | list | No | `[]` | Optional CONNECT target port allowlist |
| `connect_allowed_authorities` | list | No | `[]` | Optional exact CONNECT `host:port` allowlist |
| `allowed_methods` | list | No | `[]` | Allowed HTTP methods; empty means all methods allowed |
| `denied_path_prefixes` | list | No | `[]` | Path prefixes that are always rejected with 403 |

Request-shape rules enforced by the runtime:

- HTTP/3 requests are rejected when `:authority` and `Host` differ and `enforce_authority_host_match` is enabled.
- `CONNECT` requires `:authority`/`Host` in `host:port` form and must also satisfy the CONNECT allowlists when enabled.
- Native HTTP/3 ingress rejects `Upgrade` / `Connection: upgrade` style requests. WebSocket-style upgrades are only supported on the bootstrap HTTP/1.1 compatibility path, not on native H3.
- `HEAD` responses terminate after headers even if the upstream attempted to send a body.

### watchdog

Monitors worker health and triggers a restart hook when error rates or stall conditions exceed thresholds.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `enabled` | bool | No | `false` | Enable the worker watchdog |
| `check_interval_ms` | integer | No | `1000` | How often (ms) the watchdog evaluates metrics |
| `poll_stall_timeout_ms` | integer | No | `5000` | Declare a stall if the event loop hasn't polled within this window |
| `timeout_error_rate_percent` | integer | No | `60` | Trigger if timeout errors exceed this % of requests in a window |
| `min_requests_per_window` | integer | No | `20` | Minimum requests in a window before error-rate check applies |
| `overload_inflight_percent` | integer | No | `95` | Trigger if in-flight % exceeds this threshold |
| `unhealthy_consecutive_windows` | integer | No | `3` | Consecutive unhealthy windows before invoking the restart hook |
| `drain_grace_ms` | integer | No | `8000` | Grace period (ms) to drain connections before restarting |
| `restart_cooldown_ms` | integer | No | `120000` | Minimum time (ms) between restart hook invocations |
| `restart_hook` | string | No | `null` | Shell command invoked on restart trigger |

### Startup Validation Errors

The following resilience configurations are rejected at startup with a descriptive error:

| Condition | Error |
|-----------|-------|
| `recover_inflight_percent >= trigger_inflight_percent` | brownout hysteresis inverted |
| `adaptive_admission.min_limit == 0` | min_limit must be > 0 |
| `adaptive_admission.max_limit == 0` | max_limit must be > 0 when provided |
| `adaptive_admission.max_limit < adaptive_admission.min_limit` | max_limit must be >= min_limit |
| `adaptive_admission.max_limit > performance.global_inflight_limit` | max_limit must be <= global_inflight_limit |
| `retry_budget.ratio_percent > 100` | ratio_percent must be 0–100 |
| `hedging.enabled && delay_ms == 0` | delay_ms must be > 0 when hedging is enabled |

### Example

```yaml
resilience:
  adaptive_admission:
    enabled: true
    min_limit: 64
    max_limit: 4096
    high_latency_ms: 500

  circuit_breaker:
    enabled: true
    failure_threshold: 3
    open_ms: 30000
    half_open_max_probes: 1

  retry_budget:
    enabled: true
    ratio_percent: 10

  hedging:
    enabled: false
    delay_ms: 100

  brownout:
    enabled: true
    trigger_inflight_percent: 90
    recover_inflight_percent: 60
    core_routes:
      - "auth_pool"
      - "payments_pool"
```

## Observability Endpoint Hardening

When enabling `observability.metrics` or `observability.control_api`, keep endpoints on loopback unless you intentionally expose them behind network controls.

### Metrics Endpoint

Key fields:

- `observability.metrics.max_connections` (default: `512`): concurrent connection cap.
- `observability.metrics.connection_timeout_ms` (default: `30000`): per-connection lifetime timeout.

### Control API Endpoint

Key fields:

- `observability.control_api.auth_token`: bearer token required for runtime, reload-certs, and restart endpoints (`Authorization: Bearer <token>`).
- `observability.control_api.reload_certs_path`: authenticated POST endpoint that reloads listener certificate and client-auth CA material for new handshakes.
- `observability.control_api.max_connections` (default: `256`): concurrent connection cap.
- `observability.control_api.connection_timeout_ms` (default: `30000`): per-connection lifetime timeout.

If `observability.control_api.address` is non-loopback, `observability.control_api.auth_token` is required.

### Routing Transparency

`observability.routing` enables explicit route-decision logging.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `enabled` | boolean | No | `false` | Emit route-decision transparency logs |
| `include_reason` | boolean | No | `true` | Include deterministic tie-break reason in route-decision logs |
| `expose_header` | boolean | No | `false` | Reserved toggle for downstream route-decision response headers |
| `header_name` | string | No | `"x-spooky-route-decision"` | Reserved header name; must be non-empty when `expose_header=true` |

### Watchdog Restart Hook

Use structured command execution:

- `resilience.watchdog.restart_command`: array, where index `0` is executable and remaining entries are arguments.

Legacy `resilience.watchdog.restart_hook` is deprecated and rejected by validation.

## Configuration Validation

Spooky validates configuration at startup and reports errors before attempting to start the server.

### Common Validation Errors

1. **Missing required fields**
   - Neither `listen.tls.cert/key` nor `listen.tls.certificates` specified
   - Backend address or ID missing
   - Route configuration empty

2. **Invalid file paths**
   - TLS certificate file not found or not readable
   - TLS key file not found or not readable
   - Incorrect file permissions

3. **Invalid values**
   - Port number out of range (1-65535)
   - Invalid IP address format
   - Invalid backend address format (accepted: `host:port`, `https://host:port`, `http://host:port`, or bare `host`; scheme-default port is inferred when omitted)
   - Duplicate backend IDs within a pool

4. **Configuration conflicts**
   - Port already in use
   - Duplicate upstream pool names
   - Overlapping or ambiguous route definitions
   - Brownout `recover_inflight_percent` ≥ `trigger_inflight_percent`
   - `adaptive_admission.min_limit` set to 0
   - `retry_budget.ratio_percent` > 100
   - `hedging.enabled` with `delay_ms` = 0

### Testing Configuration

Validate configuration without starting the server:

```bash
spooky --config <path>
```

The command exits with status 0 if configuration is valid, or prints detailed error messages and exits with non-zero status if invalid.

## Complete Working Example

```yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 9889
  tls:
    cert: "certs/proxy-fullchain.pem"
    key: "certs/proxy-key-pkcs8.pem"

upstream:
  api_pool:
    load_balancing:
      type: "consistent-hash"

    route:
      path_prefix: "/api"

    backends:
      - id: "backend1"
        address: "https://127.0.0.1:7001"
        weight: 100
        health_check:
          path: "/health"
          interval: 5000

      - id: "backend2"
        address: "https://127.0.0.1:7002"
        weight: 50
        health_check:
          path: "/status"
          interval: 10000

  default_pool:
    load_balancing:
      type: "round-robin"

    route:
      path_prefix: "/"

    backends:
      - id: "auth1"
        address: "https://127.0.0.1:8001"
        weight: 100
        health_check:
          path: "/health"
          interval: 5000

log:
  level: debug
  format: plain
```
