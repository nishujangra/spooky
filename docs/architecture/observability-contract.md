# Observability Contract

This document defines the stable observability contract for Spooky after the refactor work that centralized reason vocabularies, outcome recording, backend lifecycle snapshots, and control-plane runtime views.

## Purpose

Spooky has three operator-facing observability surfaces:

- Prometheus metrics
- structured operational logs
- control API runtime snapshots

These surfaces should describe the same events using the same vocabulary.

The source of truth for that vocabulary is:

- `crates/edge/src/observability/mod.rs`

This file defines the canonical enums, stable slugs, and field names that all emitters should use.

## Contract Rule

One operational concept should have:

- one canonical enum value
- one canonical slug string
- one consistent meaning across metrics, logs, and control API payloads

The contract exists to prevent drift such as:

- one reason name in metrics
- a different prose string in logs
- a third serialized form in control-plane JSON

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
- control API serialized fields

Changing a slug is an observability contract change, not a cosmetic refactor.

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

## Control API Snapshot Contract

The control API is not just an admin action surface. It is also a runtime introspection contract.

The runtime snapshot should describe the current active generation and shared operational state using the same meaning as logs and metrics.

Important snapshot areas include:

- active generation id and config path
- worker expectations
- watchdog state
- adaptive admission state
- backend lifecycle inventory
- coarse metrics summary
- listener TLS inventory

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

### Control API snapshots

Control API snapshots are for:

- current-state inspection
- rollout verification
- backend and watchdog status inspection
- runtime-generation confirmation

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
