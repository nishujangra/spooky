# Distributed Quota Policy And Operations

This page documents how to configure, roll out, and operate Spooky's
distributed quota layer.

Use this together with:

- [Configuration Reference](../configuration/reference.md) for the canonical
  schema
- [Configuration Defaults](../configuration/defaults.md) for omitted-field
  behavior
- [Distributed Quota Contract](../architecture/quota-policy-contract.md) for
  the locked semantics behind selectors, deny reasons, and backend behavior
- [Metrics Reference](../reference/metrics-reference.md) for the exported
  metric families

## What Distributed Quota Is For

Distributed quota is the abuse-control and contract-enforcement layer.

Use it when you need:

- route-, tenant-, token-, or client-scoped request contracts
- the same counters shared across multiple Spooky instances
- explicit burst and sustained policy windows
- stable operator-visible outcomes for exhaustion, backend degradation, and
  fail-open or fail-closed behavior

Do not treat distributed quota as an overload-control system. Local overload
controls such as adaptive admission, route queue caps, and inflight shedding
remain separate and should stay enabled.

## Policy Shape

The quota config lives under `resilience.quota`.

Key top-level fields:

- `enabled`
- `enforcement`
  - `enforce`: deny matching requests
  - `shadow`: record would-deny outcomes without blocking traffic
- `backend_failure_policy`
  - `fail_open`
  - `fail_closed`
- `backend`
  - `in_memory`
  - `redis`
- `local_fallback`
  - optional bounded in-memory degraded-mode fallback for Redis only
- `policies`

Each policy contains:

- `name`
- `route_allowlist`
- `selector`
  - `route`
  - `tenant`
  - `token`
  - `client`
- `burst`
- `sustained`

## Policy Examples

### Route + tenant contract

Use this when each tenant gets an independent contract per routed upstream.

```yaml
resilience:
  quota:
    enabled: true
    enforcement: enforce
    backend_failure_policy: fail_closed
    backend:
      kind: redis
      url: "redis://redis-quota.service.consul:6379/0"
      key_prefix: "spooky:quota:prod"
      connect_timeout_ms: 250
      command_timeout_ms: 100
      max_inflight: 1024
    policies:
      - name: "payments-tenant-contract"
        route_allowlist: ["payments"]
        selector:
          route: true
          tenant:
            key: "header:x-tenant-id"
        burst:
          requests: 100
          window_secs: 1
        sustained:
          requests: 5000
          window_secs: 60
```

### Route + tenant + token + client contract

Use this when a single tenant can issue multiple bearer tokens and you need to
constrain both the token and the caller identity.

```yaml
resilience:
  quota:
    enabled: true
    enforcement: enforce
    backend_failure_policy: fail_open
    backend:
      kind: redis
      url: "redis://redis-quota.service.consul:6379/0"
      key_prefix: "spooky:quota:prod"
      connect_timeout_ms: 250
      command_timeout_ms: 100
      max_inflight: 1024
    local_fallback:
      key_prefix: "spooky:quota:fallback:prod"
      max_entries: 50000
    policies:
      - name: "tenant-token-client-contract"
        route_allowlist: ["api"]
        selector:
          route: true
          tenant:
            key: "header:x-tenant-id"
          token:
            key: "bearer_token"
          client:
            key: "client_ip"
        burst:
          requests: 20
          window_secs: 1
        sustained:
          requests: 1200
          window_secs: 60
```

### Shadow-mode migration policy

Use this to observe outcomes before turning on enforcement.

```yaml
resilience:
  quota:
    enabled: true
    enforcement: shadow
    backend_failure_policy: fail_open
    backend:
      kind: redis
      url: "redis://redis-quota.service.consul:6379/0"
      key_prefix: "spooky:quota:shadow"
      connect_timeout_ms: 250
      command_timeout_ms: 100
      max_inflight: 512
    policies:
      - name: "legacy-tenant-shadow"
        route_allowlist: ["api"]
        selector:
          route: true
          tenant:
            key: "header:x-tenant-id"
        burst:
          requests: 50
          window_secs: 1
```

