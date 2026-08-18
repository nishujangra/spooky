# Troubleshooting and Operator Diagnosis

Use this page when Spooky is running but something is wrong in startup, traffic handling, backend behavior, TLS, the control plane, or observability.

This is an operator guide, not a source-code debugging guide. Each section starts from a production symptom and gives the fastest path to confirm the cause and fix it.

## Start Here

Use the same triage order for most incidents:

1. check `GET /health` and `GET /ready`
2. inspect `GET /admin/runtime` for active generation, backend health, quota backend health, and watchdog state
3. inspect `GET /metrics` for trend and rate
4. inspect runtime logs and control-plane audit events for the request, actor, or generation involved

Canonical references for the checks above:

- exact admin endpoints and curl flows: [Control API Reference](../reference/control-api-reference.md)
- exact metric names and labels: [Metrics Reference](../reference/metrics-reference.md)
- dashboard and alert interpretation: [Observability Operator Bundle](../operations/observability-bundle.md)
- rollout, drain, and rollback workflow: [Runbook](../operations/runbook.md) and [Reload and Drain](../operations/reload-and-drain.md)

## Symptom Index

| Symptom | First checks |
| --- | --- |
| process starts but never becomes ready | `/health`, `/ready`, startup logs, `/admin/runtime` |
| requests reach Spooky but do not reach the expected upstream | route config, `/admin/runtime`, request logs, upstream/backend request metrics |
| clients receive `429` or other contract-style denials | quota metrics, scoped rate-limit metrics, policy config in `/admin/runtime` |
| clients receive `503` during pressure | overload metrics, brownout state, backend timeout pressure, watchdog state |
| backend failures or timeouts are rising | backend metrics, active health results, retry metrics, backend inventory in `/admin/runtime` |
| all backends look unhealthy | health-check config, backend reachability, backend health summary, passive failure metrics |
| TLS handshakes are failing | TLS metrics, certificate state, listener config, client handshake logs |
| control API calls fail or runtime changes do not apply | control-plane auth, audit events, runtime history, activation metrics |
| Grafana or Prometheus shows missing or empty data | `/metrics`, scrape path, target state, dashboard queries, traffic generation |

## Process Starts but Never Becomes Ready

### Symptom

- the process is up, but `GET /ready` does not return ready
- the control plane reports degraded startup state
- listeners are not accepting traffic after boot

### Likely Causes

- invalid or incomplete configuration
- listener bind failure
- TLS file or certificate loading failure
- runtime generation could not activate cleanly
- required control-plane service failed during startup

### Verification Steps

1. check `GET /health` and `GET /ready`
2. inspect startup logs for configuration, bind, TLS, or runtime-activation failures
3. inspect `GET /admin/runtime` for:
   - `current_generation`
   - backend health summary
   - quota backend health summary
   - watchdog state
4. inspect `GET /admin/runtime/history` for the most recent activation or rollback result
5. if a staged change was just applied, run `POST /admin/runtime/validate` or `POST /admin/runtime/preview` again with the same candidate

### Fix Steps

1. correct the invalid config field, missing file path, or bind conflict
2. validate the candidate config before activating it
3. confirm certificate and key files are readable by the Spooky process
4. activate the corrected generation or roll back to the last healthy generation
5. confirm `/ready` becomes healthy before restoring traffic

### Relevant Logs, Metrics, and Endpoints

- endpoints: `/health`, `/ready`, `/admin/runtime`, `/admin/runtime/history`
- metrics: `spooky_runtime_activation_total`, `spooky_runtime_rejections_total`, `spooky_runtime_active_generation`, `spooky_runtime_history_depth`
- logs: startup logs, bind failures, TLS load failures, runtime activation failures
- audit: validate, preview, activate, rollback attempts and outcomes

## Requests Reach Spooky but Not the Expected Upstream

### Symptom

- traffic reaches the listener, but the selected route or upstream is not the one you expected
- clients receive failures that point to missing routing or backend selection
- one upstream shows no traffic even though clients are sending requests

### Likely Causes

- route matchers do not match the request host, path, or method as intended
- a more specific or earlier route is taking precedence
- the upstream exists in config, but the active generation is not the one you expect
- the upstream has no usable backends

### Verification Steps

1. inspect the request shape actually sent by the client:
   - host
   - path
   - method
2. inspect `GET /admin/runtime` and confirm the active generation contains the expected route and upstream
3. inspect `GET /admin/runtime/history` if a recent activation may have changed route precedence
4. inspect request logs for route resolution, selected upstream, and selected backend
5. inspect:
   - `spooky_upstream_requests_total`
   - `spooky_backend_requests_total`
   - `spooky_policy_denied`

### Fix Steps

1. make the route matcher explicit and unambiguous
2. ensure the intended route is more specific than fallback routes
3. activate the corrected generation
4. verify the expected upstream starts receiving request volume

