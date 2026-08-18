# Control API Reference

This page documents the current operator-facing control-plane endpoints and their intended use.

## Open These First

Use this table when you need the fastest runtime-introspection path:

| Need | Endpoint |
|---|---|
| current runtime state, backend health, quota backend state, watchdog state, observability package metadata | `GET /admin/runtime` |
| retained generations, rollback candidates, and recent runtime operations | `GET /admin/runtime/history` |
| one generation's retained record and related entries | `GET /admin/runtime/history/{generation}` |
| check whether a candidate config is valid and compatible | `POST /admin/runtime/validate` |
| dry-run a change and record it in history | `POST /admin/runtime/preview` |
| commit a compatible runtime-managed change | `POST /admin/runtime/activate` |

## Scope

The control API is a privileged admin surface. It should be treated as operator-only infrastructure, not as a public application endpoint.

Current endpoint family:

- health
- readiness
- runtime snapshot
- staged activation: validate, preview, activate
- rollback
- generation history
- certificate reload
- full config reload
- restart request

## Protocol

The control API uses **HTTP/1.1 over TLS**. HTTP/2 is not supported.

When using curl, pass `--http1.1` explicitly — curl negotiates h2 by default when connecting to a TLS endpoint and the server will reject the connection:

```bash
curl -k --http1.1 https://<address>:<port>/...
```

The `-k` flag skips certificate verification for self-signed certs.

## Security Expectations

- bind to loopback or a strongly isolated admin network whenever possible
- prefer `mTLS required` plus either a bearer token or an mTLS-derived role-bearing identity in production
- avoid broad public exposure even when authentication is enabled
- if IP allowlisting is configured, source address policy is enforced before bearer-token validation
- do not trust `X-Forwarded-For` or similar proxy headers unless a future deployment-specific policy layer explicitly enables that behavior

## Authentication

The admin plane supports these authentication shapes:

- bearer token only
- mTLS only
- mTLS + bearer token

Recommended production posture:

- `mTLS required`
- bearer token or another role-bearing admin identity
- isolated admin network exposure

Authentication and authorization are separate concerns:

- authentication proves who the caller is
- authorization decides whether the caller has `viewer`, `operator`, or `admin`

Bearer-token form:

```http
Authorization: Bearer <token>
```

Compatibility note:

- `observability.control_api.auth_token` remains supported as the legacy single-token admin credential
- the legacy token is mapped internally to a static admin identity so existing operators keep current restart/reload privileges during migration
- the legacy token runs in compatibility mode; it preserves behavior, but it is not the recommended production posture
- new deployments should prefer `observability.control_api.auth.bearer_tokens[]` with explicit roles
- compatibility boundary: a new Spooky binary accepts legacy `auth_token` configs, but an older binary will reject configs that use the newer nested control-plane fields because `ControlApi` uses strict `deny_unknown_fields`

Role model:

- `viewer`: runtime snapshot and history reads
- `operator`: `viewer` plus validate, preview, activate, rollback, reload, and cert reload
- `admin`: `operator` plus restart and future destructive admin actions

This role matrix is the intended contract for control-plane implementation.

## Route Classification And Role Matrix

The admin plane classifies routes in three groups:

- unauthenticated or separately configurable: `/health`, `/ready`
- read-only privileged: `/admin/runtime`, `/admin/runtime/history`, `/admin/runtime/history/{generation}`
- mutating privileged: `/admin/runtime/validate`, `/admin/runtime/preview`, `/admin/runtime/activate`, `/admin/runtime/rollback`, `/admin/runtime/reload`, `/admin/runtime/reload-certs`, `/admin/runtime/restart`

Minimum role requirements:

| Route | Method | Classification | Minimum role |
| --- | --- | --- | --- |
| `/health` | `GET` | unauthenticated or separately configurable | none |
| `/ready` | `GET` | unauthenticated or separately configurable | none |
| `/admin/runtime` | `GET` | read-only privileged | `viewer` |
| `/admin/runtime/history` | `GET` | read-only privileged | `viewer` |
| `/admin/runtime/history/{generation}` | `GET` | read-only privileged | `viewer` |
| `/admin/runtime/validate` | `POST` | mutating privileged | `operator` |
| `/admin/runtime/preview` | `POST` | mutating privileged | `operator` |
| `/admin/runtime/activate` | `POST` | mutating privileged | `operator` |
| `/admin/runtime/rollback` | `POST` | mutating privileged | `operator` |
| `/admin/runtime/reload` | `POST` | mutating privileged | `operator` |
| `/admin/runtime/reload-certs` | `POST` | mutating privileged | `operator` |
| `/admin/runtime/restart` | `POST` | mutating privileged | `admin` |

