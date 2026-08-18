# Metrics Reference

This page documents the major built-in Prometheus metrics families exposed by Spooky and how operators should use them.

## Use This Page With

- [Observability Operator Bundle](../operations/observability-bundle.md) for the packaged dashboards, alerts, and SLO views
- [Metrics and Alerts](../operations/metrics-and-alerts.md) for domain-level interpretation
- [Control API Reference](control-api-reference.md) when you need current runtime state rather than trend

## Endpoint

- method: `GET`
- path: configurable by `observability.metrics.path`
- default path: `/metrics`

## Read Metrics In This Order

When something is wrong, the fastest operator path is usually:

1. request and latency families
2. overload and quota families
3. backend health and timeout families
4. retry and hedge families
5. TLS and control-plane families

Metrics tell you trend and rate. The Control API tells you current runtime state.

## Canonical Label Rules

The most important low-cardinality labels are:

- `upstream`
- `backend`
- `route`
- `status_class`
- `outcome`
- `reason`
- `decision`
- `selector_dimensions`
- `backend_mode`

Operator rule:

- build dashboards and alerts from canonical label values, not from prose parsing
- keep quota, overload, auth, and generic backend failure separate

## Core Request Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_requests_total` | counter | Total requests seen by the proxy |
| `spooky_requests_success` | counter | Successful upstream responses |
| `spooky_requests_failure` | counter | Failed requests |
| `spooky_request_validation_rejects` | counter | Requests rejected by protocol validation |
| `spooky_policy_denied` | counter | Requests denied by runtime method/path policy |
| `spooky_external_auth_allowed` | counter | Requests explicitly allowed by external auth |
| `spooky_external_auth_denied` | counter | Requests denied, challenged, or redirected by external auth |
| `spooky_external_auth_timeout` | counter | External auth decisions that timed out |
| `spooky_external_auth_error` | counter | External auth transport or execution failures |
| `spooky_request_rate_limited` | counter | Requests rejected by scoped request rate limits |

## Request Breakdown Metrics

These families are the primary source for production dashboards because they preserve request totals while adding low-cardinality dimensions.

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_upstream_requests_total{upstream,status_class,outcome}` | counter | Completed requests grouped by upstream, response status class, and final outcome |
| `spooky_backend_requests_total{upstream,backend,status_class,outcome}` | counter | Completed requests grouped by upstream and selected backend |

Expected label values:

- `status_class`: `1xx`, `2xx`, `3xx`, `4xx`, `5xx`, `other`, `unknown`
- `outcome`: `success`, `failure`, `timeout`, `backend_error`, `overload_shed`, `rate_limited`

Use these for questions like:

- which upstream is producing 5xx responses?
- which backend is taking most of the failed traffic?
- are failures mostly timeouts, backend errors, or overload shedding?

These are the primary traffic families behind the packaged dashboards and recording rules.

## Latency Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_upstream_request_latency_ms_bucket{upstream,outcome,le}` | histogram bucket | End-to-end request latency grouped by upstream and final outcome |
| `spooky_upstream_request_latency_ms_sum{upstream,outcome}` | histogram sum | Sum of request latency observations in milliseconds |
| `spooky_upstream_request_latency_ms_count{upstream,outcome}` | histogram count | Count of latency observations |
| `spooky_route_latency_ms_p50{route}` | gauge | Approximate p50 route latency |
| `spooky_route_latency_ms_p95{route}` | gauge | Approximate p95 route latency |
| `spooky_route_latency_ms_p99{route}` | gauge | Approximate p99 route latency |

Practical note:

- if you only grep `spooky_requests_total` and `spooky_requests_success`, you are looking at the coarse top-level counters rather than the richer labeled families above
- for Grafana and Prometheus alerting, prefer the labeled upstream/backend metrics and the histogram family

## Early Data Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_early_data_accepted` | counter | Requests accepted in early data |
| `spooky_early_data_rejected` | counter | Requests rejected in early data |

## Health And Backend Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_health_checks_total` | counter | Active health checks executed |
| `spooky_health_checks_success` | counter | Successful active health checks |
| `spooky_health_checks_failure` | counter | Failed active health checks |
| `spooky_backend_timeouts` | counter | Backend timeout events |
| `spooky_backend_errors` | counter | Backend error events |
| `spooky_health_failures_total{reason=...}` | counter | Passive health failures by reason such as `5xx`, `timeout`, `transport`, `tls` |

## Overload And Admission Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_overload_shed` | counter | Total requests shed due to overload controls |
| `spooky_overload_shed_by_reason_total{reason=...}` | counter | Shed decisions by reason |
| `spooky_inflight_wait_admit_total{scope=...}` | counter | Successful admissions after micro-wait |
| `spooky_brownout_active` | gauge | Brownout mode active state |
| `spooky_circuit_breaker_rejected_total` | counter | Requests rejected by open circuits |

Interpretation rules:

