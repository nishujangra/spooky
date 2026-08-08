# Distributed Quota And Advanced Rate-Limit Policy Contract

This document defines the contract for first-class distributed quota and
advanced rate-limit policy in Spooky.

The runtime now ships:

- scoped per-instance rate limiting
- distributed quota policy with route, tenant, token, and client selectors
- burst and sustained quota contracts
- Redis-backed distributed counters
- explicit fail-open or fail-closed backend behavior
- bounded local fallback when it is configured explicitly

Use [Distributed Quota](../operations/distributed-quota.md) for production
configuration, rollout guidance, and operator interpretation. This page remains
the semantic contract that config, admission behavior, metrics, and control API
output must continue to follow.

## Goals

- separate quota enforcement from overload control
- support route-, tenant-, token-, and client-scoped policy, including
  composite selectors
- support both short-window burst control and long-window sustained control
- support distributed counters with explicit consistency guarantees
- make denial and backend-failure outcomes operator-visible and stable

## Non-Goals

- replacing local overload controls with a distributed backend
- building a generic policy engine
- promising multi-region globally serializable counters
- introducing more selector dimensions than current product needs

## Quota Versus Overload

Quota policy and overload policy are separate subsystems with different
operational intent.

### Quota policy

Quota policy is an abuse-control and commercial-contract layer.

It exists to answer:

- how much traffic is this tenant, token, client, or route allowed to consume
- has a contractual burst or sustained limit been exceeded
- should this request be denied even if the proxy and backend fleet are healthy

Quota decisions are driven by policy and counter state. They are not a signal
that the proxy is saturated.

### Overload policy

Overload policy is a local safety layer.

It exists to answer:

- is this proxy or backend path under resource pressure
- must the request be shed to preserve availability

Overload decisions are driven by in-process resource pressure and health state.
They must remain local to the proxy runtime and must not depend on a distributed
counter backend.

### Required separation

Future implementation must preserve these rules:

1. Quota denial and overload shedding must use different reason vocabularies.
2. Quota exhaustion must not be emitted as overload.
3. Counter-backend failure must not be emitted as overload.
4. Local overload controls must still work when the distributed quota backend is
   unavailable.

## Selector Model

Quota policy evaluates a request against a canonical selector key. The supported
selector dimensions are:

- `route`
- `tenant`
- `token`
- `client`

### Selector dimensions

`route`
: The canonical upstream or route identity selected by routing. Quota policy is
  attached to the routed target, not to the raw request path string alone.

`tenant`
: A normalized tenant identifier derived from authenticated request context or a
  configured trusted identity source.

`token`
: A normalized token identifier derived from bearer-token or equivalent auth
  context. The raw secret value must not be emitted in logs or metrics.

`client`
: A normalized client identity. This may come from configured client-id
  headers, mTLS identity, trusted proxy identity, or canonical peer/client-IP
  extraction depending on policy.

### Composite selectors

Policies may target one dimension or a composite of dimensions. Supported
composite examples include:

- `route + tenant`
- `route + token`
- `route + client`
- `tenant + client`
- `route + tenant + token`

Composite selectors are treated as first-class policy keys, not as a chain of
independent checks.

### Selector normalization rules

Future implementation must satisfy these invariants:

1. Selector extraction must be deterministic for the same request context.
2. Composite keys must be built from normalized component identities in stable
   field order.
3. Missing selector identity must not silently collapse to another identity such
   as `unknown` unless the policy explicitly allows that behavior.
4. Sensitive selector values, especially token-derived identities, must be
   redacted or hashed before they appear in logs, metrics, or the control API.

## Contract Model

Each quota policy defines one or more windows over the same selector key. The
initial contract model is:

- `burst`
- `sustained`

### Burst contract

The burst contract protects short-term spikes. It is a short window with a
relatively higher allowance that answers:

- can this selector consume a sudden spike right now

### Sustained contract

The sustained contract protects long-term fairness and commercial limits. It is
a longer window with a lower average allowance that answers:

- can this selector continue consuming traffic at this rate over time

### Combined evaluation rules

Future implementation must evaluate all configured windows for a policy as one
logical decision.