## Redis Backend Setup

Redis is the first-class distributed backend. Spooky evaluates burst and
sustained windows in one atomic Redis script invocation.

### Recommended baseline

- use a dedicated Redis deployment or logical database for quota traffic
- keep Redis close to the Spooky fleet in network terms
- use a distinct `key_prefix` per environment
- size `max_inflight` to protect Redis from unbounded concurrent evaluation
- keep `connect_timeout_ms` and `command_timeout_ms` tight so degraded modes are
  detected quickly

### Operational expectations

- Spooky stores one key per policy-selector-window bucket
- Redis key TTL tracks the window boundary plus a small grace interval
- the protocol version is explicit so script/result changes can be rolled out
  intentionally
- a single request decision updates all configured windows atomically

### Example production posture

```yaml
resilience:
  quota:
    enabled: true
    enforcement: enforce
    backend_failure_policy: fail_open
    backend:
      kind: redis
      url: "redis://10.20.30.40:6379/0"
      key_prefix: "spooky:quota:prod:cluster-a"
      connect_timeout_ms: 200
      command_timeout_ms: 75
      max_inflight: 2048
    local_fallback:
      key_prefix: "spooky:quota:fallback:prod:cluster-a"
      max_entries: 100000
    policies:
      - name: "default-api-contract"
        route_allowlist: ["api"]
        selector:
          route: true
          tenant:
            key: "header:x-tenant-id"
        burst:
          requests: 40
          window_secs: 1
        sustained:
          requests: 2400
          window_secs: 60
```

## Failure-Mode Guidance

Distributed quota has three separate operational choices:

- `enforcement`
- `backend_failure_policy`
- optional `local_fallback`

### `fail_open`

Choose `fail_open` when availability is more important than temporary contract
drift.

Operational effect:

- quota backend failure does not block the request
- metrics and runtime state mark the backend as degraded
- overload shedding remains separate and can still return its own 503s

Recommended for:

- user-facing APIs where temporary over-admission is acceptable during backend
  incidents
- initial rollout and shadow-mode phases

### `fail_closed`

Choose `fail_closed` when contract strictness is more important than temporary
availability.

Operational effect:

- quota backend failure rejects the request
- the request returns a quota-specific 503, not an overload 503
- the deny reason reflects backend timeout, unavailability, or error

Recommended for:

- paid contract enforcement where over-consumption is unacceptable
- narrow internal control-plane routes with strict commercial or abuse limits

### Bounded local fallback

`local_fallback` is only supported with the Redis backend.

Use it when:

- you want outage survival without fully abandoning quota checks
- you accept temporary per-instance divergence during the outage window

Rules:

- fallback is only attempted for outage-style backend failures such as timeout
  or unavailability
- fallback is not used for protocol/configuration/backend logic errors
- fallback capacity is bounded by `max_entries`
- degraded-mode metrics and runtime state remain explicit even when the request
  is allowed locally

Do not enable local fallback unless you have decided that temporary per-instance
counter divergence is acceptable.

## Migration From Scoped Rate Limiting

Spooky's older `resilience.scoped_rate_limits` layer remains useful for
single-instance or purely local throttling, but distributed quota is the
preferred path for shared contract enforcement.

### When to keep scoped rate limiting

Keep scoped rate limiting when:

- limits are intentionally local to one proxy instance
- you do not want a Redis dependency
- the policy is operationally simple and does not need shared counters

### When to migrate

Migrate to distributed quota when:

- you need one quota shared across multiple Spooky instances
- you need burst and sustained windows together
- you need consistent operator-visible backend health and deny semantics
- the limit is part of a customer or abuse-control contract rather than local
  overload posture

### Recommended migration order

