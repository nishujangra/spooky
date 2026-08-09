# Observability Contract

This document defines the stable observability contract for Spooky after the refactor work that centralized reason vocabularies, outcome recording, backend lifecycle snapshots, control-plane runtime views, and admin audit events.

## Purpose

Spooky has five operator-facing observability surfaces:

- Prometheus metrics
- structured operational logs
- OTLP traces
- control API runtime snapshots
- structured control-plane audit events

These surfaces should describe the same events using the same vocabulary.

The source of truth for that vocabulary is:

- `crates/edge/src/observability/mod.rs`

This file defines the canonical enums, stable slugs, and field names that all emitters should use.

## Source Of Truth

The source of truth is not a dashboard JSON file, alert rule, or one emitter implementation.

The source of truth is the canonical vocabulary in:

- `crates/edge/src/observability/mod.rs`

Operator-packaged assets must be derived from that module's stable public meaning.

The main source-of-truth enums and helper types are:

- `RequestOutcomeReason`
- `BackendHealthReason`
- `RetryDecisionReason`
- `HedgeDecisionReason`
- `AdmissionDecisionReason`
- `AdmissionOverloadCause`
- `QuotaPolicyDecision`
- `QuotaPolicyReason`
- `QuotaBackendHealthReason`
- `OperationalEventContext`
- `MetricReasonLabels`

If one of those enums changes, that is an observability contract change. Dashboard queries, alert rules, runbooks, and control-plane JSON examples must be reviewed together.

## Contract Rule

One operational concept should have:

- one canonical enum value
- one canonical slug string
- one consistent meaning across metrics, logs, traces, control API payloads, and audit events

The contract exists to prevent drift such as:

- one reason name in metrics
- a different prose string in logs
- a third serialized form in control-plane JSON
- a fourth ad hoc field in audit events
- a fifth trace attribute that cannot be joined back to the operator vocabulary

## Canonical Reason Families

The main canonical reason families are:

- `RequestOutcomeReason`
- `BackendHealthReason`
- `RetryDecisionReason`
- `HedgeDecisionReason`
- `AdmissionDecisionReason`
- `AdmissionOverloadCause`

Each of these exposes a stable `slug()` used as the canonical external string.

## Stable String Contract

The canonical slug is the external operator contract.

That means the slug should be treated as the stable value for:

- metric label values
- structured log `reason=` values
- trace span attributes describing the same reason
- control API serialized fields
- audit event `reason` values

Changing a slug is an observability contract change, not a cosmetic refactor.

## Operator Correlation Contract

Operators need a stable path from an alert to a dashboard panel to an event log or audit record to a runtime snapshot.

The default correlation field set is:

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

### Correlation field meanings

- `request_id` identifies a request-path event when request-scoped logging or tracing exists
- `trace_id` identifies the distributed trace carrying the request or control-plane action
- `span_id` identifies the local operation within that trace
- `event_id` identifies a discrete audit or operator-significant event
- `generation` identifies the active or candidate runtime generation involved in a control-plane action
- `listener` identifies the ingress or admin listener involved in the event
- `route` is a route identity when route-specific visibility exists
- `upstream` is the canonical traffic attribution field for request-path metrics and logs
- `backend` is the selected backend identity
- `reason` is the canonical reason slug from `crates/edge/src/observability/mod.rs`
- `failure_class` is a coarse refinement axis where `reason` alone is not sufficient
- `policy` identifies the named policy that produced a decision when policy attribution exists
- `component` identifies the subsystem such as `admission`, `quota`, `tls`, `runtime`, or `control_api`

### Surface expectations

- metrics should use low-cardinality correlation dimensions only
- logs should emit the canonical field names when the values are known
- traces should carry the same canonical names as span attributes where request or control-plane tracing exists
- control API snapshots should expose current state using the same identities and reason slugs
- audit events should serialize the same actor, target, generation, and reason meaning used elsewhere

Absence is better than fabricated values. If a field is not known, omit it rather than inventing placeholders.

## Structured Log Field Contract

Operational logs should use the canonical `OperationalEventContext` field set.

The canonical fields are:

- `request_id`
- `route`
- `upstream`
- `backend`
- `reason`
- `failure_class`

