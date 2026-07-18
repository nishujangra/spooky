# Configuration Defaults

This page is the central reference for configuration defaults in Spooky.

Use it when you need to answer two questions quickly:

- which fields may be omitted from the YAML
- what value or behavior Spooky applies when they are omitted

This page reflects the defaults defined in `crates/config/src/default.rs` and the `Default`-backed config structs in `crates/config/src/config.rs`.

## How Defaults Work

Spooky applies defaults in three different ways:

- explicit helper-function defaults such as `get_default_port()` or `perf_default_worker_threads()`
- struct-level `Default` implementations for nested sections such as `performance`, `observability`, and `resilience`
- Rust/Serde zero-value defaults for optional or collection fields such as `false`, `[]`, `{}`, empty strings, and `null`

Defaults only apply when a field is omitted. Validation still runs after defaults are applied, so an omitted field may deserialize successfully and still be rejected later if a related feature is enabled.

Examples:

- `observability.control_api.enabled` defaults to `false`
- `observability.control_api.auth_token` defaults to `null`
- if you set `observability.control_api.enabled: true`, validation then requires `auth_token`

## Important Non-Defaults

The following top-level and structural fields are still required and do not have a default:

- `listen`
- `listen.tls`
- `upstream`
- `upstream.<name>.route`
- `upstream.<name>.backends`
- `upstream.<name>.backends[].id`
- `upstream.<name>.backends[].address`
- `load_balancing.type` when a top-level `load_balancing` block is present

`listeners` is optional and defaults to `[]`, but if `listeners[]` is non-empty it overrides the top-level `listen` block at runtime.

## Top-Level Defaults