### Relevant Logs, Metrics, and Endpoints

- endpoints: `/admin/runtime`, `/admin/runtime/history`
- metrics: `spooky_upstream_requests_total`, `spooky_backend_requests_total`, `spooky_route_latency_ms_p50`, `spooky_route_latency_ms_p95`, `spooky_route_latency_ms_p99`
- logs: route-resolution logs, selected upstream/backend logs, policy-denial logs

## Clients Receive `429` or Other Contract-Style Denials

### Symptom

- clients receive contract-style rejections during otherwise healthy backend conditions
- denial volume rises without a matching rise in overload shedding
- operators need to know whether the cause is scoped rate limiting or quota

### Likely Causes

- scoped request rate limits are configured and firing
- quota policy burst or sustained windows are exhausted
- quota backend is degraded and the configured failure policy is affecting decisions
- a selector is matching a broader set of traffic than intended

### Verification Steps

1. inspect:
   - `spooky_request_rate_limited`
   - `spooky_quota_policy_outcomes_total`
   - `spooky_quota_backend_health_total`
2. separate the decision types:
   - scoped rate limiting uses `spooky_request_rate_limited`
   - quota decisions use `spooky_quota_policy_outcomes_total`
   - overload uses `spooky_overload_shed_by_reason_total`
3. inspect `GET /admin/runtime` for configured quota policies, backend mode, and quota backend health summary
4. inspect request logs for matched policy, selector dimensions, decision, and reason
5. confirm whether the denial reason is expected for the caller:
   - route
   - tenant
   - token
   - client

### Fix Steps

1. adjust the policy scope if the selector is too broad
2. adjust burst or sustained limits if the contract is too small for the intended traffic pattern
3. correct caller identity extraction if the wrong tenant, token, or client key is being used
4. restore quota backend availability if the issue is backend degradation
5. keep quota and overload actions separate in analysis and remediation

### Relevant Logs, Metrics, and Endpoints

- endpoints: `/admin/runtime`
- metrics: `spooky_request_rate_limited`, `spooky_quota_policy_outcomes_total`, `spooky_quota_backend_health_total`
- logs: admission logs with matched quota policy, decision, deny reason, selector dimensions, backend mode

## Clients Receive `503` During Pressure

### Symptom

- `503` responses rise during load or backend stress
- dashboard panels show overload shedding or brownout activity
- operators need to confirm whether the failure is overload protection or backend failure

### Likely Causes

- adaptive admission is shedding requests
- circuit-open protection is rejecting traffic
- backend timeout pressure is forcing overload controls
- brownout mode is active because the system is protecting itself

### Verification Steps

1. inspect:
   - `spooky_overload_shed`
   - `spooky_overload_shed_by_reason_total`
   - `spooky_brownout_active`
   - `spooky_backend_timeouts`
   - `spooky_circuit_breaker_rejected_total`
2. inspect `GET /admin/runtime` for current backend and watchdog summaries
3. confirm whether `503` is paired with rising backend timeout and error pressure
4. inspect logs for overload reason, circuit-open decisions, and adaptive admission behavior
5. compare with quota metrics to confirm this is not a contract failure being misread as overload

### Fix Steps

1. reduce backend latency or error pressure first
2. restore unhealthy backends before increasing admitted load
3. scale backend capacity or reduce concurrency pressure upstream
4. review overload thresholds only after verifying the backend path is healthy
5. confirm brownout clears after the pressure event ends

### Relevant Logs, Metrics, and Endpoints

- endpoints: `/admin/runtime`
- metrics: `spooky_overload_shed`, `spooky_overload_shed_by_reason_total`, `spooky_brownout_active`, `spooky_circuit_breaker_rejected_total`, `spooky_backend_timeouts`, `spooky_backend_errors`
- logs: overload decisions, brownout transitions, circuit-open rejects, backend timeout spikes

## Backend Failures or Timeouts Are Rising

### Symptom

- requests fail with timeout, transport, or backend-error outcomes
- retries or hedges increase
- one upstream or backend is clearly worse than the rest

### Likely Causes

- backend application latency is rising
- backend transport or TLS failures are occurring
- a bad deployment has made one backend unhealthy
- retries are being consumed by repeated timeout or transport failures
- DNS or address rotation changed the backend set

### Verification Steps

1. inspect:
   - `spooky_backend_timeouts`
   - `spooky_backend_errors`
   - `spooky_backend_requests_total`
   - `spooky_upstream_requests_total`
   - `spooky_retry_attempts_total`
   - `spooky_retry_denied_total`
   - `spooky_hedge_triggered_total`
2. inspect passive health reasons in `spooky_health_failures_total`
3. inspect DNS and rotation metrics if failures started after a backend-address change:
   - `spooky_backend_dns_refresh_success_total`
   - `spooky_backend_dns_refresh_failure_total`
   - `spooky_backend_dns_address_set_changes_total`
   - `spooky_backend_client_rotations_total`
