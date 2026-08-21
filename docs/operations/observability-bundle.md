# Observability Operator Bundle

This page documents the shipped operator observability bundle for Impulse.

It is the packaging guide for the artifacts under:

- `deploy/observability/grafana/`
- `deploy/observability/prometheus/recording-rules.yaml`
- `deploy/observability/prometheus/alerts.yaml`
- `deploy/observability/slo/`

Use this page when you need to answer:

- which dashboard to open first
- what each alert severity means
- how to interpret the packaged SLOs
- how to correlate one incident across metrics, logs, traces, control API, and audit
- how to roll out or version the observability package safely

The canonical observability vocabulary still lives in
[`docs/architecture/observability-contract.md`](../architecture/observability-contract.md)
and `crates/edge/src/observability/mod.rs`. This page describes the shipped
operator experience built on top of that contract.

## Open This First

Use this table when you need a fast starting point during an incident:

| Symptom | First dashboard | First runtime view | Why |
|---|---|---|---|
| customer-visible 5xx or latency regression | `edge-traffic.json` | `GET /admin/runtime` | start broad, then localize by upstream or backend |
| rising 429s or policy denials | `admission-overload.json` | `GET /admin/runtime` | separates quota, overload, auth, and rate-limit outcomes |
| rising backend timeouts or backend 5xx concentration | `backend-health.json` | `GET /admin/runtime` | ties request failures to backend lifecycle and DNS state |
| retry or hedge growth | `retries-hedges.json` | `GET /admin/runtime` | shows whether resilience is helping or amplifying failure |
| handshake or certificate trouble | `tls-certificates.json` | `GET /admin/runtime` | separates downstream TLS, upstream TLS, and cert expiry |
| activation, rollback, or restart trouble | `control-plane.json` | `GET /admin/runtime/history` | shows runtime generation state and recent control-plane activity |

## Shipped Package

The current package is made of four layers:

1. recording rules
2. alert rules
3. SLO definitions
4. Grafana dashboards

The operator-facing contract version and audit schema version are surfaced in
the control API runtime views. At this point the packaged values are:

- observability contract version: `v1`
- audit schema version: `v1`

## Package Inventory

### Prometheus recording rules

File:

- `deploy/observability/prometheus/recording-rules.yaml`

Purpose:

- turn raw scrape-time series into stable operator queries
- keep dashboards and alerts off ad hoc raw PromQL
- provide bounded-cardinality rollups for traffic, admission, backend, TLS, quota, and control-plane views

### Prometheus alert rules

File:

- `deploy/observability/prometheus/alerts.yaml`

Purpose:

- define page-level and ticket-level production alerts
- consume recording rules instead of duplicating raw query logic
- map every alert to a concrete runbook or operations document

### SLO package

Files:

- `deploy/observability/slo/README.md`
- `deploy/observability/slo/definitions.promql`

Purpose:

- lock the numerator and denominator contracts for operator reporting
- keep overload, quota, auth, and backend timeout concepts separate
- provide the canonical 30-minute reporting layer used by dashboards and burn-rate alerts

### Grafana dashboards

Files:

- `deploy/observability/grafana/edge-traffic.json`
- `deploy/observability/grafana/admission-overload.json`
- `deploy/observability/grafana/backend-health.json`
- `deploy/observability/grafana/retries-hedges.json`
- `deploy/observability/grafana/tls-certificates.json`
- `deploy/observability/grafana/control-plane.json`

Purpose:

- provide a packaged operator workflow instead of leaving teams to assemble dashboards ad hoc
- make the distinction between traffic failure, overload, quota, auth, backend health, TLS, and control-plane state obvious

## Dashboard Guide

### `edge-traffic.json`

Dashboard:

- title: `Impulse Edge Traffic and Latency`
- uid: `impulse-edge-traffic`

Open this first for:

- request volume shifts
- success-rate regression
- p50, p95, or p99 latency growth
- failing upstream or backend concentration
- active runtime generation drift across the fleet

Primary panels:

- `Request Rate`
- `Success Rate`
- `P50 Latency`
- `P95 Latency`
- `P99 Latency`
- `Traffic Volume by Upstream`
- `Status Class Mix`
- `Upstream Outcome Mix`
- `Top Failing Upstreams`
- `Top Failing Backends`

Operator intent:

