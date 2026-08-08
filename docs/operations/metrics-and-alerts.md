# Metrics and Alerts

This document explains which Spooky metrics matter most in production and how operators should use them for alerting and dashboards.

## Purpose

Spooky exposes many counters, gauges, and labeled request families. Operators should focus on the metrics that answer:

- is traffic succeeding
- is latency rising
- is admission or overload policy firing
- are backends healthy
- are retries and hedges increasing
- is the runtime generation and control plane behaving normally

For the full catalog, see [Metrics Reference](../reference/metrics-reference.md).

Distributed quota-specific interpretation lives in
[Distributed Quota](distributed-quota.md). Use that page when you need to
distinguish quota exhaustion, quota backend degradation, and overload shedding.

## Primary Production Domains

The metrics surface is easiest to understand by domain.

### Request outcome metrics

These answer whether traffic is succeeding or failing.

Key families:

- `spooky_requests_total`
- `spooky_requests_success`
- `spooky_requests_failure`
- `spooky_upstream_requests_total{upstream,status_class,outcome}`
- `spooky_backend_requests_total{upstream,backend,status_class,outcome}`

Use these to answer:

- which upstream is failing
- which backend is producing the failures
- whether failures are timeouts, overload, rate-limit, or backend errors

### Latency metrics

These answer how long requests are taking.

Key families:

- `spooky_upstream_request_latency_ms_bucket`
- `spooky_upstream_request_latency_ms_sum`
- `spooky_upstream_request_latency_ms_count`
- `spooky_route_latency_ms_p50`
- `spooky_route_latency_ms_p95`
- `spooky_route_latency_ms_p99`

Prefer the histogram family for service-level alerting and the route percentile gauges for quick route-level dashboards.

### Admission and overload metrics

These answer whether the system is refusing work before dispatch.

Key families:

- `spooky_policy_denied`
- `spooky_request_rate_limited`
- `spooky_overload_shed`
- `spooky_overload_shed_by_reason_total{reason=...}`
- `spooky_inflight_wait_admit_total{scope=...}`
- `spooky_brownout_active`
- `spooky_circuit_breaker_rejected_total`

These are especially important during load tests, incident response, and capacity tuning.

### Backend health and lifecycle metrics

These answer whether upstream capacity is healthy.

Key families:

- `spooky_health_checks_total`
- `spooky_health_checks_success`
- `spooky_health_checks_failure`
- `spooky_backend_timeouts`
- `spooky_backend_errors`
- `spooky_health_failures_total{reason=...}`
- `spooky_backend_dns_refresh_success_total`
- `spooky_backend_dns_refresh_failure_total`
- `spooky_backend_dns_address_set_changes_total`
- `spooky_backend_client_rotations_total`

These should be interpreted together with backend lifecycle snapshots from the control API.

### Buffering and body-pressure metrics

These answer whether request or response streaming pressure is building.

Key families:

- `spooky_request_buffered_bytes`
- `spooky_request_buffered_high_watermark_bytes`
- `spooky_request_buffer_limit_rejects`
- `spooky_response_prebuffer_limit_rejects`

These are important leading indicators of backpressure or response-shaping issues before outright traffic failure.

### Retry and hedge metrics

These answer whether resiliency behavior is activating.

Key families:

- `spooky_retries_total`
- `spooky_retry_denied_total{reason=...}`
- `spooky_retry_attempts_total{reason=...}`
- `spooky_hedge_triggered_total`
- `spooky_hedge_won_total`
- `spooky_hedge_wasted_total`
- `spooky_hedge_primary_won_after_trigger_total`

Rising retries or hedges may be correct behavior, but sustained increases usually indicate backend degradation or timeout pressure.

### Connection and ingress metrics

These answer whether the edge ingress path itself is healthy.

Key families:

- `spooky_active_connections`
- `spooky_connection_cap_rejects`
- `spooky_ingress_packets_total`
- `spooky_ingress_queue_drops`
- `spooky_ingress_queue_drop_bytes`
- `spooky_ingress_queue_bytes`
- `spooky_ingress_bad_header_total`
- `spooky_ingress_rate_limited_total`
- `spooky_ingress_unroutable_total`
- `spooky_ingress_draining_drops_total`

These are key signals for edge saturation, malformed traffic, and drain behavior.

### TLS metrics

These answer whether TLS negotiation is healthy on both downstream and upstream paths.

Key families:

- `spooky_downstream_tls_handshake_success_total`
- `spooky_downstream_tls_handshake_failure_total{listener,reason}`
- `spooky_downstream_tls_certificate_selection_total{listener,selection}`
- `spooky_downstream_tls_alpn_total{listener,protocol}`
- `spooky_upstream_tls_failure_total{backend,phase,reason}`

These are critical for certificate rollouts and protocol transition debugging.

### Control-plane and runtime metrics

These answer whether the administrative surfaces and watchdog are stable.

Key families:

- `spooky_control_api_connection_limit_drops`
- `spooky_watchdog_restart_requests`
- `spooky_watchdog_restart_hooks`
- `spooky_watchdog_degraded_windows`
- `spooky_runtime_panics`

These are not request SLO metrics, but they are important for platform reliability.

## First Dashboards To Build

Start with dashboards grouped by the domains above.

Recommended first dashboards:

- request success/failure by upstream
- backend outcome and timeout dashboard
- upstream latency percentile dashboard
- admission and overload dashboard
- retry and hedge activity dashboard
- backend DNS refresh and health dashboard
- ingress saturation and buffering dashboard
- TLS health dashboard
- runtime/control-plane health dashboard

## First Alerts To Add

### Request failure alerts

Alert on sustained increases in:

- upstream 5xx rate
- backend error outcome rate
- timeout outcome rate

### Admission alerts

Alert on sustained growth in:

- overload shed by reason
- rate-limit denials where unexpected
- request buffer or response prebuffer rejects

### Backend health alerts

Alert on:

- health-check failure rate increases
- backend timeout spikes
- backend DNS refresh failures
- upstream TLS failure increases

### Ingress saturation alerts

Alert on:

- ingress queue drops
- ingress queue bytes remaining elevated
- connection cap rejects
- request buffered bytes remaining elevated

### Control-plane alerts

Alert on:

- watchdog restart requests
- watchdog degraded windows
- runtime panics
- unexpected control API limiter drops

## Metric Interpretation Rules

### Use labeled families first

The coarse top-level counters are useful, but production dashboards should usually prefer the labeled upstream and backend families so the problem can be localized.

### Use canonical reason labels

Reason labels are part of the stable observability contract. Build alerts and dashboards on canonical values rather than prose parsing.

### Correlate metrics with control-plane snapshots

Metrics tell you trend and rate. Control-plane snapshots tell you current runtime state. Use both:

- metrics to detect an issue
- control API runtime and backend inventory to inspect current state

## Operator Expectations

Metrics should let operators answer:

- what is failing
- where it is failing
- why it is failing in canonical reason terms
- whether the problem is ingress, policy, backend, transport, TLS, or runtime control

If a dashboard requires reading implementation-specific logs to interpret a core metric, the dashboard likely needs to be redesigned around the canonical labeled families.

## Related Pages

- [Metrics Reference](../reference/metrics-reference.md)
- [Observability Contract](../architecture/observability-contract.md)
- [Control Plane](control-plane.md)
- [Reload and Drain](reload-and-drain.md)