4. inspect `GET /admin/runtime` for backend health summary and backend inventory
5. compare backend-specific failures instead of looking only at total request failure rate

### Fix Steps

1. remove or drain the failing backend if one backend is responsible for most failures
2. fix backend latency or transport health before increasing retry aggressiveness
3. correct DNS or service-discovery issues if the backend set changed unexpectedly
4. verify upstream TLS settings if failures are isolated to secure backends
5. confirm success and latency normalize after the backend recovers

### Relevant Logs, Metrics, and Endpoints

- endpoints: `/admin/runtime`
- metrics: `spooky_backend_timeouts`, `spooky_backend_errors`, `spooky_backend_requests_total`, `spooky_upstream_requests_total`, `spooky_retry_attempts_total`, `spooky_retry_denied_total`, `spooky_hedge_triggered_total`, `spooky_hedge_won_total`, `spooky_hedge_wasted_total`, `spooky_health_failures_total`
- logs: backend transport failures, timeout errors, retry reasons, hedge activity, DNS refresh and rotation logs

## All Backends Look Unhealthy

### Symptom

- one or more upstreams have no healthy backends
- traffic fails even though the listener is healthy
- health checks and passive health signals both show sustained failures

### Likely Causes

- active health checks are failing
- backend health endpoint is wrong or too strict
- timeout thresholds are too aggressive for the workload
- the backend application is down or unreachable
- all backends entered failure or cooldown state together

### Verification Steps

1. inspect `GET /admin/runtime` for backend health summary
2. inspect:
   - `spooky_health_checks_total`
   - `spooky_health_checks_success`
   - `spooky_health_checks_failure`
   - `spooky_health_failures_total`
3. confirm the configured backend health endpoint and timeout match the real backend behavior
4. test the backend health endpoint directly from the same network where Spooky runs
5. inspect logs for health transitions and repeated passive-failure reasons

### Fix Steps

1. restore backend process availability or network reachability
2. correct health endpoint path, port, TLS mode, or timeout settings
3. make thresholds less sensitive only if the backend is healthy but noisy
4. reactivate traffic gradually and confirm healthy backend count recovers

### Relevant Logs, Metrics, and Endpoints

- endpoints: `/admin/runtime`
- metrics: `spooky_health_checks_total`, `spooky_health_checks_success`, `spooky_health_checks_failure`, `spooky_health_failures_total`
- logs: backend health transitions, health-check timeouts, passive health failures

## TLS Handshakes Are Failing

### Symptom

- clients cannot establish downstream TLS or QUIC sessions
- upstream secure backends fail during TLS connection setup
- certificate panels show abnormal expiry or selection behavior

### Likely Causes

- expired, missing, or unreadable certificates
- server name or certificate selection mismatch
- unsupported protocol or ALPN mismatch
- upstream TLS verification or handshake failure
- clients are speaking plain HTTP to a TLS listener

### Verification Steps

1. inspect downstream TLS metrics:
   - `spooky_downstream_tls_handshake_success_total`
   - `spooky_downstream_tls_handshake_failure_total`
   - `spooky_downstream_tls_certificate_selection_total`
   - `spooky_downstream_tls_alpn_total`
   - `spooky_downstream_tls_certificate_days_remaining`
2. inspect upstream TLS metrics:
   - `spooky_upstream_tls_failure_total`
3. inspect listener and backend TLS configuration in the active runtime
4. inspect logs for handshake-failure reason, certificate-selection result, and SNI context
5. verify certificate expiration and client compatibility outside the data path if needed

### Fix Steps

1. replace expired or invalid certificates
2. correct listener certificate selection or server-name configuration
3. confirm clients are using the expected protocol and ALPN
4. correct upstream CA, hostname, or TLS settings for secure backends
5. verify handshake failures return to baseline after the fix

### Relevant Logs, Metrics, and Endpoints

- endpoints: `/admin/runtime`
- metrics: `spooky_downstream_tls_handshake_success_total`, `spooky_downstream_tls_handshake_failure_total`, `spooky_downstream_tls_certificate_selection_total`, `spooky_downstream_tls_alpn_total`, `spooky_downstream_tls_certificate_not_after_seconds`, `spooky_downstream_tls_certificate_days_remaining`, `spooky_upstream_tls_failure_total`
- logs: downstream handshake failures, certificate-selection anomalies, upstream TLS failure logs

## Control API Calls Fail or Runtime Changes Do Not Apply

### Symptom

- control API requests return `401`, `403`, `404`, or handshake failure
- validate, preview, activate, rollback, reload, or restart actions fail
- operators are unsure whether the runtime changed or not

### Likely Causes

