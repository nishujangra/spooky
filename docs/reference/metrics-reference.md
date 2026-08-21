# Metrics Reference

Use this page to look up exported metric families, labels, and meanings.

## Quick Lookup

| If you need to check | Start here |
| --- | --- |
| request volume, outcomes, and status classes | [Core Request Metrics](#core-request-metrics) and [Request Breakdown Metrics](#request-breakdown-metrics) |
| latency families | [Latency Metrics](#latency-metrics) |
| overload, brownout, and circuit protection | [Overload And Admission Metrics](#overload-and-admission-metrics) |
| quota decisions and backend health | [Quota Metrics](#quota-metrics) |
| retries and hedges | [Retry And Hedging Metrics](#retry-and-hedging-metrics) |
| downstream or upstream TLS | [TLS Metrics](#tls-metrics) |
| control-plane or runtime generation signals | [Control Plane And Runtime Metrics](#control-plane-and-runtime-metrics) |

## Endpoint

- method: `GET`
- path: configurable by `observability.metrics.path`
- default path: `/metrics`

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
| `impulse_requests_total` | counter | Total requests seen by the proxy |
| `impulse_requests_success` | counter | Successful upstream responses |
| `impulse_requests_failure` | counter | Failed requests |
| `impulse_request_validation_rejects` | counter | Requests rejected by protocol validation |
| `impulse_policy_denied` | counter | Requests denied by runtime method/path policy |
| `impulse_external_auth_allowed` | counter | Requests explicitly allowed by external auth |
| `impulse_external_auth_denied` | counter | Requests denied, challenged, or redirected by external auth |
| `impulse_external_auth_timeout` | counter | External auth decisions that timed out |
| `impulse_external_auth_error` | counter | External auth transport or execution failures |
| `impulse_request_rate_limited` | counter | Requests rejected by scoped request rate limits |

## Request Breakdown Metrics

These families are the primary source for production dashboards because they preserve request totals while adding low-cardinality dimensions.

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_upstream_requests_total{upstream,status_class,outcome}` | counter | Completed requests grouped by upstream, response status class, and final outcome |
| `impulse_backend_requests_total{upstream,backend,status_class,outcome}` | counter | Completed requests grouped by upstream and selected backend |

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
| `impulse_upstream_request_latency_ms_bucket{upstream,outcome,le}` | histogram bucket | End-to-end request latency grouped by upstream and final outcome |
| `impulse_upstream_request_latency_ms_sum{upstream,outcome}` | histogram sum | Sum of request latency observations in milliseconds |
| `impulse_upstream_request_latency_ms_count{upstream,outcome}` | histogram count | Count of latency observations |
| `impulse_route_latency_ms_p50{route}` | gauge | Approximate p50 route latency |
| `impulse_route_latency_ms_p95{route}` | gauge | Approximate p95 route latency |
| `impulse_route_latency_ms_p99{route}` | gauge | Approximate p99 route latency |

Practical note:

- if you only grep `impulse_requests_total` and `impulse_requests_success`, you are looking at the coarse top-level counters rather than the richer labeled families above
- for Grafana and Prometheus alerting, prefer the labeled upstream/backend metrics and the histogram family

## Early Data Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_early_data_accepted` | counter | Requests accepted in early data |
| `impulse_early_data_rejected` | counter | Requests rejected in early data |

## Health And Backend Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_health_checks_total` | counter | Active health checks executed |
| `impulse_health_checks_success` | counter | Successful active health checks |
| `impulse_health_checks_failure` | counter | Failed active health checks |
| `impulse_backend_timeouts` | counter | Backend timeout events |
| `impulse_backend_errors` | counter | Backend error events |
| `impulse_health_failures_total{reason=...}` | counter | Passive health failures by reason such as `5xx`, `timeout`, `transport`, `tls` |

## Overload And Admission Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_overload_shed` | counter | Total requests shed due to overload controls |
| `impulse_overload_shed_by_reason_total{reason=...}` | counter | Shed decisions by reason |
| `impulse_inflight_wait_admit_total{scope=...}` | counter | Successful admissions after micro-wait |
| `impulse_brownout_active` | gauge | Brownout mode active state |
| `impulse_circuit_breaker_rejected_total` | counter | Requests rejected by open circuits |

Interpretation rules:

- `impulse_overload_shed_by_reason_total` is for overload self-protection
- `impulse_request_rate_limited` is for scoped rate-limit enforcement
- neither of those is the quota contract signal

## Quota Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_quota_policy_outcomes_total{policy,decision,reason,selector_dimensions,backend_mode}` | counter | Quota outcomes grouped by matched policy, decision, canonical reason, selector dimensions, and backend mode |
| `impulse_quota_backend_health_total{backend_mode,reason}` | counter | Quota backend health and error observations |

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
| `impulse_active_connections` | gauge | Current active QUIC connections |
| `impulse_connection_cap_rejects` | counter | New connections rejected by active-connection cap |
| `impulse_ingress_packets_total` | counter | Total UDP packets processed |
| `impulse_ingress_queue_drops` | counter | Packets dropped due to full shard queues |
| `impulse_ingress_queue_drop_bytes` | counter | Bytes dropped due to full shard queues |
| `impulse_ingress_queue_bytes` | gauge | Bytes currently buffered in ingress shard queues |
| `impulse_ingress_bad_header_total` | counter | Packets dropped due to invalid QUIC headers |
| `impulse_ingress_rate_limited_total` | counter | Initial packets rejected by rate limiting |
| `impulse_ingress_unroutable_total` | counter | Non-initial packets for unknown connections |
| `impulse_ingress_draining_drops_total` | counter | Packets dropped while draining |
| `impulse_ingress_connection_create_failed_total` | counter | Connection creation failures |
| `impulse_ingress_version_neg_failed_total` | counter | Version-negotiation construction failures |
| `impulse_scid_rotations` | counter | SCID rotations |

## Buffer And Body-Pressure Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_request_buffered_bytes` | gauge | Bytes currently buffered in request backpressure queues |
| `impulse_request_buffered_high_watermark_bytes` | gauge | Peak buffered-request bytes since process start |
| `impulse_request_buffer_limit_rejects` | counter | Requests rejected by request-buffer caps |
| `impulse_response_prebuffer_limit_rejects` | counter | Unknown-length responses rejected by prebuffer cap |

## Retry And Hedging Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_retries_total` | counter | Total retry attempts |
| `impulse_retry_denied_total{reason=...}` | counter | Retry attempts blocked by reason |
| `impulse_retry_attempts_total{reason=...}` | counter | Retries triggered by error reason |
| `impulse_hedge_triggered_total` | counter | Hedge attempts started |
| `impulse_hedge_won_total` | counter | Hedge won the race |
| `impulse_hedge_wasted_total` | counter | Hedge lost or became unnecessary |
| `impulse_hedge_primary_won_after_trigger_total` | counter | Primary still won after hedge start |
| `impulse_hedge_primary_late_ms_total` | counter | Aggregate lateness after hedge trigger |
| `impulse_hedge_primary_late_samples_total` | counter | Late-primary observations |

## TLS Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_downstream_tls_handshake_success_total` | counter | Successful downstream TLS handshakes |
| `impulse_downstream_tls_handshake_failure_total{listener,reason}` | counter | Downstream TLS handshake failures |
| `impulse_downstream_tls_certificate_selection_total{listener,selection}` | counter | Certificate-selection outcomes |
| `impulse_downstream_tls_alpn_total{listener,protocol}` | counter | Negotiated ALPN protocols |
| `impulse_downstream_tls_certificate_not_after_seconds{listener,server_name}` | gauge | Certificate expiration timestamp |
| `impulse_downstream_tls_certificate_days_remaining{listener,server_name}` | gauge | Estimated remaining days to expiration |
| `impulse_upstream_tls_failure_total{backend,phase,reason}` | counter | Upstream TLS failures |

## DNS And Backend Refresh Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_backend_dns_refresh_success_total` | counter | Successful backend DNS refreshes |
| `impulse_backend_dns_refresh_failure_total` | counter | Failed backend DNS refreshes |
| `impulse_backend_dns_address_set_changes_total` | counter | Refreshes that changed address set |
| `impulse_backend_client_rotations_total` | counter | Backend client rotations caused by DNS changes |
| `impulse_backend_dns_last_refresh_success_seconds` | gauge | Unix timestamp of last successful refresh |

## JWT And JWKS Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_jwt_validation_failures_total{reason}` | counter | JWT validation failures by stable rejection reason |
| `impulse_jwt_algorithm_rejections_total{algorithm}` | counter | Tokens rejected because their `alg` is not in the allowlist |
| `impulse_jwks_unknown_kid_total{jwks_source_id}` | counter | Tokens presenting a `kid` absent from the cached key set |
| `impulse_jwks_refresh_success_total{jwks_source_id}` | counter | Successful JWKS refreshes |
| `impulse_jwks_refresh_failure_total{jwks_source_id}` | counter | Failed JWKS refreshes |
| `impulse_jwks_age_seconds{jwks_source_id}` | gauge | Age of the active key set since last successful refresh |
| `impulse_jwks_state{jwks_source_id,state}` | gauge | Current cache state, `1` on the active state series |
| `impulse_jwks_active_keys{jwks_source_id}` | gauge | Usable verification keys currently loaded |
| `impulse_jwks_last_refresh_attempt_seconds{jwks_source_id}` | gauge | Unix timestamp of the last refresh attempt |
| `impulse_jwks_last_refresh_success_seconds{jwks_source_id}` | gauge | Unix timestamp of the last successful refresh |

JWKS series are labelled by `jwks_source_id`, an opaque per-source identifier —
the configured endpoint URL is never used as a label value, so query strings and
embedded credentials cannot leak into the metrics endpoint. Map a source id back
to its endpoint through `jwks.sources[]` in the `/admin/runtime` snapshot, which
reports both `jwks_source_id` and a sanitized `jwks_endpoint`.

`impulse_jwks_state` reports one of `never_fetched`, `fresh`, `stale`,
`refresh_failed_retained`, `quarantined_retained`, or `empty_unusable`. Only the
last is unable to validate tokens; the two `*_retained` states are still serving
last-known-good keys.

## Control Plane And Runtime Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `impulse_runtime_validation_attempts_total` | counter | Staged validation requests accepted by the control plane |
| `impulse_runtime_preview_attempts_total` | counter | Staged preview requests accepted by the control plane |
| `impulse_runtime_activation_total{result,reason}` | counter | Runtime activation outcomes grouped by result and canonical reason |
| `impulse_runtime_rollback_total{result,reason}` | counter | Runtime rollback outcomes grouped by result and canonical reason |
| `impulse_runtime_rejections_total{reason}` | counter | Runtime activation or rollback rejections grouped by canonical operator reason |
| `impulse_runtime_active_generation` | gauge | Current active runtime generation identifier |
| `impulse_runtime_history_depth` | gauge | Number of retained runtime history entries visible to the active generation |
| `impulse_control_api_connection_limit_drops` | counter | Control API connections dropped by limiter |
| `impulse_watchdog_restart_requests` | counter | Watchdog restart requests |
| `impulse_watchdog_restart_hooks` | counter | Restart hooks executed |
| `impulse_watchdog_degraded_windows` | counter | Degraded watchdog windows |
| `impulse_runtime_panics` | counter | Observed runtime panics |

Use these when:

- activation, rollback, or restart workflows are misbehaving
- the active generation may differ across nodes
- watchdog activity might be driving drain or restart behavior

## Metrics To Control API Workflow

Use this fast path when the metrics tell you the class of problem but not the live state:

| Metric family | Next control-plane read |
|---|---|
| `impulse_quota_policy_outcomes_total` | `GET /admin/runtime` |
| `impulse_quota_backend_health_total` | `GET /admin/runtime` |
| `impulse_runtime_activation_total` | `GET /admin/runtime/history` |
| `impulse_runtime_rollback_total` | `GET /admin/runtime/history` |
| `impulse_runtime_rejections_total` | `GET /admin/runtime/history` |
| `impulse_watchdog_*` or `impulse_runtime_panics` | `GET /admin/runtime` |
| `impulse_downstream_tls_*` | `GET /admin/runtime` |

## Related Pages

- [Observability Operator Bundle](../operations/observability-bundle.md)
- [Metrics and Alerts](../operations/metrics-and-alerts.md)
- [Control API Reference](control-api-reference.md)
- [Operations Runbook](../operations/runbook.md)
