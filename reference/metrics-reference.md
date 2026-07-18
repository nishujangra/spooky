# Metrics Reference

This page documents the major built-in Prometheus metrics families currently exposed by Spooky.

## Endpoint

- method: `GET`
- path: configurable by `observability.metrics.path`
- default path: `/metrics`

## Core Request Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_requests_total` | counter | Total requests seen by the proxy |
| `spooky_requests_success` | counter | Successful upstream responses |
| `spooky_requests_failure` | counter | Failed requests |
| `spooky_request_validation_rejects` | counter | Requests rejected by protocol validation |
| `spooky_policy_denied` | counter | Requests denied by runtime method/path policy |

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

## Control Plane And Runtime Metrics

| Metric | Type | Meaning |
| --- | --- | --- |
| `spooky_control_api_connection_limit_drops` | counter | Control API connections dropped by limiter |
| `spooky_watchdog_restart_requests` | counter | Watchdog restart requests |
| `spooky_watchdog_restart_hooks` | counter | Restart hooks executed |
| `spooky_watchdog_degraded_windows` | counter | Degraded watchdog windows |
| `spooky_runtime_panics` | counter | Observed runtime panics |

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

## First Alerts To Add

- `sum by (upstream) (rate(spooky_upstream_requests_total{status_class="5xx"}[5m]))`
- `sum by (backend) (rate(spooky_backend_requests_total{outcome="backend_error"}[5m]))`
- `histogram_quantile(0.95, sum by (le, upstream) (rate(spooky_upstream_request_latency_ms_bucket[5m])))`
- sustained growth in `spooky_overload_shed_by_reason_total`
- rising `spooky_backend_timeouts`
- rising `spooky_downstream_tls_handshake_failure_total`
- unexpectedly high `spooky_request_buffered_bytes`
- any sustained `spooky_runtime_panics`

## Related Pages

- [Control API Reference](control-api-reference.md)
- [Operations Runbook](../operations/runbook.md)