- this is the entry dashboard for customer-visible traffic symptoms
- use it to decide whether the problem is broad, upstream-localized, or backend-localized before opening specialized dashboards
- from here, the most common next move is either `backend-health.json` for timeout-heavy failures or `admission-overload.json` for 503s that might be self-protection

### `admission-overload.json`

Dashboard:

- title: `Impulse Admission, Overload, Quota, and Auth`
- uid: `impulse-admission-overload`

Open this when:

- 429 or policy denials rise
- 503s might be overload-related
- brownout or adaptive admission is suspected
- quota backend degradation is reported
- auth denials or auth dependency failures increase

Primary panels:

- `Brownout State`
- `Overload Shed Rate`
- `Circuit Open Reject Rate`
- `Scoped Rate Limit Reject Rate`
- `Overload Shed by Reason`
- `Adaptive Admission Behavior`
- `Quota Decisions`
- `Quota Backend Health`
- `Top Quota Denials by Policy and Reason`
- `Auth Denied vs Unavailable`
- `Auth Contract Detail`

Operator intent:

- preserve the runtime boundary between overload control and policy-contract failure
- do not interpret quota denial as overload
- do not interpret auth unavailability as quota exhaustion
- if quota-backend degradation is visible here, confirm runtime state in `GET /admin/runtime` before changing policy

### `backend-health.json`

Dashboard:

- title: `Impulse Backend Health and DNS Lifecycle`
- uid: `impulse-backend-health`

Open this when:

- backend timeouts are rising
- backend errors are concentrated on a subset of upstream/backend pairs
- active probe failures and request-path failures disagree
- DNS refresh or topology churn may be driving instability

Primary panels:

- `Health Check Failure Ratio`
- `Backend Timeout Rate`
- `Backend Error Rate`
- `DNS Refresh Failure Ratio`
- `Active Health Check Outcomes`
- `Passive Health Failures by Reason`
- `Backend Timeout and Error Pressure`
- `Top Backend Errors by Upstream and Backend`
- `DNS Refresh Outcomes and Address-Set Changes`
- `Client Rotations and Failures`
- `Resolved Addresses by Backend`
- `Last Successful DNS Refresh Age by Backend`

Operator intent:

- separate active health probes from passive request-path failures
- show when DNS churn or stale resolver state is the real cause of backend instability
- if this dashboard shows churn or degraded health while traffic is failing, the next control-plane read should usually be `GET /admin/runtime`

### `retries-hedges.json`

Dashboard:

- title: `Impulse Retries and Hedges`
- uid: `impulse-retries-hedges`

Open this when:

- retries increase faster than raw request failure
- hedge activity appears to be masking latency regression
- duplicate work might be amplifying backend pressure

Primary panels:

- `Retry Attempt Rate`
- `Retry Ratio`
- `Retry Reasons`
- `Retry Denials by Reason`
- `Retry Pressure vs Backend Failure Pressure`
- `Hedge Trigger Rate`
- `Hedge Waste Ratio`
- `Hedge Trigger, Win, Waste, and Primary-Won Patterns`
- `Hedge Effectiveness Ratios`
- `Average Primary Late Time After Hedge Trigger`
- `Primary Late Sample Rate`

Operator intent:

- distinguish resilience behavior that is helping from resilience behavior that is amplifying failure
- read retry growth and hedge waste together with backend timeout pressure
- if hedge growth is high but user-visible latency is still rising, treat it as backend stress rather than success

### `tls-certificates.json`

Dashboard:

- title: `Impulse TLS Certificates and Handshakes`
- uid: `impulse-tls-certificates`

Open this when:

- handshake failures rise
- a certificate rotation is in progress
- ALPN negotiation or SNI selection looks wrong
- upstream TLS failures might be request-path specific

Primary panels:

- `Handshake Failure Ratio`
- `Handshake Failure Rate`
- `Minimum Certificate Days Remaining`
- `Upstream TLS Failure Rate`
- `Downstream Handshake Failures by Listener and Reason`
- `Top Upstream TLS Failures by Backend, Phase, and Reason`
- `Lowest Certificate Days Remaining`
- `Certificate Selection Outcomes by Listener`
- `Negotiated ALPN by Listener`

Operator intent:

- separate downstream handshake problems from upstream TLS request-path failures
- make certificate expiry and unexpected selection behavior visible before client impact broadens
- after a certificate change, confirm both the dashboard and the runtime snapshot before expanding rollout

