# Control Plane

This document explains the operator-facing control-plane services in Spooky and the boundaries each service is allowed to know about runtime state.

## Operator Fast Path

Use the control plane to answer four questions quickly:

1. what runtime generation is active now
2. what changed recently
3. what is the current backend, quota, or watchdog state
4. can an operator action proceed safely

## Services

The control plane consists of four main operator-facing surfaces:

- control API
- metrics endpoint
- watchdog service
- audit stream

These services are not listener sidecars anymore. They are explicit services built from canonical runtime views and shared services.

## Control API

The control API is the privileged administrative HTTP surface.

Its responsibilities are:

- health and readiness checks
- runtime snapshot rendering
- staged runtime operations such as validate, preview, activate, and rollback
- legacy runtime reload
- listener certificate reload
- controlled restart requests

### Transport and protocol expectations

- protocol: HTTP/1.1 over TLS
- audience: operators and automation only
- security model: a dedicated admin-plane authn/authz layer, separate from request-path auth

### Admin-plane security contract

This section is the implementation contract for control API authn/authz. Later control-plane security work should follow this matrix rather than inventing route-by-route behavior ad hoc.

Authentication factors supported by the admin plane:

- bearer token only
- mTLS only
- mTLS + bearer token

Recommended production posture:

- require mTLS on the control API
- require either a bearer token or an mTLS-derived role-bearing identity
- keep the control API bound to loopback or a strongly isolated admin network even when auth is enabled

Role model:

- `viewer`: read-only operator visibility
- `operator`: read/write runtime operations that do not intentionally restart the process
- `admin`: destructive or high-impact operational control, including restart

Role inheritance:

- `operator` includes all `viewer` permissions
- `admin` includes all `operator` permissions

Route classification:

- unauthenticated or separately configurable: `/health`, `/ready`
- read-only privileged: `/admin/runtime`, `/admin/runtime/history`, `/admin/runtime/history/{generation}`
- mutating privileged: `/admin/runtime/validate`, `/admin/runtime/preview`, `/admin/runtime/activate`, `/admin/runtime/rollback`, `/admin/runtime/reload`, `/admin/runtime/reload-certs`, `/admin/runtime/restart`

Route-to-role matrix:

| Route family | Access level | Minimum role |
| --- | --- | --- |
| `/health` | unauthenticated or separately configurable | none |
| `/ready` | unauthenticated or separately configurable | none |
| `/admin/runtime` | read-only privileged | `viewer` |
| `/admin/runtime/history` | read-only privileged | `viewer` |
| `/admin/runtime/history/{generation}` | read-only privileged | `viewer` |
| `/admin/runtime/validate` | mutating privileged | `operator` |
| `/admin/runtime/preview` | mutating privileged | `operator` |
| `/admin/runtime/activate` | mutating privileged | `operator` |
| `/admin/runtime/rollback` | mutating privileged | `operator` |
| `/admin/runtime/reload` | mutating privileged | `operator` |
| `/admin/runtime/reload-certs` | mutating privileged | `operator` |
| `/admin/runtime/restart` | mutating privileged | `admin` |

Implementation rules:

- authentication failure must be distinct from authorization failure
- source-address policy must run before bearer-token validation when IP allowlisting is configured
- read-only runtime visibility is a `viewer` capability, not an implicit side effect of having any token
- restart is reserved for `admin`, even if other runtime mutation routes are granted to `operator`
- this admin-plane contract is separate from upstream/request-path auth policy and must stay in control-plane code

### Failure semantics

Control API authn/authz failures are intentionally split:

- `401 Unauthorized`: missing authentication or invalid authentication material
- `403 Forbidden`: authenticated but insufficient role, or denied by pre-auth source-address policy

Control API mTLS failure is separate:

- when `observability.control_api.tls.client_auth.mode: required`, missing or invalid client certificates fail the TLS handshake
- that failure happens before HTTP routing, so there is no HTTP `401` or `403` payload
- operators should rely on control-plane TLS handshake logs and audit events for diagnosis

### Admin-plane configuration guidance