1. Recreate the existing scoped rule as a quota policy in `shadow` mode.
2. Verify selector extraction, route scoping, and metric cardinality.
3. Verify Redis health and timeout behavior under load.
4. Decide `fail_open` versus `fail_closed`.
5. Enable `local_fallback` only if temporary per-instance drift is acceptable.
6. Switch the policy to `enforce`.
7. Remove the legacy scoped rule after shadow and enforced outcomes match your
   expectations.

### Mapping guidance

- scoped `Route` rules usually become selector `route: true`
- scoped `Tenant` rules usually become selector `tenant.key: authority` or a
  trusted tenant header
- scoped `Token` rules usually become selector `token.key: bearer_token`
- scoped `Client` rules usually become selector `client.key: client_ip` or
  `peer_ip`

## Operator Interpretation

### HTTP outcomes

- `429 Too Many Requests`
  - contract exhaustion
  - `burst_quota_exhausted`
  - `sustained_quota_exhausted`
  - `selector_identity_missing`
  - `selector_identity_invalid`
- `503 Service Unavailable`
  - quota backend failure under `fail_closed`
  - local overload shedding

Do not assume every 503 is overload. Check the quota reason and backend health
state before concluding that the proxy is saturated.

### Deny reasons

Canonical quota reasons:

- `burst_quota_exhausted`
- `sustained_quota_exhausted`
- `selector_identity_missing`
- `selector_identity_invalid`
- `backend_timeout`
- `backend_unavailable`
- `backend_error`

Interpret them as:

- `burst_quota_exhausted`: short-window spike blocked
- `sustained_quota_exhausted`: long-window contract blocked
- `selector_identity_missing`: the policy matched the route but required
  identity data was absent
- `selector_identity_invalid`: identity input was present but unusable
- `backend_timeout`: Redis did not answer within `command_timeout_ms`
- `backend_unavailable`: Redis or fallback capacity was unavailable
- `backend_error`: protocol mismatch, script error, or another non-retryable
  evaluation fault

### Metrics

Primary quota metric families:

- `spooky_quota_policy_outcomes_total{policy,decision,reason,selector_dimensions,backend_mode}`
- `spooky_quota_backend_health_total{backend_mode,reason}`

Read them this way:

- alert on `decision="denied"` growth only when it is unexpected for the
  selected policy
- alert on `decision="failed_open"` or `decision="failed_closed"` immediately
  because they indicate backend trouble
- treat `backend_mode="redis_local_fallback_backend_timeout"` or similar
  degraded backend labels as incidents even if the request decision is
  `allowed`

### Runtime and control API state

The runtime snapshot exposes:

- configured quota policies
- active backend type
- backend availability
- degraded status
- fail-open or fail-closed mode
- recent backend errors

Interpretation:

- `availability="available"`: backend has recent successful observations
- `availability="unknown"`: Redis is configured but no live observation has
  happened yet
- `availability="degraded"`: initialization failed, fallback is active, or live
  backend errors are being recorded
- `degraded=true` with `active_backend="redis"`: Redis is the intended backend
  and the runtime currently considers it unhealthy
- `degraded=true` with a fallback-flavored backend mode: traffic is currently
  being evaluated through bounded local fallback

## Rollout Checklist

1. Start in `shadow` mode.
2. Confirm selector dimensions are stable and do not explode cardinality.
3. Validate that Redis timeouts and inflight caps are realistic for production
   latency.
4. Confirm control API runtime snapshot shows the expected policies and backend
   posture.
5. Build dashboards on quota outcome and backend health metrics before enabling
   enforcement.
6. Switch to `enforce`.
7. Remove superseded `scoped_rate_limits` rules after the migration is stable.

## Related Pages

- [Distributed Quota Contract](../architecture/quota-policy-contract.md)
- [Configuration Reference](../configuration/reference.md)
- [Configuration Defaults](../configuration/defaults.md)
- [Metrics and Alerts](metrics-and-alerts.md)
- [Failure Modes](failure-modes.md)