### `control-plane.json`

Dashboard:

- title: `Impulse Control-Plane Activity`
- uid: `impulse-control-plane`

Open this when:

- reload, activation, rollback, or restart workflows fail
- watchdog or runtime panic signals appear
- audit-correlated control-plane activity needs validation
- runtime generation state might be drifting across the fleet

Primary panels:

- `Runtime State`
- `Active Runtime Generation`
- `Runtime History Depth`
- `Audit-Correlated Control-Plane Activity Rate`
- `Control-Plane Error Event Rate`
- `Validation, Preview, Activation, and Rollback Activity`
- `Runtime Outcomes by Result and Reason`
- `Runtime Rejections by Reason`
- `Watchdog State and Runtime Health`
- `Control API Pressure and Error Events`

Operator intent:

- this is the admin-plane dashboard, not a traffic dashboard
- use it to correlate runtime operations with emitted audit events and to verify whether the control plane is healthy enough to support incident response

## Runtime Introspection Entry Points

The control plane is part of the observability bundle, not a separate concern.

Use these endpoints as the runtime-introspection entry points:

- `GET /admin/runtime`
  Use this for current runtime state, observability package metadata, backend health summary, quota backend health summary, watchdog state, and recent admin actions.
- `GET /admin/runtime/history`
  Use this for retained generations, activation history, rollback candidates, and generation-scoped operator history.
- `GET /admin/runtime/history/{generation}`
  Use this when one generation needs focused review during rollback or activation debugging.

When dashboards show trend but not current state, these are the next calls to make.

## Audit As The Operator History Surface

The audit stream is the control-plane event history for:

- authentication and authorization outcomes
- validate, preview, activate, rollback, reload, restart, and cert reload attempts
- attempt versus result correlation
- actor attribution and peer attribution

The current audit schema version is `v1`, which matches the code-defined `ADMIN_AUDIT_SCHEMA_VERSION`.

Operators should treat audit as:

- the source of truth for who initiated a control-plane action
- the source of truth for whether the action succeeded, was denied, or failed
- the place to verify `requested_by`, actor roles, authn mechanisms, generation movement, and canonical reason or failure class

Metrics show rate and trend. Audit shows action sequence and attribution.

## Alert Severity And Runbook Mapping

The alert package has two severities:

- `page`: operator action is expected immediately because user impact or control-plane safety is already at risk
- `ticket`: the issue is meaningful and should be investigated, but it is not yet assumed to require immediate paging

### Page alerts

| Alert | Meaning | First dashboard | Runbook |
| --- | --- | --- | --- |
| `ImpulseAvailabilityBurnRatePage` | 5xx availability budget is burning in fast and sustained windows | `edge-traffic.json` | `docs/operations/runbook.md#scenario-rising-503-rate` |
| `ImpulseP99LatencyBurnPage` | p99 latency is materially above the packaged objective in fast and sustained windows | `edge-traffic.json` | `docs/operations/runbook.md#scenario-backend-timeout-surge` |
| `ImpulseBackendTimeoutSurgePage` | backend timeout ratio is high enough to threaten user-visible reliability | `backend-health.json` | `docs/operations/runbook.md#scenario-backend-timeout-surge` |
| `ImpulseTlsCertificateExpiryCritical` | at least one downstream certificate has fewer than 7 days remaining | `tls-certificates.json` | `docs/operations/runbook.md#scenario-cert-rotation` |
| `ImpulseWatchdogDegraded` | watchdog degraded windows are being recorded continuously | `control-plane.json` | `docs/operations/runbook.md#scenario-control-api-or-metrics-endpoint-unavailable` |
| `ImpulseRuntimePanicObserved` | runtime panic counters are non-zero in the recent window | `control-plane.json` | `docs/operations/runbook.md#after-any-incident` |
| `ImpulseControlPlaneUnavailable` | packaged control-plane recording-rule series are absent | `control-plane.json` | `docs/operations/runbook.md#scenario-control-api-or-metrics-endpoint-unavailable` |

### Ticket alerts