These fields are rendered in stable `key=value` form and unset fields are omitted.

### Practical meaning

- use `upstream` for route/upstream attribution in request-path events
- use `backend` for selected backend identity
- use `reason` for the canonical reason slug
- use `failure_class` when a coarse outcome still needs finer transport or rejection detail

Do not invent new ad hoc field keys for the same concept if the canonical field already exists.

## Trace Attribute Contract

Tracing is a diagnostic surface, not a separate vocabulary.

Where traces are emitted, span and event attributes should reuse the canonical names:

- `request_id`
- `trace_id`
- `span_id`
- `generation`
- `listener`
- `route`
- `upstream`
- `backend`
- `reason`
- `failure_class`
- `policy`
- `component`

Trace attributes may include additional diagnostic detail, but canonical operator fields must keep the same names and meanings as logs, metrics, control API, and audit events.

## Metric Label Contract

Reasoned metric emitters should use the canonical label model:

- `outcome`
- `reason`
- `failure_class`

This contract is represented by `MetricReasonLabels`.

### Meaning of the labels

- `outcome` is the coarse terminal bucket such as success, failure, timeout, overload, or rate-limited
- `reason` is the canonical reason slug
- `failure_class` carries finer transport or rejection detail where needed

This is what allows dashboards to compare:

- route outcomes
- backend failures
- retries and hedges
- admission denials

without each emitter choosing different label semantics.

## Audit Event Contract

The control-plane audit stream is part of the operator observability bundle, not a side channel.

Audit events should use the same identities and reason vocabulary as other surfaces, with fields oriented around:

- actor
- action
- target
- generation
- result
- reason
- event_id
- peer address
- authentication mechanism

Audit events may include control-plane-specific context, but they should not redefine canonical reason values or resource identities already established elsewhere.

## Control API Snapshot Contract

The control API is not just an admin action surface. It is also a runtime introspection contract.

The runtime snapshot should describe the current active generation and shared operational state using the same meaning as logs and metrics.

Important snapshot areas include:

- active generation id and config path
- observability package metadata and contract versions
- worker expectations
- watchdog state
- adaptive admission state
- backend lifecycle inventory
- coarse metrics summary
- listener TLS inventory

### Observability package metadata

The runtime snapshot is also the packaged entry point into observability for operators.

The snapshot should expose:

- the current active generation
- the active observability contract version
- the control-plane audit schema version
- a backend health summary
- a quota backend health summary
- recent tracked admin/runtime actions when runtime history is available
- stable references to shipped dashboard definition files
- stable references to operator documentation files

These references should point to repository-managed assets such as:

- `deploy/observability/grafana/*.json`
- `docs/architecture/observability-contract.md`
- `docs/operations/control-plane.md`
- `docs/operations/metrics-and-alerts.md`
- `docs/operations/distributed-quota.md`

This is intentionally not a UI URL contract. The control plane should expose stable
package references that other operator tooling can map into local workflows.

### Backend snapshot meanings

Backend lifecycle payloads expose:

- backend identity
- health
- `health_reason`
- membership
- authority host and port
- resolved addresses
- resolution generation
- last successful refresh time
- per-upstream placement details

These fields should be interpreted as the canonical backend lifecycle view, not as a best-effort debug dump.

## Runtime Snapshot Boundaries

Control-plane surfaces should consume canonical runtime views, not listener-local state.

That means runtime snapshot rendering should depend on:

- the active runtime generation
- shared runtime services
- backend lifecycle inventory
- watcher/coordinator state

It should not reconstruct state by reaching into multiple unrelated listener internals.

## Canonical Dimensions Versus Diagnostic Dimensions

The operator bundle must distinguish between dimensions that are safe for dashboards and alerts and dimensions that belong only in logs, traces, or targeted investigation.

### Canonical operator dimensions

These are acceptable for stable metrics, dashboards, alerts, and control-plane summaries:

- `upstream`
- `backend`
- `listener`
- `protocol`
- `status_class`
- `outcome`
- `reason`
- `failure_class`
- `policy`
- `component`
- `generation` when bounded to active or recent generations

