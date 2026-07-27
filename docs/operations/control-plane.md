# Control Plane

This document explains the operator-facing control-plane services in Spooky and the boundaries each service is allowed to know about runtime state.

## Services

The control plane consists of three main service surfaces:

- control API
- metrics endpoint
- watchdog service

These services are not listener sidecars anymore. They are explicit services built from canonical runtime views and shared services.

## Control API

The control API is the privileged administrative HTTP surface.

Its responsibilities are:

- health and readiness checks
- runtime snapshot rendering
- runtime reload
- listener certificate reload
- controlled restart requests

### Transport and protocol expectations

- protocol: HTTP/1.1 over TLS
- audience: operators and automation only
- security model: bearer-token protected for privileged routes

### Route families

The current route family includes:

- health
- ready
- runtime
- reload-certs
- reload
- restart

Refer to [Control API Reference](../reference/control-api-reference.md) for concrete endpoints.

### What the control API is allowed to know

The control API should read from:

- canonical runtime generation view
- shared runtime services
- backend lifecycle inventory
- watchdog state

It should not depend on listener-local internals that only exist because a particular ingress path happens to hold them.

## Metrics Endpoint

The metrics endpoint is the Prometheus scrape surface.

Its responsibilities are:

- expose rendered Prometheus text
- validate the configured scrape path
- stay bound to the current runtime metrics surface

### Operator expectations

- only the configured metrics path returns metrics
- wrong paths return `404`
- response content type is Prometheus text format
- metrics are rendered from the canonical shared metrics registry

The metrics endpoint is intentionally simpler than the control API. It is a read-only scrape surface, not an administration interface.

## Watchdog Service

The watchdog is the control-plane coordinator for runtime liveness degradation and controlled restart workflows.

Its responsibilities are:

- monitor poll progress and service health
- mark the runtime degraded when thresholds are crossed
- request controlled restarts
- coordinate drained worker completion across the runtime

### Important watchdog state

Operators should expect the watchdog to surface:

- whether it is enabled
- whether the system is degraded
- whether a restart has been requested
- restart reason
- restart request timestamp
- whether all expected workers have drained

This state is surfaced through control-plane runtime views rather than by reading listener internals directly.

## Runtime View Contract

All control-plane services should depend on the same runtime model:

- active runtime generation
- shared runtime services
- generation-owned state where relevant

This ensures:

- metrics and control API describe the same active generation
- restart and reload actions act on the same authoritative runtime handle
- backend lifecycle state is rendered from one canonical inventory

## Authentication and Access Model

### Control API

Privileged control API routes require bearer-token authorization.

Health and readiness routes are intentionally treated differently from privileged administration routes and may be left unauthenticated depending on configuration and deployment pattern.

### Metrics endpoint

The metrics endpoint is typically protected by deployment topology rather than by the control API auth model.

Expose it only on loopback or an isolated observability network unless you explicitly want broader visibility.

### Watchdog

The watchdog is not a public HTTP surface. It is an internal coordinator surfaced through metrics and control API snapshots.

## Separation from the Data Plane

Control-plane code should be able to answer operator questions without becoming part of the request hot path.

That means:

- no request-path policy logic should live in control-plane services
- no control-plane-only state should be required to serve requests
- admin surfaces should observe canonical runtime state, not own it

## Operational Expectations

Operators should expect the control plane to answer questions such as:

- what runtime generation is active
- is the system ready to serve
- is the watchdog degraded or requesting restart
- what do backend health and placement look like
- can a reload be applied safely

They should not need to infer those answers indirectly from unrelated logs.

## Failure Behavior

When control-plane services fail, the desired behavior is:

- fail clearly
- leave the active data plane intact
- preserve the authoritative runtime generation unless an explicit swap succeeded

For example:

- a rejected reload must leave the active generation unchanged
- a metrics scrape failure should not affect request serving
- a control API bind or TLS initialization failure should be treated as an explicit service failure, not hidden behind data-plane behavior

## Contributor Rules

When adding operator-facing functionality:

- put administrative request handling in control API modules
- put scrape-only rendering in metrics modules
- put restart/degraded coordination in watchdog
- read runtime state through canonical runtime views

Do not:

- add listener-local state dependencies only to satisfy an admin surface
- duplicate runtime snapshot assembly in multiple services
- let control-plane services become alternate owners of runtime state

## Related Pages

- [Observability Contract](../architecture/observability-contract.md)
- [Reload and Drain](reload-and-drain.md)
- [Metrics and Alerts](metrics-and-alerts.md)
- [Control API Reference](../reference/control-api-reference.md)