- `spooky_overload_shed_by_reason_total` is for overload self-protection
- `spooky_request_rate_limited` is for scoped rate-limit enforcement
- neither of those is the quota contract signal

## Quota Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_quota_policy_outcomes_total{policy,decision,reason,selector_dimensions,backend_mode}` | counter | Quota outcomes grouped by matched policy, decision, canonical reason, selector dimensions, and backend mode |
| `spooky_quota_backend_health_total{backend_mode,reason}` | counter | Quota backend health and error observations |

Interpretation rules:

- `decision` answers whether quota allowed, denied, failed open, or otherwise changed request handling
- `reason` answers why the quota result happened
- `selector_dimensions` tells you which identity dimensions were part of the matched policy
- `backend_mode` tells you whether the decision came from Redis, local fallback, or another configured enforcement mode

Use these to separate:

- contract exhaustion
- quota backend degradation
- fail-open versus fail-closed behavior

## Connection And Ingress Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_active_connections` | gauge | Current active QUIC connections |
| `spooky_connection_cap_rejects` | counter | New connections rejected by active-connection cap |
| `spooky_ingress_packets_total` | counter | Total UDP packets processed |
| `spooky_ingress_queue_drops` | counter | Packets dropped due to full shard queues |
| `spooky_ingress_queue_drop_bytes` | counter | Bytes dropped due to full shard queues |
| `spooky_ingress_queue_bytes` | gauge | Bytes currently buffered in ingress shard queues |
| `spooky_ingress_bad_header_total` | counter | Packets dropped due to invalid QUIC headers |
| `spooky_ingress_rate_limited_total` | counter | Initial packets rejected by rate limiting |
| `spooky_ingress_unroutable_total` | counter | Non-initial packets for unknown connections |
| `spooky_ingress_draining_drops_total` | counter | Packets dropped while draining |
| `spooky_ingress_connection_create_failed_total` | counter | Connection creation failures |
| `spooky_ingress_version_neg_failed_total` | counter | Version-negotiation construction failures |
| `spooky_scid_rotations` | counter | SCID rotations |

## Buffer And Body-Pressure Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_request_buffered_bytes` | gauge | Bytes currently buffered in request backpressure queues |
| `spooky_request_buffered_high_watermark_bytes` | gauge | Peak buffered-request bytes since process start |
| `spooky_request_buffer_limit_rejects` | counter | Requests rejected by request-buffer caps |
| `spooky_response_prebuffer_limit_rejects` | counter | Unknown-length responses rejected by prebuffer cap |

## Retry And Hedging Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_retries_total` | counter | Total retry attempts |
| `spooky_retry_denied_total{reason=...}` | counter | Retry attempts blocked by reason |
| `spooky_retry_attempts_total{reason=...}` | counter | Retries triggered by error reason |
| `spooky_hedge_triggered_total` | counter | Hedge attempts started |
| `spooky_hedge_won_total` | counter | Hedge won the race |
| `spooky_hedge_wasted_total` | counter | Hedge lost or became unnecessary |
| `spooky_hedge_primary_won_after_trigger_total` | counter | Primary still won after hedge start |
| `spooky_hedge_primary_late_ms_total` | counter | Aggregate lateness after hedge trigger |
| `spooky_hedge_primary_late_samples_total` | counter | Late-primary observations |

## TLS Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_downstream_tls_handshake_success_total` | counter | Successful downstream TLS handshakes |
| `spooky_downstream_tls_handshake_failure_total{listener,reason}` | counter | Downstream TLS handshake failures |
| `spooky_downstream_tls_certificate_selection_total{listener,selection}` | counter | Certificate-selection outcomes |
| `spooky_downstream_tls_alpn_total{listener,protocol}` | counter | Negotiated ALPN protocols |
| `spooky_downstream_tls_certificate_not_after_seconds{listener,server_name}` | gauge | Certificate expiration timestamp |
| `spooky_downstream_tls_certificate_days_remaining{listener,server_name}` | gauge | Estimated remaining days to expiration |
| `spooky_upstream_tls_failure_total{backend,phase,reason}` | counter | Upstream TLS failures |

## DNS And Backend Refresh Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_backend_dns_refresh_success_total` | counter | Successful backend DNS refreshes |
| `spooky_backend_dns_refresh_failure_total` | counter | Failed backend DNS refreshes |
| `spooky_backend_dns_address_set_changes_total` | counter | Refreshes that changed address set |
| `spooky_backend_client_rotations_total` | counter | Backend client rotations caused by DNS changes |
| `spooky_backend_dns_last_refresh_success_seconds` | gauge | Unix timestamp of last successful refresh |

