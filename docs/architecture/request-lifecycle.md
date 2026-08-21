# Request Lifecycle

This document describes the canonical request path through Impulse. It explains the product flow that both ingress paths should follow after intake: shared admission, auth, routing, transport execution, response normalization, streaming guardrails, and outcome recording.

## Purpose

The request path should be understandable as one ordered flow:

1. intake
2. admission
3. auth
4. route resolution and backend selection
5. canonical request building
6. backend dispatch
7. response normalization
8. body streaming and guardrails
9. retry and hedge evaluation when needed
10. outcome recording

That flow should be shared conceptually across:

- QUIC ingress
- bootstrap compatibility ingress

The two paths differ in ingress and egress mechanics, not in core policy meaning.

## One Request, Two Ingress Paths

The product-level rule is simple:

- QUIC and bootstrap may parse requests differently
- they should reach the same internal policy decisions for the same logical request
- they should emit the same canonical observability reasons for the same outcome

If those paths disagree semantically, the shared boundary is in the wrong place.

## Main Ownership Boundaries

### `edge::quic_listener`

Owns ingress mechanics and request orchestration.

It is responsible for:

- accepting traffic
- creating request envelopes
- invoking shared policy layers in order
- coordinating transport execution
- writing results back to the client protocol

### `edge::runtime::connection`

Owns request-path shared policy and accounting helpers that are runtime concerns rather than ingress mechanics.

This includes:

- body guardrails
- outcome classification and recording
- stream terminal-state vocabulary

### `bridge`

Owns canonical request building and response normalization.

It is responsible for:

- upstream request construction
- host and forwarded-header policy application
- canonical response/header normalization
- websocket and upgrade helper logic

### `transport`

Owns backend protocol execution.

It is responsible for:

- runtime-selected H1/H2 dispatch
- connection reuse
- transport-level timeouts
- backend client rotation

### `lb`

Owns load-balancing substrate and pool accounting primitives.

It is not where request-path orchestration should live.

## Step 1: Intake

The request path begins in an ingress-specific intake layer.

### QUIC intake

QUIC intake owns:

- packet processing
- QUIC connection lifecycle
- HTTP/3 stream establishment
- request envelope creation from stream headers and body chunks

### Bootstrap intake

Bootstrap intake owns:

- HTTP request acceptance
- method/path/authority extraction
- compatibility-path validation
- websocket upgrade detection

By the end of intake, the system should have a canonical request envelope or request context, not a partially interpreted protocol object scattered across multiple branches.

## Step 2: Admission

Admission is the first shared policy gate.

Admission covers:

- quota and scoped rate-limit decisions
- overload and brownout shedding
- route-level policy rejection
- local auth prerequisites where applicable
- permit acquisition and admission execution checks

Admission should produce typed policy results, not direct response I/O decisions scattered across intake code.

Both QUIC forwarding and bootstrap compatibility code should route these checks through the same admission layer.

Quota and overload are intentionally separate:

- quota describes contract or entitlement enforcement
- overload describes runtime self-protection

They may both reject a request, but they should not collapse into one policy meaning.

## Step 3: Auth Decisions

If external auth is configured, the request enters the shared auth decision layer.

This layer owns:

- timeout handling
- fail-open vs fail-closed policy
- allow/deny/challenge/redirect mapping
- allowlist filtering
- header mutation validation and safety checks
- OIDC helper checks where configured

Ingress-specific code should only orchestrate the auth request and apply the shared auth decision result.

## Step 4: Route Resolution and Backend Selection

After admission and auth, the request enters the shared resolution pipeline.

Resolution covers:

- listener-aware route matching
- host and path-based route matching
- upstream lookup
- load-balancing strategy selection
- canonical load-balancing key extraction
- backend selection
- backend-selection logging and telemetry

The result of this phase should be a typed resolved target, not a mix of route, upstream, and backend values reconstructed later by dispatch code.