| Alert | Meaning | First dashboard | Runbook |
| --- | --- | --- | --- |
| `ImpulseRetryGrowth` | retry ratio is rising and may be amplifying backend trouble | `retries-hedges.json` | `docs/operations/runbook.md#scenario-backend-timeout-surge` |
| `ImpulseHedgeGrowth` | hedge activity is becoming routine rather than exceptional | `retries-hedges.json` | `docs/operations/runbook.md#scenario-backend-timeout-surge` |
| `ImpulseBackendDnsRefreshFailures` | DNS refresh failures and ratio indicate stale or unstable backend resolution | `backend-health.json` | `docs/operations/runbook.md#scenario-backend-timeout-surge` |
| `ImpulseBrownoutActiveTooLong` | brownout is staying active beyond the tolerated window | `admission-overload.json` | `docs/operations/runbook.md#scenario-brownout-or-overload-triggering` |
| `ImpulseQuotaBackendDegraded` | the distributed quota backend is timing out, unavailable, or otherwise degraded | `admission-overload.json` | `docs/operations/distributed-quota.md` |
| `ImpulseTlsHandshakeFailuresRising` | downstream handshake failures are rising above normal background levels | `tls-certificates.json` | `docs/operations/runbook.md#scenario-handshake-failures-or-client-connection-failures` |

## SLO Interpretation

The SLO package is defined in `deploy/observability/slo/`. Use the packaged
`impulse:slo_*` series for reporting rather than rebuilding numerator and
denominator logic in Grafana.

### Availability

Primary series:

- `impulse:slo_availability_ratio:30m`
- `impulse:slo_request_server_error_ratio:rate30m`

Interpretation:

- this is the coarse user-visible server-failure contract
- upstream `5xx`, overload-generated `503`, and backend failures that surface as `5xx` count here
- auth denials, quota denials, and scoped rate limits do not count here

### Latency

Primary series:

- `impulse:slo_request_latency_ms:p50_30m`
- `impulse:slo_request_latency_ms:p95_30m`
- `impulse:slo_request_latency_ms:p99_30m`

Interpretation:

- these percentiles describe successful service only
- rejected work, timeout outcomes, and quota or auth denials are intentionally excluded
- use the edge traffic dashboard for cluster-wide latency, then move to backend or retry dashboards if tails deteriorate

### Overload shed rate

Primary series:

- `impulse:slo_overload_shed_ratio:30m`

Interpretation:

- only canonical overload-control decisions count
- quota denials, auth denials, and scoped rate limits are excluded
- this is a capacity-protection signal, not a general policy-denial signal

### Backend timeout rate

Primary series:

- `impulse:slo_backend_timeout_ratio:30m`

Interpretation:

- this is narrower than overall backend failure
- it should be read with `backend-health.json` and `retries-hedges.json`
- do not silently widen it into a generic backend error SLO

### Auth failure rate

Primary series:

- `impulse:slo_auth_denial_ratio:30m`

Interpretation:

- this covers explicit auth contract failures such as external auth denial and local JWT validation failure
- it does not fold in generic policy rejection or fail-open dependency errors

### Quota denial operator view

Primary series:

- `impulse:slo_quota_denial_ratio:30m`

Interpretation:

- quota is a contract-enforcement view, not overload
- quota backend degradation should be interpreted alongside quota backend health, not mislabeled as ordinary traffic failure

## Correlation Workflow

The observability bundle is designed so an operator can start from any one
surface and move to the others without translating vocabulary.

The stable correlation fields are:

- `request_id`
- `trace_id`
- `span_id`
- `event_id`
- `generation`
- `listener`
- `route`
- `upstream`
- `backend`
- `reason`
- `failure_class`
- `policy`
- `component`

The source of truth for reason and decision vocabulary is:

- `crates/edge/src/observability/mod.rs`

### Alert to dashboard

1. Read the firing series labels.
2. Keep the canonical low-cardinality labels intact:
   `upstream`, `backend`, `listener`, `reason`, `decision`, `policy`, `backend_mode`.
3. Open the dashboard named in the alert mapping above.
4. Validate whether the issue is traffic, admission, backend, TLS, or control-plane scoped.

### Dashboard to control API

Use the control API when you need current runtime state rather than trend.

Look for:

- current runtime generation
- backend lifecycle and membership state
- quota backend availability and recent errors
- active observability contract version
- recent admin actions
- audit schema version

The main runtime entry points are documented in
[Control Plane](control-plane.md).

When the runtime snapshot is open, the high-signal fields to inspect first are:

- `observability.contract_version`
- `observability.audit_schema_version`
- `observability.current_generation`
- `observability.backend_health_summary`
- `observability.quota_backend_health_summary`
- `observability.recent_admin_actions`