That means:

1. a request is allowed only if every configured window allows it
2. a request denied by burst must not consume sustained allowance
3. a request denied by sustained must not consume burst allowance
4. the counter backend must evaluate and update the windows atomically for a
   single request decision

## Backend Failure Semantics

Distributed quota evaluation requires an explicit backend-failure policy.
Backend-failure behavior must never be implicit.

The supported modes are:

- `fail_open`
- `fail_closed`

### `fail_open`

If quota evaluation cannot complete because the distributed backend is
unavailable, times out, or returns an internal evaluation error:

- the request is allowed to continue
- the request is marked as quota-degraded in observability surfaces
- the backend failure must be recorded in metrics, logs, and control API state

`fail_open` protects availability at the cost of temporary contract drift.

### `fail_closed`

If quota evaluation cannot complete because the distributed backend is
unavailable, times out, or returns an internal evaluation error:

- the request is denied
- the deny reason must name the backend failure cause explicitly
- the denial must not be emitted as overload

`fail_closed` protects contract strictness at the cost of availability.

### HTTP status expectations

Future implementation must separate exhausted quota from failed quota
evaluation:

- quota exhaustion returns `429 Too Many Requests`
- fail-closed backend evaluation failure returns `503 Service Unavailable`

This prevents operators and clients from confusing policy exhaustion with
backend dependency failure.

## Exact Deny-Reason Vocabulary

The deny-reason vocabulary for distributed quota policy is locked to the
following canonical values:

- `burst_quota_exhausted`
- `sustained_quota_exhausted`
- `selector_identity_missing`
- `selector_identity_invalid`
- `backend_timeout`
- `backend_unavailable`
- `backend_error`

### Meaning of each reason

`burst_quota_exhausted`
: The request would exceed the configured short-window burst contract.

`sustained_quota_exhausted`
: The request would exceed the configured long-window sustained contract.

`selector_identity_missing`
: The policy requires one or more selector dimensions that could not be derived
  from the request context.

`selector_identity_invalid`
: The required selector dimension was present but failed normalization or trust
  validation.

`backend_timeout`
: The distributed quota backend did not answer within the configured evaluation
  timeout.

`backend_unavailable`
: The distributed quota backend could not be reached or was not ready for
  evaluation.

`backend_error`
: The distributed quota backend responded, but the evaluation failed due to an
  internal or protocol-level error.

No other deny-reason strings may be introduced for this subsystem without
updating this contract first.

## Consistency Expectations For Distributed Counters

Spooky's distributed quota layer is intended to provide strong-enough
per-selector contract enforcement, not globally serialized traffic accounting
across every deployment topology.

Future implementation must meet these consistency expectations:

1. Counter evaluation for one request against one policy key must be atomic
   across all configured windows for that key.
2. Two concurrent requests for the same policy key must not both be allowed if
   doing so would exceed a limit that a single atomic backend decision could
   prevent.
3. A successful allow decision must commit the corresponding counter update as
   part of the same backend operation.
4. The proxy must not depend on local wall-clock time as the source of truth for
   distributed window rollover if the backend provides the authoritative timing.
5. Cross-key fairness is best-effort. The contract is per selector key, not a
   globally serializable ledger across unrelated keys.
6. Multi-region deployments may observe replication lag or region-local drift
   unless the selected backend guarantees stronger coordination. This is an
   operator tradeoff, not a hidden implementation detail.

## Required Operator Visibility

When this feature is implemented, the following must be visible:

- which quota policy matched
- which selector dimensions were used
- whether the request was allowed, quota-denied, or degraded by backend failure
- the exact deny reason when denied
- whether backend failure handling is `fail_open` or `fail_closed`
- whether the distributed quota backend is healthy, degraded, or unavailable

## Implementation Guardrails

The implementation that follows this contract should stay within these
boundaries:

- keep overload control local
- keep selector extraction centralized and reusable
- keep distributed backend logic behind a narrow counter-evaluation interface
- do not add more selector dimensions or quota-window types until there is a
  concrete product need

That keeps the first implementation useful without turning the proxy into a
generic external policy platform.