Contract rules:

- `viewer` is the minimum privileged read role
- `operator` is the minimum non-restart mutation role
- `admin` is required for restart
- health and readiness may remain unauthenticated, but deployments may choose to protect them separately
- implementation should distinguish invalid authentication from insufficient role

## Response Contract

Privileged routes distinguish authentication failure from authorization failure:

- `401 Unauthorized`: missing authentication or invalid authentication
- `403 Forbidden`: authenticated caller is under-scoped, or the source-address policy rejected the request

Representative reasons returned in JSON payloads:

- `missing_authentication`
- `invalid_bearer_token`
- `insufficient_role`
- `source_ip_not_allowed`

When control API mTLS is configured as `required`, missing or invalid client certificates are rejected during the TLS handshake before HTTP routing. That failure does not produce an HTTP `401` or `403` response; the connection is terminated during handshake, and the server emits a control-plane TLS failure log with a stable client-auth reason code.

## Configuration Patterns

### Bearer-Only Local Dev

Use this for loopback-only development or local automation:

This is compatibility-friendly and intentionally simple, but it is not the recommended long-term production posture.

```yaml
observability:
  control_api:
    enabled: true
    address: "127.0.0.1"
    port: 9890
    auth_token: "change-me-local-dev"
```

### mTLS Optional With Viewer Token

Use this when you want to start accepting client certificates without making them mandatory yet:

This is a transitional posture for gradual hardening. It is safer than legacy single-token mode, but still weaker than required mTLS.

```yaml
observability:
  control_api:
    enabled: true
    address: "127.0.0.1"
    port: 9902
    tls:
      client_auth:
        mode: optional
        ca_file: "/etc/spooky/pki/admin-ca.pem"
    auth:
      bearer_tokens:
        - token: "viewer-token"
          role: viewer
          actor_id: "ops-readonly"
```

### mTLS Required With Operator/Admin Identities

Recommended production posture:

```yaml
observability:
  control_api:
    enabled: true
    required: true
    address: "10.0.10.5"
    port: 9902
    tls:
      client_auth:
        mode: required
        ca_file: "/etc/spooky/pki/admin-ca.pem"
    auth:
      bearer_tokens:
        - token: "operator-token"
          role: operator
          actor_id: "ops-automation"
        - token: "admin-token"
          role: admin
          actor_id: "platform-admin"
      identity_source:
        kind: "mtls_subject_cn"
        role_attribute: "OU"
    ip_allowlist:
      cidrs:
        - "10.0.10.0/24"
    audit:
      enabled: true
      format: json
      sink: log
```

## Endpoints

### `GET /health`

Purpose:

- liveness check
- watchdog state visibility

Expected use:

- load balancer or platform liveness probe
- operator sanity check

### `GET /ready`

Purpose:

- readiness state for serving traffic

Expected use:

- deployment orchestration
- maintenance and rollout checks

### `GET /admin/runtime`

Purpose:

- runtime snapshot for operators

Minimum role:

- `viewer`

Typical contents include:

- worker and runtime state
- key counters
- admission state
- backend health summary
- quota backend health summary
- observability package metadata
- recent admin actions when available
- dashboard and documentation references for the shipped operator bundle

Expected use:

- debugging
- rollout validation
- incident response

The `observability` block is the packaged runtime-introspection entry point for operators. The high-signal fields are:

- `contract_version`
- `audit_schema_version`
- `current_generation`
- `dashboard_packages`
- `documentation`
- `backend_health_summary`
- `quota_backend_health_summary`
- `recent_admin_actions`

### `POST /admin/runtime/validate`

Purpose:

- parse and validate a candidate config, and report whether it could be activated — without touching the running runtime

