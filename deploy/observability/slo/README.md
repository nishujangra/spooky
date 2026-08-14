# Spooky SLO Package

This directory defines the operator SLO contract for Spooky before any Grafana
dashboard packaging.

The goal is to make the SLO math explicit, stable, and reviewable:

- what population each SLO measures
- which requests count against the objective
- which operational events are intentionally excluded
- which packaged recording rules should be used for dashboards and reports

Use the PromQL definitions in [definitions.promql](definitions.promql) for
operator views. Those queries are built on the packaged recording rules in
[`../prometheus/recording-rules.yaml`](../prometheus/recording-rules.yaml), not
directly on raw scrape-time PromQL.

## Contract Rules

- Use packaged `spooky:slo_*` recording rules for SLO reporting.
- Do not redefine the numerator or denominator ad hoc in dashboards.
- Keep overload, quota, auth, and backend-failure concepts separate.
- Treat denominator changes as SLO contract changes, not dashboard tweaks.

## SLO Inventory

The first packaged operator SLOs are:

- availability
- p50 latency
- p95 latency
- p99 latency
- overload shed rate
- backend timeout rate
- auth failure rate

Quota denial rate is also defined as an operator view because it must remain
explicitly separate from overload and availability, even though it is not part
of the primary availability SLO.

## Shared Request Population

The default denominator for traffic-rate SLOs is:

- `sum without (upstream, status_class, outcome) (rate(spooky_upstream_requests_total[window]))`

Meaning:

- every completed request observed by Spooky counts in the population
- status class and terminal outcome stay available in the raw metric, but the
  packaged SLO denominator intentionally collapses them
- 4xx responses remain in the denominator but do not count as availability
  failures unless another SLO explicitly says they do

This denominator is exposed through:

- `spooky:slo_requests:rate30m`

## Availability SLO

### Objective

The packaged page-alert thresholds assume a 99.9% availability objective unless
your operators override them.

### Denominator

- all completed requests in `spooky_upstream_requests_total`

### Failure numerator

- `sum without (upstream, status_class, outcome) (rate(spooky_upstream_requests_total{status_class="5xx"}[window]))`

### What counts as an availability failure

- upstream responses that land in HTTP `5xx`
- overload-generated `503` responses
- backend timeout paths that surface as `5xx`
- backend transport, protocol, bridge, or TLS failures that surface as `5xx`

### What does not count as an availability failure

- client `4xx`
- auth denials
- quota denials
- scoped rate-limit denials

### Why

Availability here is the coarse user-visible server-failure contract. Auth and
quota failures are tracked separately because they are policy-contract outcomes,
not edge availability failures.

### Packaged reporting series

- `spooky:slo_request_server_error_ratio:rate30m`
- `spooky:slo_availability_ratio:30m`

## Latency SLOs

### Objective population

Latency SLOs measure only successful requests.

### Denominator population

- `spooky_upstream_request_latency_ms_bucket{outcome="success"}`

### What is included

- requests whose terminal outcome is `success`

### What is excluded

- timeouts
- overload shed responses
- rate-limited responses
- auth denials
- quota denials
- backend-error terminal outcomes

### Why

A latency percentile should describe the latency delivered for successful
service, not mix successful and rejected work into one percentile.

### Packaged reporting series

- `spooky:slo_request_latency_ms:p50_30m`
- `spooky:slo_request_latency_ms:p95_30m`
- `spooky:slo_request_latency_ms:p99_30m`

## Overload Shed Rate SLO

### Denominator

- all completed requests in `spooky_upstream_requests_total`

### Numerator

- `sum without (reason) (rate(spooky_overload_shed_by_reason_total[window]))`

### What counts as overload

Only canonical overload-control decisions, including reasons such as:

- `brownout`
- `adaptive_admission`
- `route_cap`
- `route_global_cap`
- `global_inflight`
- `upstream_inflight`
- `backend_inflight`
- `circuit_open`
- `request_buffer_cap`
- `response_prebuffer_cap`
- `connection_cap`

### What does not count as overload

- quota denials
- scoped request rate-limit denials
- auth denials

### Why

Overload is a capacity-protection signal. Quota and auth are policy-contract
signals and must stay separate in operator reporting.

### Packaged reporting series

- `spooky:slo_overload_shed_ratio:30m`

## Backend Timeout Rate SLO

### Denominator

- all completed requests in `spooky_upstream_requests_total`

### Numerator

- `rate(spooky_backend_timeouts[window])`

### What counts

- backend timeout events emitted by the forwarding path

### What does not count

- generic backend errors that were not timeout-classified
- overload shed
- quota denials
- auth denials

### Why

Timeouts are a leading reliability signal distinct from broader backend-error
buckets.

### Packaged reporting series

- `spooky:slo_backend_timeout_ratio:30m`

## Auth Failure Rate SLO

### Denominator

- all completed requests in `spooky_upstream_requests_total`

### Numerator

- `rate(spooky_external_auth_denied[window])`
- `sum without (reason) (rate(spooky_jwt_validation_failures_total[window]))`

These are added together in the packaged recording rule.

### What counts as an auth failure

- explicit external-auth denials
- local JWT validation failures

### What does not count as an auth failure

- generic `spooky_policy_denied`
- quota denials
- overload shed
- external-auth transport errors or timeouts when the request was failed open

### Why

`spooky_policy_denied` is intentionally excluded because it mixes non-auth path
or method policy with auth-adjacent behavior. The auth SLO is scoped only to
explicit authentication and authorization contract failures that already have
their own stable metric families.

### Packaged reporting series

- `spooky:slo_auth_denial_ratio:30m`

## Quota Denial Operator View

Quota is not folded into overload or availability reporting.

### Denominator

- all completed requests in `spooky_upstream_requests_total`

### Numerator

- `sum without (policy, reason, selector_dimensions, backend_mode, decision) (rate(spooky_quota_policy_outcomes_total{decision="denied"}[window]))`

### What counts as a quota denial

- `decision="denied"` in `spooky_quota_policy_outcomes_total`

### What does not count as a quota denial

- `decision="failed_open"`
- `decision="failed_closed"`
- overload shed
- scoped request rate-limited events

### Why

Quota enforcement is a contract outcome. Backend degradation in the quota store
is tracked separately through quota backend health and should not be mislabeled
as ordinary overload.

### Packaged reporting series

- `spooky:slo_quota_denial_ratio:30m`

## Backend Failure Interpretation Boundaries

`backend timeout` and `backend failure` are not interchangeable.

For this package:

- backend timeout SLO uses `spooky_backend_timeouts`
- broader backend-error behavior uses
  `spooky:backend_errors_by_upstream_backend:rate5m`
- upstream `5xx` availability failures may include backend error classes, but
  the SLO package still keeps timeout and availability views separate

If you need a broader backend-failure SLO later, define it explicitly rather
than silently widening the timeout numerator.

## Reporting Windows

The packaged SLO reporting window is currently:

- `30m` for operator reporting and alert support

The faster `5m` recording rules remain available for burn-rate and incident
views, but they are not the canonical reporting window for these SLO ratios.

## Related Files

- [definitions.promql](definitions.promql)
- [../prometheus/recording-rules.yaml](../prometheus/recording-rules.yaml)
- [../prometheus/alerts.yaml](../prometheus/alerts.yaml)