## Step 5: Canonical Request Building

Once a backend is selected, ingress code shapes the request into the canonical bridge input.

This phase covers:

- method/path/authority transfer
- body mode decisions
- host policy application
- forwarded-header policy application
- websocket tunnel request shaping

`bridge` owns the canonical request-building contract. Listener code should assemble inputs, not reimplement header policy.

## Step 6: Backend Dispatch

Backend dispatch means handing the canonical upstream request to transport.

Edge-owned dispatch logic still owns:

- retry and hedge orchestration
- upstream inflight coordination
- admission-related dispatch constraints
- backend-target bookkeeping for observations

But edge should not re-decide protocol execution details. At this point it should effectively say:

- send this canonical request to this backend

Transport decides how that backend is executed according to the runtime-selected backend transport kind.

## Step 7: Response Normalization

When an upstream response arrives, it is normalized before downstream emission.

This shared layer covers:

- hop-by-hop header stripping
- connection-token filtering
- trailer normalization and conversion
- content-length and content-type shaping
- HEAD/bodyless/no-content behavior

`bridge::response` is the canonical surface for this. QUIC and bootstrap should differ only in how they emit the normalized result downstream.

## Step 8: Streaming and Guardrails

After headers are normalized, response and request bodies continue under shared guardrail policy.

This layer covers:

- request body idle timeout
- request total body timeout
- response body idle timeout
- response total streaming timeout
- body size-cap enforcement
- unknown-length prebuffer limits
- chunk emission policy
- progressive emission eligibility

The guardrail layer lives in `edge::runtime::connection::guardrails`.

Ingress-specific code owns:

- polling
- wakeups
- channel and stream progression
- actual downstream chunk emission mechanics

It should not own the meaning of timeout and size-cap policy.

## Step 9: Error Classification, Retry, and Hedge Policy

When upstream execution fails or stalls, the request path uses shared classification and retry policy.

This layer covers:

- upstream error classification
- retryability interpretation
- retry denial reasons
- hedge trigger and suppression rules
- alternate backend selection
- canonical telemetry reason vocabularies

This allows forwarding code to orchestrate retries and hedges without owning raw error inspection logic itself.

## Step 10: Outcome Recording

Every terminal request path should flow through shared outcome recording.

This covers:

- route outcome classification
- backend outcome classification
- metrics recording
- overload and rejection reason mapping
- backend accounting hooks
- backend health feedback hooks

This is important because request paths should not each invent their own answer to “what happened.”

## QUIC vs Bootstrap Differences

Both paths should pass through the same policy layers in the same order.

They intentionally differ only in:

- ingress parsing and protocol setup
- downstream response/writeback mechanics
- websocket and upgrade mechanics on bootstrap
- QUIC-specific stream progression details

If a new policy exists only in one path, it usually belongs in a shared layer instead.

## Routing, Transport, and Lifecycle Relationship

The request path depends on three adjacent subsystems:

- routing decides which upstream and backend should receive the request
- transport decides how that backend request is executed on the wire
- backend lifecycle decides how request feedback changes backend health and operator-visible state

Those concerns are related, but they should not collapse into one module.

## Contributor Rules

When adding a new request-path feature:

- put ingress mechanics in `quic_listener` or `bootstrap`
- put shared policy in admission/auth/resolution/runtime layers
- put canonical request/response shaping in `bridge`
- put backend protocol execution in `transport`
- put shared terminal observation and accounting in `runtime::connection::outcome`

Do not:

- add protocol-specific branching in edge when transport owns it
- duplicate request header policy in listener code
- classify terminal outcomes independently in multiple request paths

## Mental Model

The shortest correct model is:

- ingress produces a canonical request context
- shared policy decides whether and where it goes
- bridge shapes it
- transport executes it
- bridge normalizes the result
- guardrails constrain streaming
- outcome recording tells the rest of the system what happened

That is the data-plane contract contributors should preserve.