### Dashboard to logs and traces

Use logs and traces when the packaged metrics explain the class of failure but
not the specific request path.

Look for the same canonical names:

- logs: `request_id`, `upstream`, `backend`, `reason`, `failure_class`
- traces: `trace_id`, `span_id`, `generation`, `listener`, `upstream`, `backend`, `reason`

Do not correlate by prose strings when a canonical slug is available.

### Control API to audit

Use the audit stream when the incident involves:

- reload or activation attempts
- rollback decisions
- control-plane auth failures
- certificate reload attempts
- actor attribution

Audit is the source of truth for control-plane action history. Metrics and
dashboards show rate and trend; audit shows who did what, against which target,
with which generation and result.

### Metrics to audit

Move directly from metrics to audit when:

- control-plane errors or restart requests rise
- runtime activation or rollback outcomes change unexpectedly
- auth failures are clearly on the admin plane rather than the request path

Typical sequence:

1. identify the metric family and time window
2. open `control-plane.json`
3. read `GET /admin/runtime` or `GET /admin/runtime/history`
4. use audit events for actor, action, and result attribution

## Incident Workflows

### Rising 5xx or latency page

1. Open `edge-traffic.json`.
2. Check whether `Top Failing Upstreams` or `Top Failing Backends` is concentrated.
3. If timeout-heavy, move to `backend-health.json`.
4. If retry or hedge growth is visible, move to `retries-hedges.json`.
5. If 503s are overload-generated rather than upstream-generated, move to `admission-overload.json`.

### Quota backend degraded

1. Open `admission-overload.json`.
2. Read `Quota Decisions`, `Quota Backend Health`, and `Top Quota Denials by Policy and Reason`.
3. Confirm current backend mode and degraded state in the control API runtime snapshot.
4. Use [Distributed Quota](distributed-quota.md) for fail-open, fail-closed, and fallback interpretation.

### TLS or certificate incident

1. Open `tls-certificates.json`.
2. Separate downstream handshake failures from upstream TLS failures.
3. Check the certificate days-remaining panels before assuming the issue is trust or routing.
4. If the issue followed a certificate or listener change, use the runbook certificate-rotation path.

### Control-plane incident

1. Open `control-plane.json`.
2. Check runtime generation, recent activity rate, runtime outcomes, watchdog state, and control API pressure.
3. Read runtime snapshot and runtime history from the control API.
4. Use audit records for actor attribution and operation sequence.

## Rollout And Versioning

### Prometheus rollout order

1. Load `deploy/observability/prometheus/recording-rules.yaml`.
2. Confirm the `impulse:*` recording-rule series are present.
3. Load `deploy/observability/prometheus/alerts.yaml`.
4. Validate that every alert evaluates against recording rules rather than missing raw series.
5. Only then import or refresh dashboards.

Do not roll out alert rules before the recording rules they consume.

### Grafana import rules

- import the dashboard JSON files from `deploy/observability/grafana/`
- preserve the packaged `uid` values so links, references, and operator runbooks stay stable
- treat the repo JSON as the source of truth rather than hand-editing panels in production
- if you customize a dashboard locally, either keep it clearly separate or upstream the change into the repo package

Current packaged dashboard JSON files all ship with explicit `version` fields.
Treat a dashboard `version` bump in the repo as an intentional artifact change
that should be reviewed together with alert, SLO, or recording-rule changes.

### Control API compatibility checks

After rollout, confirm the control API runtime snapshot exposes:

- `observability.contract_version`
- `observability.audit_schema_version`
- `observability.dashboard_packages`
- `observability.documentation`

This is the runtime proof that the node is serving the expected operator bundle
contract.

### Safe production update pattern

1. update recording rules
2. wait for the new recording series to appear
3. update alerts
4. import dashboard JSON changes
5. verify runtime snapshot metadata
6. verify one page alert, one ticket alert, and one dashboard query in staging or canary before fleet-wide rollout

## Related Pages

- [Observability Contract](../architecture/observability-contract.md)
- [Metrics and Alerts](metrics-and-alerts.md)
- [Control Plane](control-plane.md)
- [Distributed Quota](distributed-quota.md)
- [Runbook](runbook.md)
- [Impulse SLO Package](../../deploy/observability/slo/README.md)