### High-cardinality diagnostic dimensions

These should not be introduced as default metric labels or dashboard variables:

- raw request path
- user id
- tenant id
- token id
- client ip
- header values
- query strings
- certificate fingerprints for every request
- arbitrary trace baggage
- per-request nonce or session identifiers

These belong in:

- structured logs
- trace attributes
- audit events where operationally justified
- control-plane detail views

The rule is simple: if a dimension can grow with end-user traffic, it is diagnostic by default, not metric-facing by default.

## Metrics vs Logs vs Control API

These surfaces serve different operational jobs.

### Metrics

Metrics are for:

- time-series alerting
- SLO and capacity dashboards
- rate and latency trends

### Logs

Logs are for:

- event-level diagnosis
- request-path and lifecycle debugging
- understanding the exact reason a single action happened

### Traces

Traces are for:

- cross-component timing analysis
- causal sequencing within a single request or control-plane action
- joining request-path latency with retry, hedge, auth, and backend sub-operations

### Control API snapshots

Control API snapshots are for:

- current-state inspection
- rollout verification
- backend and watchdog status inspection
- runtime-generation confirmation

### Audit events

Audit events are for:

- operator accountability
- change history
- security-relevant admin actions
- correlating runtime changes with traffic or control-plane symptoms

They should complement each other, not contradict each other.

## Cardinality Rules

The observability contract aims for stable, low-cardinality labels on default operational metrics.

Use labeled metrics for:

- upstream
- backend
- listener
- protocol
- canonical reason slugs

Avoid introducing unbounded labels such as:

- raw user IDs
- arbitrary request paths
- per-request trace tokens

If a dimension is needed for debugging but not for alerting, prefer logs or control-plane snapshots over high-cardinality metrics.

## Dashboard And Alert Compatibility Promise

Shipped dashboards and alert rules are versioned operator assets that depend on this contract.

Compatibility promises:

- canonical metric names and canonical reason label values are treated as stable operator API
- dashboard panels should depend on canonical metrics or recording rules built from canonical metrics
- alert rules should depend on canonical metrics or recording rules, not prose log parsing
- control-plane examples and runbooks should use the same reason slugs and field names as the code contract
- additive fields are allowed when they do not change existing meanings
- renaming a canonical slug, field key, or low-cardinality label is a breaking observability change

When a breaking observability change is unavoidable:

- update `crates/edge/src/observability/mod.rs`
- update this contract document in the same change
- update shipped dashboards and alerts in the same change
- document migration notes for operators
- avoid silent drift where old dashboards continue to query non-existent or semantically changed series

## Backend Lifecycle Observability

Backend health and refresh behavior should be visible consistently across surfaces.

Important canonical concepts include:

- refresh classification
- health-failure reason
- membership state
- placement state
- client rotation behavior

This lets operators answer:

- did refresh fail or just return the same addresses
- was a backend removed, suppressed, or merely unhealthy
- did traffic continue on retained addresses after refresh failure

## Request Outcome Observability

Request outcome recording provides the canonical answer to:

- did the request succeed
- did it time out
- was it rejected by policy
- did backend transport/protocol/TLS fail
- was it shed by overload controls

This contract should be consistent for:

- QUIC forwarding
- bootstrap compatibility ingress
- retries and hedges
- admission and auth rejections

## Reload and Runtime Observability

Runtime generation changes should be visible through:

- control API runtime generation fields
- logs around generation swaps and rejection reasons
- metrics that surface restart requests, degraded windows, and control-plane activity

Operators should not have to infer whether a reload committed from unrelated side effects.

## Contributor Rules

When adding a new operational reason:

- add it to the canonical observability vocabulary
- give it one stable slug
- map local enums into the canonical reason if the local enum is richer
- reuse canonical field names in logs
- reuse canonical label names in metrics
- expose the same meaning in control-plane JSON where relevant

Do not:

- invent a new public reason string in one emitter only
- log prose-only reasons that cannot be reconciled with metrics
- serialize backend or request state under different names in different surfaces

## Mental Model

The observability contract is successful when an operator can look at:

- a metric label
- a log line
- a control API field

and know they are all describing the same event using the same vocabulary.