Returns `200` with a plan describing the candidate generation, a per-domain diff, and any rejected changes. A config that cannot be activated still returns `200`; inspect `rejected_changes` and `candidate_status` rather than relying on the status code.

Accepts the same optional body fields as `/admin/runtime/reload`.

Expected use:

- CI gating on config changes before a deploy
- confirming a config is loadable before scheduling a maintenance window

Minimum role:

- `operator`

### `POST /admin/runtime/preview`

Purpose:

- same planning work as validate, recorded in generation history as an operator preview

Returns `200` with the same plan shape as validate. Neither endpoint mutates the active generation.

Expected use:

- operator dry-run immediately before an activation, when you want the attempt in the audit trail

Minimum role:

- `operator`

### `POST /admin/runtime/activate`

Purpose:

- stage and commit a config change, returning the structured activation result

Returns `202` on success. Failures are classified rather than collapsed into `500`:

| Status | Meaning |
| --- | --- |
| `400` | the candidate config is invalid |
| `409` | conflict — a stale `expected_generation`, or changes that require a restart |
| `500` | resource preparation failed, or the runtime swap itself failed |

Accepts the same optional body fields as `/admin/runtime/reload`.

Expected use:

- the preferred activation path — prefer this over the legacy `/reload` shortcut, since it returns the full diff, rejection detail, and generation history entry

Minimum role:

- `operator`

### `POST /admin/runtime/rollback`

Purpose:

- restore a previously retained runtime generation

Required request body:

| Field | Type | Purpose |
| --- | --- | --- |
| `target_generation` | integer | The retained generation to restore. Required. |
| `expected_active_generation` | integer | Reject with `409` unless this matches the active generation. |
| `requested_by` | string | Recorded in generation history for audit. |
| `reason` | string | Recorded in generation history for audit. |

Returns `202` on success. Failures:

| Status | Meaning |
| --- | --- |
| `404` | the target generation is not retained (unknown generation) |
| `409` | the target is retained but not rollback-eligible, or the active generation moved |
| `500` | resource preparation failed, or the rollback swap itself failed |

Use `GET /admin/runtime/history` first to pick a target whose `rollback_candidate` is `true`.

Minimum role:

- `operator`

Example:

```bash
curl -k --http1.1 -X POST https://127.0.0.1:9890/admin/runtime/rollback \
  -H "Authorization: Bearer <token>" \
  -H "content-type: application/json" \
  -d '{"target_generation": 3}'
```

### `GET /admin/runtime/history`

Purpose:

- list retained runtime generations and the recorded history of control-plane operations

Response shape:

| Field | Type | Purpose |
| --- | --- | --- |
| `active_generation` | integer | The generation currently serving traffic. |
| `retained_generations` | array | Retained generation records — the state needed to choose a rollback target. |
| `entries` | array | Operation log (validate, preview, activate, rollback), newest first. |

Each entry in `retained_generations`:

| Field | Type | Purpose |
| --- | --- | --- |
| `generation` | integer | The generation number. |
| `status` | string | One of `active`, `previous`, `failed_prepare`, `rolled_back`, `superseded`. |
| `rollback_candidate` | bool | Whether `/admin/runtime/rollback` will accept this generation as a target. |
| `has_bundle` | bool | Whether the runtime bundle is still retained. A rollback target needs `true`. |
| `note` | string | Present only when there is explanatory detail, e.g. why a staged prepare failed. |

`status: failed_prepare` records a candidate generation that was never successfully prepared; it has `has_bundle: false` and carries the failure reason in `note`.

Expected use:

- choosing a safe rollback target
- auditing who changed runtime config, when, and from which config source
- diagnosing why a staged activation never committed
- correlating runtime operations with audit and observability views

Minimum role:

- `viewer`

### `GET /admin/runtime/history/{generation}`

Purpose:

- the retained-generation record and operation entries for a single generation

Returns `200` with `generation`, a single `retained_generation` object (same shape as above), and the `entries` recorded against it. Returns `404` if that generation is not retained.

Minimum role:

- `viewer`

### `POST /admin/runtime/reload`

Legacy shortcut. Prefer `POST /admin/runtime/activate`, which returns the full diff and rejection detail.