- wrong control API protocol or path
- missing or invalid bearer token
- authenticated caller does not have the required role
- control API mTLS is required and the client certificate is missing or invalid
- the candidate config is incompatible with the active runtime
- rollback target is not available in retained history

### Verification Steps

1. confirm the control API is called with HTTP/1.1 over TLS
2. confirm the request is using the correct path and admin credentials
3. inspect `GET /admin/runtime` and `GET /admin/runtime/history`
4. inspect:
   - `spooky_runtime_validation_attempts_total`
   - `spooky_runtime_preview_attempts_total`
   - `spooky_runtime_activation_total`
   - `spooky_runtime_rollback_total`
   - `spooky_runtime_rejections_total`
   - `spooky_control_api_connection_limit_drops`
5. inspect audit events for actor, action, result, reason, and failure class
6. inspect logs for authn failure, authz failure, mTLS failure, or generation-activation failure

### Fix Steps

1. use the correct protocol, path, and credentials
2. fix client certificate or token issues before retrying the admin action
3. validate and preview the candidate config before activation
4. roll back to the last healthy generation if the latest activation degraded runtime behavior
5. confirm the active generation changed only after activation succeeds

### Relevant Logs, Metrics, and Endpoints

- endpoints: `/admin/runtime`, `/admin/runtime/history`, `/admin/runtime/history/{generation}`, `/admin/runtime/validate`, `/admin/runtime/preview`, `/admin/runtime/activate`, `/admin/runtime/rollback`
- metrics: `spooky_runtime_validation_attempts_total`, `spooky_runtime_preview_attempts_total`, `spooky_runtime_activation_total`, `spooky_runtime_rollback_total`, `spooky_runtime_rejections_total`, `spooky_control_api_connection_limit_drops`
- logs: control-plane authn/authz failures, mTLS handshake failures, runtime activation/rollback logs
- audit: actor, action, target generation, result, reason, failure class

## Grafana or Prometheus Shows Missing or Empty Data

### Symptom

- Prometheus target is down
- dashboards render without series even though traffic exists
- only coarse request totals move while richer dashboards stay empty

### Likely Causes

- Prometheus is scraping the wrong host, port, or path
- the metrics endpoint is disabled or not reachable from the scraper
- dashboards are querying labeled metrics but only low traffic or the wrong labels are present
- no signal has been generated for the specific feature being viewed
- the runtime generation changed but dashboards or annotations are pointed at stale state

### Verification Steps

1. open the Prometheus target page and confirm the Spooky target is up
2. curl the metrics endpoint directly and confirm the configured path returns Prometheus text
3. inspect whether the expected families exist:
   - request and latency metrics
   - overload and quota metrics
   - backend, retry, hedge, and DNS metrics
   - TLS and control-plane metrics
4. confirm dashboard queries use canonical labels and existing label values
5. confirm real traffic was generated for the feature area you are testing

### Fix Steps

1. fix the scrape address, port, or path
2. expose the metrics endpoint to Prometheus from the correct network
3. update dashboards to use canonical metric names and stable label values
4. generate traffic that exercises the exact feature under test
5. verify data appears first in Prometheus, then in recording rules, then in Grafana

### Relevant Logs, Metrics, and Endpoints

- endpoints: `/metrics`, `/admin/runtime`
- metrics: all families in [Metrics Reference](../reference/metrics-reference.md)
- logs: metrics endpoint startup logs, scrape-path mismatches, observability package deployment logs

## Capture a Useful Incident Bundle

When you need to escalate an incident or hand it to another operator, capture:

1. active runtime generation and recent generation history
2. current backend health summary and quota backend health summary
3. request, overload, quota, backend, TLS, and control-plane metrics
4. relevant runtime logs for the incident window
5. relevant audit events for admin actions in the same window

Recommended capture checklist:

```bash
curl -k --http1.1 https://127.0.0.1:9902/health
curl -k --http1.1 https://127.0.0.1:9902/ready
curl -k --http1.1 -H "Authorization: Bearer <token>" https://127.0.0.1:9902/admin/runtime
curl -k --http1.1 -H "Authorization: Bearer <token>" https://127.0.0.1:9902/admin/runtime/history
curl -s http://127.0.0.1:9901/metrics
```

Also capture:

- startup and incident-window logs
- the sanitized config that produced the active generation
- the exact time window of the incident
- the caller, tenant, route, upstream, backend, or generation involved

## Related Pages

- [Operations Overview](../operations/overview.md)
- [Runbook](../operations/runbook.md)
- [Metrics Reference](../reference/metrics-reference.md)
- [Control API Reference](../reference/control-api-reference.md)
- [Control Plane](../operations/control-plane.md)
- [Observability Operator Bundle](../operations/observability-bundle.md)
- [Metrics and Alerts](../operations/metrics-and-alerts.md)