## JWT And JWKS Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_jwt_validation_failures_total{reason}` | counter | JWT validation failures by stable rejection reason |
| `spooky_jwt_algorithm_rejections_total{algorithm}` | counter | Tokens rejected because their `alg` is not in the allowlist |
| `spooky_jwks_unknown_kid_total{jwks_source_id}` | counter | Tokens presenting a `kid` absent from the cached key set |
| `spooky_jwks_refresh_success_total{jwks_source_id}` | counter | Successful JWKS refreshes |
| `spooky_jwks_refresh_failure_total{jwks_source_id}` | counter | Failed JWKS refreshes |
| `spooky_jwks_age_seconds{jwks_source_id}` | gauge | Age of the active key set since last successful refresh |
| `spooky_jwks_state{jwks_source_id,state}` | gauge | Current cache state, `1` on the active state series |
| `spooky_jwks_active_keys{jwks_source_id}` | gauge | Usable verification keys currently loaded |
| `spooky_jwks_last_refresh_attempt_seconds{jwks_source_id}` | gauge | Unix timestamp of the last refresh attempt |
| `spooky_jwks_last_refresh_success_seconds{jwks_source_id}` | gauge | Unix timestamp of the last successful refresh |

JWKS series are labelled by `jwks_source_id`, an opaque per-source identifier —
the configured endpoint URL is never used as a label value, so query strings and
embedded credentials cannot leak into the metrics endpoint. Map a source id back
to its endpoint through `jwks.sources[]` in the `/admin/runtime` snapshot, which
reports both `jwks_source_id` and a sanitized `jwks_endpoint`.

`spooky_jwks_state` reports one of `never_fetched`, `fresh`, `stale`,
`refresh_failed_retained`, `quarantined_retained`, or `empty_unusable`. Only the
last is unable to validate tokens; the two `*_retained` states are still serving
last-known-good keys.

## Control Plane And Runtime Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_runtime_validation_attempts_total` | counter | Staged validation requests accepted by the control plane |
| `spooky_runtime_preview_attempts_total` | counter | Staged preview requests accepted by the control plane |
| `spooky_runtime_activation_total{result,reason}` | counter | Runtime activation outcomes grouped by result and canonical reason |
| `spooky_runtime_rollback_total{result,reason}` | counter | Runtime rollback outcomes grouped by result and canonical reason |
| `spooky_runtime_rejections_total{reason}` | counter | Runtime activation or rollback rejections grouped by canonical operator reason |
| `spooky_runtime_active_generation` | gauge | Current active runtime generation identifier |
| `spooky_runtime_history_depth` | gauge | Number of retained runtime history entries visible to the active generation |
| `spooky_control_api_connection_limit_drops` | counter | Control API connections dropped by limiter |
| `spooky_watchdog_restart_requests` | counter | Watchdog restart requests |
| `spooky_watchdog_restart_hooks` | counter | Restart hooks executed |
| `spooky_watchdog_degraded_windows` | counter | Degraded watchdog windows |
| `spooky_runtime_panics` | counter | Observed runtime panics |

Use these when:

- activation, rollback, or restart workflows are misbehaving
- the active generation may differ across nodes
- watchdog activity might be driving drain or restart behavior

## Golden Signals To Watch First

- request success/failure counters
- request totals by upstream and backend outcome
- upstream request latency histogram percentiles from PromQL
- route latency percentiles
- overload shed counts by reason
- backend timeout and backend error counters
- active connections
- request buffered bytes
- downstream handshake failures
- quota policy outcomes and quota backend health when 429s or fail-open or fail-closed behavior rises
- runtime activation, runtime rollback, and runtime rejection families when control-plane workflows are involved

## First Alerts To Add

- `sum by (upstream) (rate(spooky_upstream_requests_total{status_class="5xx"}[5m]))`
- `sum by (backend) (rate(spooky_backend_requests_total{outcome="backend_error"}[5m]))`
- `histogram_quantile(0.95, sum by (le, upstream) (rate(spooky_upstream_request_latency_ms_bucket[5m])))`
- sustained growth in `spooky_overload_shed_by_reason_total`
- rising `spooky_backend_timeouts`
- rising `spooky_downstream_tls_handshake_failure_total`
- unexpectedly high `spooky_request_buffered_bytes`
- any sustained `spooky_runtime_panics`

## Metrics To Control API Workflow

Use this fast path when the metrics tell you the class of problem but not the live state:

| Metric family | Next control-plane read |
|---|---|
| `spooky_quota_policy_outcomes_total` | `GET /admin/runtime` |
| `spooky_quota_backend_health_total` | `GET /admin/runtime` |
| `spooky_runtime_activation_total` | `GET /admin/runtime/history` |
| `spooky_runtime_rollback_total` | `GET /admin/runtime/history` |
| `spooky_runtime_rejections_total` | `GET /admin/runtime/history` |
| `spooky_watchdog_*` or `spooky_runtime_panics` | `GET /admin/runtime` |
| `spooky_downstream_tls_*` | `GET /admin/runtime` |

## Related Pages

- [Control API Reference](control-api-reference.md)
- [Observability Operator Bundle](../operations/observability-bundle.md)
- [Metrics and Alerts](../operations/metrics-and-alerts.md)
- [Operations Runbook](../operations/runbook.md)