Purpose:

- reload the full config from disk and apply changes to upstreams, backends, policies, timeouts, and `log.level`

Config source:

- with no request body, the reload re-reads the **currently active runtime config source**
- on a freshly started process that source is the path passed at startup, but activating an alternate `config_path` makes that file the active source for every later reload
- pass `config_path` in the body to read a different file; a successful activation makes that path the new active source

Important scope note:

- listener bind addresses, control API bind, and metrics bind cannot change without a restart
- log format/file settings, tracing config (`observability.tracing.*`), and `performance.control_plane_threads` also require a restart (a reload changing them is rejected); `log.level`, however, is applied live
- in-flight requests on the old config complete normally; new requests use the new config immediately

Expected use:

- adding or removing backends
- changing load balancing, timeouts, resilience, or routing policy at runtime

Minimum role:

- `operator`

Optional request body:

| Field | Type | Purpose |
| --- | --- | --- |
| `config_path` | string | Read this config file instead of the active source. On success it becomes the new active source. |
| `expected_generation` | integer | Reject with `409` unless this matches the active generation (optimistic concurrency). |
| `requested_by` | string | Recorded in generation history for audit. |
| `reason` | string | Recorded in generation history for audit. |

Example:

```bash
curl -k --http1.1 -X POST https://127.0.0.1:9890/admin/runtime/reload \
  -H "Authorization: Bearer <token>"
```

Activating an alternate config file:

```bash
curl -k --http1.1 -X POST https://127.0.0.1:9890/admin/runtime/reload \
  -H "Authorization: Bearer <token>" \
  -H "content-type: application/json" \
  -d '{"config_path": "/etc/spooky/canary.yaml"}'
```

### `POST /admin/runtime/reload-certs`

Purpose:

- reload listener certificate and related trust material for **new handshakes**

Important scope note:

- this is not full config hot reload
- existing sessions keep their already-negotiated certificate and auth state

Expected use:

- listener certificate rotation
- listener trust-material refresh

Minimum role:

- `operator`

### `POST /admin/runtime/restart`

Purpose:

- request a controlled restart/drain workflow through the watchdog coordinator

Expected use:

- operational restart requests
- orchestrated maintenance flow

Minimum role:

- `admin`

## Audit Configuration And Event Shape

The control API audit stream is the operator history surface for admin-plane actions.

Recommended production posture:

- enable audit
- keep it separate from request-path logs
- use JSON output
- retain it on a protected sink

Example:

```yaml
observability:
  control_api:
    audit:
      enabled: true
      format: json
      sink: log
```

Current audit schema version:

- `v1`

The stable top-level event fields are:

- `schema_version`
- `event_id`
- `event_type`
- `time_unix_ms`
- `request_id`
- `trace_id`
- `span_id`
- `listener`
- `actor`
- `action`
- `target`
- `generation`
- `result`
- `reason`
- `failure_class`
- `peer_addr`
- `authn`

Use audit when you need:

- actor attribution
- authn and authz failure history on the admin plane
- attempt versus result correlation for validate, preview, activate, rollback, reload, restart, or cert reload
- a reasoned record of why a control-plane action failed or was denied

## Operator Notes

- use cert reload for cert-only changes
- use `validate` (or `preview`) then `activate` for backend, policy, timeout, or routing changes that don't require rebinding listeners — `activate` reports the diff and classifies failures, unlike the legacy `/reload`
- pass `expected_generation` on `activate` and `rollback` so a concurrent change fails with `409` instead of silently overwriting
- check `GET /admin/runtime/history` for a target with `rollback_candidate: true` before calling `rollback`
- use `GET /admin/runtime` when metrics or dashboards show trend but you need the current backend, quota, watchdog, or observability-package state
- use audit for actor attribution and attempt versus result history
- use drain-and-restart when listener addresses or control API/metrics bind must change
- keep rollback available before using restart-triggering control-plane actions in production
- all curl invocations must use `--http1.1` — the control API does not support HTTP/2

## Related Pages

- [Metrics Reference](metrics-reference.md)
- [Production Readiness](../operations/production-readiness.md)
- [Operations Runbook](../operations/runbook.md)