| Field | Default | Notes |
| --- | --- | --- |
| `version` | `1` | Current schema version |
| `listeners` | `[]` | Optional multi-listener override |
| `load_balancing` | `null` | Global fallback is absent unless configured |
| `upstream_tls` | object defaults | See [Upstream TLS Defaults](#upstream-tls-defaults) |
| `log` | object defaults | See [Log Defaults](#log-defaults) |
| `performance` | object defaults | See [Performance Defaults](#performance-defaults) |
| `observability` | object defaults | See [Observability Defaults](#observability-defaults) |
| `resilience` | object defaults | See [Resilience Defaults](#resilience-defaults) |
| `security` | object defaults | See [Security Defaults](#security-defaults) |

## Listen Defaults

| Field | Default | Notes |
| --- | --- | --- |
| `listen.protocol` | `"http3"` | Native ingress protocol |
| `listen.port` | `9889` | Default listener port |
| `listen.address` | `"0.0.0.0"` | Bind all IPv4 interfaces |
| `listen.tls.cert` | `""` | Legacy/default certificate path stays empty until set |
| `listen.tls.key` | `""` | Legacy/default key path stays empty until set |
| `listen.tls.certificates` | `[]` | Optional SNI certificate map |
| `listen.tls.client_auth.enabled` | `false` | Client certificate auth off by default |
| `listen.tls.client_auth.require_client_cert` | `false` | No client cert requirement by default |
| `listen.tls.client_auth.ca_file` | `null` | No client CA bundle by default |

## Upstream TLS Defaults

These defaults apply to the top-level `upstream_tls` block and to per-upstream `upstream.<name>.tls` when that nested block is present but fields are omitted.

| Field | Default | Notes |
| --- | --- | --- |
| `upstream_tls.verify_certificates` | `true` | HTTPS upstream verification stays enabled |
| `upstream_tls.strict_sni` | `true` | Upstream SNI stays strict by default |
| `upstream_tls.ca_file` | `null` | No custom CA file |
| `upstream_tls.ca_dir` | `null` | No custom CA directory |

## Upstream And Backend Defaults

### Upstream-Level Defaults

| Field | Default | Notes |
| --- | --- | --- |
| `upstream.<name>.load_balancing.type` | `"round-robin"` | Applied when the upstream omits `load_balancing` |
| `upstream.<name>.load_balancing.key` | `null` | No hash/sticky key source by default |
| `upstream.<name>.host_policy.mode` | `pass_through` | Preserve downstream host by default |
| `upstream.<name>.host_policy.host` | `null` | No rewrite target |
| `upstream.<name>.forwarded_headers.mode` | `overwrite` | Spooky rewrites forwarded headers by default |
| `upstream.<name>.tls` | `null` | Uses global `upstream_tls` unless an override block is set |

### Route Defaults

| Field | Default | Notes |
| --- | --- | --- |
| `upstream.<name>.route.host` | `null` | No host matcher unless set |
| `upstream.<name>.route.path_prefix` | `null` | No default path matcher; validation requires either host or path |
| `upstream.<name>.route.method` | `null` | No method restriction |

### Backend Defaults

| Field | Default | Notes |
| --- | --- | --- |
| `upstream.<name>.backends[].weight` | `100` | Equal weight unless overridden |
| `upstream.<name>.backends[].health_check` | `null` | No active health check block unless configured |

### Health Check Defaults

These apply when a backend provides a `health_check` object and omits individual fields.

| Field | Default | Notes |
| --- | --- | --- |
| `upstream.<name>.backends[].health_check.path` | `"/health"` | Default probe path |
| `upstream.<name>.backends[].health_check.interval` | `5000` | Probe interval in ms |
| `upstream.<name>.backends[].health_check.timeout_ms` | `1000` | Probe timeout in ms |
| `upstream.<name>.backends[].health_check.failure_threshold` | `3` | Consecutive failures before unhealthy |
| `upstream.<name>.backends[].health_check.success_threshold` | `2` | Consecutive successes before healthy |
| `upstream.<name>.backends[].health_check.cooldown_ms` | `5000` | Cooldown after failure in ms |

## Log Defaults

| Field | Default | Notes |
| --- | --- | --- |
| `log.level` | `"info"` | Standard default log verbosity |
| `log.file.enabled` | `false` | Logs go to stderr unless file logging is enabled |
| `log.file.path` | `"/var/log/spooky/spooky.log"` | Used only when file logging is enabled |
| `log.format` | `plain` | Human-readable text output |

## Performance Defaults

| Field | Default | Notes |
| --- | --- | --- |
| `performance.worker_threads` | `1` | Data-plane worker count |
| `performance.control_plane_threads` | `2` | Tokio control-plane worker count |
| `performance.packet_shards_per_worker` | `1` | Preserves single-loop packet processing by default |
| `performance.packet_shard_queue_capacity` | `2048` | Bounded queue depth per shard |
| `performance.packet_shard_queue_max_bytes` | `67108864` | 64 MiB queued-byte cap per shard |
| `performance.reuseport` | `true` | Multi-socket reuse is enabled by default |
| `performance.pin_workers` | `false` | CPU pinning is off by default |
| `performance.global_inflight_limit` | `4096` | Global request concurrency cap |
| `performance.per_upstream_inflight_limit` | `1024` | Per-upstream concurrency cap |
| `performance.inflight_acquire_wait_ms` | `0` | No micro-wait before shedding |
| `performance.backend_timeout_ms` | `2000` | General backend timeout |
| `performance.backend_connect_timeout_ms` | `500` | Backend connect timeout |
| `performance.backend_body_idle_timeout_ms` | `2000` | Backend response-body idle timeout |
| `performance.backend_body_total_timeout_ms` | `30000` | Backend response-body total timeout |
| `performance.backend_total_request_timeout_ms` | `35000` | End-to-end backend request timeout |
| `performance.shutdown_drain_timeout_ms` | `5000` | Shutdown drain budget |
| `performance.udp_recv_buffer_bytes` | `8388608` | 8 MiB UDP receive buffer |
| `performance.udp_send_buffer_bytes` | `8388608` | 8 MiB UDP send buffer |
| `performance.h2_pool_max_idle_per_backend` | `256` | Idle HTTP/2 connection pool cap per backend |
| `performance.h2_pool_idle_timeout_ms` | `90000` | Idle HTTP/2 pool timeout |
| `performance.backend_dns_refresh_enabled` | `false` | Hostname backend refresh off by default |
| `performance.backend_dns_refresh_interval_ms` | `30000` | DNS refresh interval |
| `performance.per_backend_inflight_limit` | `64` | Per-backend concurrency cap |
| `performance.new_connections_per_sec` | `2000` | Token-bucket steady refill rate |
| `performance.new_connections_burst` | `500` | Token-bucket burst size |
| `performance.max_active_connections` | `20000` | Hard active connection cap |
| `performance.quic_max_idle_timeout_ms` | `5000` | QUIC idle timeout |
| `performance.quic_initial_max_data` | `10000000` | QUIC connection flow-control window |
| `performance.quic_initial_max_stream_data` | `1000000` | QUIC stream flow-control window |
| `performance.quic_initial_max_streams_bidi` | `100` | Max bidi streams per connection |
| `performance.quic_initial_max_streams_uni` | `100` | Max uni streams per connection |
| `performance.max_response_body_bytes` | `104857600` | 100 MiB upstream response cap |
| `performance.max_request_body_bytes` | `1000000` | 1 MiB client request-body cap |
| `performance.request_buffer_global_cap_bytes` | `67108864` | 64 MiB worker request-buffer cap |
| `performance.unknown_length_response_prebuffer_bytes` | `2097152` | 2 MiB unknown-length prebuffer cap |
| `performance.client_body_idle_timeout_ms` | `10000` | Client upload idle timeout |

## Resilience Defaults

### Adaptive Admission

| Field | Default | Notes |
| --- | --- | --- |
| `resilience.adaptive_admission.enabled` | `true` | Adaptive admission control on by default |
| `resilience.adaptive_admission.min_limit` | `64` | Minimum inflight ceiling |
| `resilience.adaptive_admission.max_limit` | `null` | No explicit upper bound unless configured |
| `resilience.adaptive_admission.decrease_step` | `16` | Step down on overload |
| `resilience.adaptive_admission.increase_step` | `16` | Step up on recovery |
| `resilience.adaptive_admission.high_latency_ms` | `500` | Latency threshold for pressure signals |

### Route Queue

| Field | Default | Notes |
| --- | --- | --- |
| `resilience.route_queue.default_cap` | `512` | Default per-route queue size |
| `resilience.route_queue.global_cap` | `2048` | Global route-queue cap |
| `resilience.route_queue.shed_retry_after_seconds` | `1` | Retry-After hint on shed responses |
| `resilience.route_queue.caps` | `{}` | No per-route overrides by default |

### Protocol Policy

| Field | Default | Notes |
| --- | --- | --- |
| `resilience.protocol.allow_0rtt` | `false` | 0-RTT disabled by default |
| `resilience.protocol.allow_connect` | `false` | CONNECT disabled by default |
| `resilience.protocol.early_data_safe_methods` | `["GET", "HEAD"]` | Safe methods allowed if 0-RTT is enabled |
| `resilience.protocol.max_headers_count` | `128` | Header count cap |
| `resilience.protocol.max_headers_bytes` | `16384` | 16 KiB aggregate header budget |
| `resilience.protocol.enforce_authority_host_match` | `true` | `:authority` and `Host` must align |
| `resilience.protocol.allowed_methods` | `[]` | Empty means no explicit allowlist |
| `resilience.protocol.denied_path_prefixes` | `[]` | No denied prefixes by default |
| `resilience.protocol.connect_allowed_ports` | `[]` | No CONNECT allowlist entries |
| `resilience.protocol.connect_allowed_authorities` | `[]` | No CONNECT authority allowlist entries |

### Circuit Breaker

| Field | Default | Notes |
| --- | --- | --- |
| `resilience.circuit_breaker.enabled` | `true` | Circuit breaker on by default |
| `resilience.circuit_breaker.failure_threshold` | `3` | Consecutive failures to open |
| `resilience.circuit_breaker.open_ms` | `30000` | Open interval in ms |
| `resilience.circuit_breaker.half_open_max_probes` | `1` | Probe requests allowed while half-open |

### Hedging

| Field | Default | Notes |
| --- | --- | --- |
| `resilience.hedging.enabled` | `false` | Hedging off by default |
| `resilience.hedging.delay_ms` | `100` | Delay before hedge attempt |
| `resilience.hedging.safe_methods` | `["GET", "HEAD"]` | Hedge-safe methods |
| `resilience.hedging.route_allowlist` | `[]` | No route allowlist restrictions by default |

### Retry Budget

| Field | Default | Notes |
| --- | --- | --- |
| `resilience.retry_budget.enabled` | `true` | Retry budget enforcement on by default |
| `resilience.retry_budget.ratio_percent` | `10` | Retry budget ratio |
| `resilience.retry_budget.per_route_ratio_percent` | `{}` | No per-route overrides by default |

### Brownout

| Field | Default | Notes |
| --- | --- | --- |
| `resilience.brownout.enabled` | `true` | Brownout policy on by default |
| `resilience.brownout.trigger_inflight_percent` | `90` | Brownout enters near saturation |
| `resilience.brownout.recover_inflight_percent` | `60` | Brownout exits with headroom |
| `resilience.brownout.core_routes` | `[]` | No explicit core-route list by default |

### Watchdog

| Field | Default | Notes |
| --- | --- | --- |
| `resilience.watchdog.enabled` | `false` | Watchdog off by default |
| `resilience.watchdog.check_interval_ms` | `1000` | Poll interval |
| `resilience.watchdog.poll_stall_timeout_ms` | `5000` | Stall threshold |
| `resilience.watchdog.timeout_error_rate_percent` | `60` | Timeout-rate unhealthy threshold |
| `resilience.watchdog.min_requests_per_window` | `20` | Minimum request volume before action |
| `resilience.watchdog.overload_inflight_percent` | `95` | Overload inflight threshold |
| `resilience.watchdog.unhealthy_consecutive_windows` | `3` | Required consecutive bad windows |
| `resilience.watchdog.drain_grace_ms` | `8000` | Drain grace before restart |
| `resilience.watchdog.restart_cooldown_ms` | `120000` | Restart cooldown |
| `resilience.watchdog.restart_command` | `[]` | No structured restart command by default |
| `resilience.watchdog.restart_hook` | `null` | Deprecated legacy hook absent by default |

## Observability Defaults

### Metrics Endpoint

| Field | Default | Notes |
| --- | --- | --- |
| `observability.metrics.enabled` | `false` | Metrics endpoint disabled by default |
| `observability.metrics.required` | `false` | Startup does not require the metrics endpoint |
| `observability.metrics.address` | `"127.0.0.1"` | Loopback-only by default |
| `observability.metrics.port` | `9901` | Default metrics port |
| `observability.metrics.path` | `"/metrics"` | Default scrape path |
| `observability.metrics.max_connections` | `512` | Concurrent metrics connections cap |
| `observability.metrics.connection_timeout_ms` | `30000` | Metrics connection timeout |

### Control API

| Field | Default | Notes |
| --- | --- | --- |
| `observability.control_api.enabled` | `false` | Control API disabled by default |
| `observability.control_api.required` | `false` | Startup does not require the control API |
| `observability.control_api.address` | `"127.0.0.1"` | Loopback-only by default |
| `observability.control_api.port` | `9902` | Default control API port |
| `observability.control_api.health_path` | `"/health"` | Health endpoint path |
| `observability.control_api.ready_path` | `"/ready"` | Readiness endpoint path |
| `observability.control_api.runtime_path` | `"/admin/runtime"` | Runtime summary path |
| `observability.control_api.restart_path` | `"/admin/runtime/restart"` | Restart control path |
| `observability.control_api.reload_path` | `"/admin/runtime/reload"` | Full config hot-reload path |
| `observability.control_api.reload_certs_path` | `"/admin/runtime/reload-certs"` | Certificate reload path |
| `observability.control_api.auth_token` | `null` | Must be set when the control API is enabled |
| `observability.control_api.max_connections` | `256` | Concurrent control API connections cap |
| `observability.control_api.connection_timeout_ms` | `30000` | Control API connection timeout |

### Tracing

| Field | Default | Notes |
| --- | --- | --- |
| `observability.tracing.enabled` | `false` | Tracing disabled by default |
| `observability.tracing.service_name` | `"spooky"` | Default OTLP service name |
| `observability.tracing.otlp_endpoint` | `null` | No exporter endpoint by default |
| `observability.tracing.sample_ratio` | `1.0` | Full sampling if tracing is enabled |

### Routing Transparency

| Field | Default | Notes |
| --- | --- | --- |
| `observability.routing.enabled` | `false` | Route-decision reporting disabled by default |
| `observability.routing.include_reason` | `true` | Include decision reasons when routing transparency is enabled |
| `observability.routing.expose_header` | `false` | Response header exposure disabled by default |
| `observability.routing.header_name` | `"x-spooky-route-decision"` | Default transparency header name |

## Security Defaults

| Field | Default | Notes |
| --- | --- | --- |
| `security.privileges.enabled` | `true` | Privilege drop is enabled by default |
| `security.privileges.user` | `"nobody"` | Default drop-to user |
| `security.privileges.group` | `"nogroup"` | Default drop-to group |

## Related Pages

- [Configuration Reference](reference.md)
- [Configuration Examples](examples.md)
- [Production Readiness](../operations/production-readiness.md)