Preferred rollout order:

1. Start with loopback-bound `auth_token` only for local development or migration compatibility.
2. Move to `auth.bearer_tokens[]` with explicit `viewer` / `operator` / `admin` roles.
3. Add IP allowlisting for the admin network.
4. Require control API mTLS in production.
5. Enable audit output and retain it separately from request-path logs.

Compatibility guidance:

- `observability.control_api.auth_token` remains supported intentionally to avoid operator lockout during migration
- the legacy token is treated as an `admin` identity so existing reload/restart automation keeps current behavior
- this is compatibility mode, not the target production design
- one-way boundary: a newer Spooky binary accepts legacy control API config, but older binaries reject configs that use the newer nested admin-plane fields because the config schema uses `deny_unknown_fields`

Recommended production posture:

- `tls.client_auth.mode: required`
- `auth.bearer_tokens[]` or role-bearing mTLS identity mapping
- `ip_allowlist.cidrs` restricted to the admin network
- audit enabled with JSON output
- health and readiness protected explicitly if deployment policy requires it

### Route families

The current route family includes:

- health
- ready
- runtime
- runtime history
- staged runtime operations: validate, preview, activate, rollback
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

### Runtime introspection contract

For operators, the highest-value reads are:

- `GET /admin/runtime`
- `GET /admin/runtime/history`
- `GET /admin/runtime/history/{generation}`

Those views should provide:

- active generation
- runtime history and rollback candidates
- backend health summary
- quota backend health summary
- watchdog state
- observability contract version
- audit schema version
- dashboard and documentation references
- recent admin actions

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

Operator rule:

- use metrics for trend, rate, and alerting
- use the control API for current state and generation-aware introspection

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

## Audit Stream

The audit stream is the control-plane history surface.

Use it when you need:

- actor attribution
- authn and authz failure history
- attempt versus result history for runtime operations
- reasoned failure records for restart, activate, rollback, reload, or cert reload

The audit stream is low-cardinality and operator-oriented. It is not a request-body or request-header log.

## Runtime View Contract

All control-plane services should depend on the same runtime model:

- active runtime generation
- shared runtime services
- generation-owned state where relevant

This ensures:

- metrics and control API describe the same active generation
- restart and reload actions act on the same authoritative runtime handle
- backend lifecycle state is rendered from one canonical inventory

### Observability package entry point

`GET /admin/runtime` and the runtime history reads are also the operator entry point into the
packaged observability bundle.

The runtime/control-plane views now expose:

- current active generation
- observability contract version
- audit schema version
- backend health summary
- quota backend health summary
- recent tracked admin/runtime actions when history exists
- dashboard definition references
- documentation references

This is what lets operators move from:

- metric
- to dashboard
- to runtime snapshot
- to generation history
- to audit attribution

without changing vocabulary.

These references are repository asset paths, not UI URLs. Operators and automation should treat
them as stable package identifiers that can be mapped into Grafana imports, runbooks, or internal
control-plane tooling without assuming one frontend.

## Authentication and Access Model

### Control API

Privileged control API routes must be authorized through the admin-plane security contract above.

Supported authentication shapes are:

- bearer token only
- mTLS only
- mTLS + bearer token

Health and readiness routes are intentionally treated differently from privileged administration routes and may be left unauthenticated depending on configuration and deployment pattern.

Minimum role requirements are:

- `viewer` for runtime snapshot and generation history reads
- `operator` for validate, preview, activate, rollback, reload, and cert reload
- `admin` for restart

Example postures:

- local dev: bearer token only on loopback
- transitional admin network: mTLS optional plus `viewer` token for read-only automation
- production: mTLS required plus `operator` / `admin` identities, IP allowlisting, and audit enabled

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
- what observability package version and audit schema version the node is serving
- what recent admin actions have been recorded

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
- [Observability Operator Bundle](observability-bundle.md)
- [Reload and Drain](reload-and-drain.md)
- [Metrics and Alerts](metrics-and-alerts.md)
- [Control API Reference](../reference/control-api-reference.md)
