# Backend Lifecycle

This document explains how Spooky models backend resolution, health, pool placement, and request feedback as one lifecycle instead of several unrelated side effects.

## Purpose

Backend lifecycle state should be understandable as one pipeline:

1. backend identity is defined
2. resolution state is tracked
3. pool membership exists in upstream pools
4. health state changes from refresh, checks, and request feedback
5. canonical snapshots are exposed to the Control API and observability surfaces

The canonical implementation lives under:

- `crates/edge/src/runtime/backend/`
- `crates/edge/src/quic_listener/backend_resolution.rs`
- `crates/edge/src/quic_listener/health_check.rs`
- `crates/lb/src/upstream_pool.rs`

## Ownership Boundary

The key rule is:

- `lb` owns balancing and per-pool request accounting substrate
- `edge::runtime::backend` owns lifecycle state, lifecycle events, and lifecycle snapshots

Listener request paths, health checks, and DNS refresh loops should emit typed lifecycle inputs rather than independently deciding backend state transitions in scattered places.

## How Lifecycle Fits Into Request Flow

Backend lifecycle is adjacent to routing and transport, but it is not the same thing as either one:

- routing decides which backend is eligible for a request
- transport decides how that backend request runs on the wire
- backend lifecycle decides how backend state changes over time and how operators see that state

Request execution feeds lifecycle. Lifecycle does not replace request execution.

## Core Concepts

### Backend identity

Backend identity is the stable key for lifecycle state.

Today that identity is modeled by `BackendIdentity`, which is centered on the canonical backend address string.

Identity is intentionally separate from mutable runtime properties such as:

- resolved addresses
- health state
- membership state

### Resolution state

Resolution state is modeled by `BackendResolutionState`.

It includes:

- authority host
- authority port
- address kind such as hostname vs IP literal
- current resolved socket addresses
- last successful refresh time
- refresh generation

This is the canonical answer to “where does this backend currently resolve.”

### Health state

Health state is modeled by `BackendHealthState`.

It can be:

- `Unknown`
- `Healthy`
- `Unhealthy { reason }`

The lifecycle layer uses shared health-failure reason vocabularies so passive failures, active health checks, and Control API views describe failures consistently.

### Membership state

Membership state is modeled by `BackendMembershipState`.

It describes whether the backend is:

- active
- suppressed
- removed

This keeps placement and availability distinct from name resolution and health.

## Lifecycle State and Snapshots

### Runtime lifecycle state

`RuntimeBackendLifecycleState` groups:

- identity
- resolution
- health
- membership

This is the canonical mutable lifecycle record for a backend.

### Snapshots

The lifecycle layer exposes snapshots for operator-facing and debugging surfaces:

- `BackendLifecycleSnapshot`
- `CanonicalBackendLifecycleSnapshot`
- `BackendLifecycleInventorySnapshot`

These are what the Control API and metrics-oriented surfaces should use instead of rebuilding backend state from several unrelated stores.

`BackendLifecycleInventorySnapshot` also supports summary views such as total backends and healthy backends.

## Backend Lifecycle Coordinator

`BackendLifecycleCoordinator` is the unification point.

Its responsibilities are:

- expose backend lifecycle snapshots
- apply DNS refresh outcomes
- apply health observations
- merge lifecycle state with upstream pool placement inventory

In practice it is the place where contributors should look first when backend lifecycle ownership is unclear.

## DNS Refresh Flow

Hostname backends participate in DNS refresh lifecycle.

The flow is:

1. refresh loop performs lookup
2. raw lookup result is passed to the lifecycle coordinator
3. lifecycle applies refresh outcome to the resolution store
4. lifecycle decides whether the backend resolution changed
5. lifecycle coordinates client rotation behavior with transport
6. lifecycle emits a canonical refresh classification

Important cases are modeled explicitly:

- updated addresses
- unchanged addresses
- empty answer retained previous addresses
- lookup failed while preserving active addresses

This is important operationally because a failed refresh should not silently erase the active resolution.

## Health Observation Flow

Active health checks and other health observations should not mutate pools inline from many places.

The intended flow is:

1. a scheduler or request path produces a `BackendHealthObservation`
2. the observation states:
   - source
   - outcome
   - optional reason
3. lifecycle-owned application logic decides the health transition
4. pool health state and lifecycle snapshot stay aligned

Observation sources include:

- active check
- passive request
- request completion
- control plane

This keeps the question “why did this backend become unhealthy” answerable from typed state rather than from log archaeology.

## Request Feedback Flow

Request completion also contributes lifecycle information.

The shared request-feedback model is `BackendRequestFeedback`, which includes:

- backend identity
- elapsed time
- optional status code
- typed outcome

Possible request-feedback outcomes are:

- success
- neutral
- failure with optional health-failure reason

The request path should emit feedback, and lifecycle application decides whether that feedback changes health state.

This prevents forwarding code from mixing transport completion, health mutation, and accounting policy in one branch.

## Pool Membership and Accounting

Upstream pools remain the balancing/accounting substrate.

They still own things such as:

- backend indices
- healthy flags inside a pool
- active request counts
- latency/accounting data used for balancing

The lifecycle layer reads and applies against that substrate through narrow mutation and snapshot boundaries instead of letting arbitrary callers mutate pool health directly.

## Canonical Inventory View

For operator-facing surfaces, backend lifecycle must be visible as one inventory, not as several separate stores.

The inventory view combines:

- stable backend identity
- resolution state
- lifecycle health state
- membership state
- per-upstream placements

That gives the Control API and observability surfaces one place to answer:

- which backends exist
- which backends are healthy
- which upstreams currently place them
- what addresses they resolve to
- whether refresh state is current

## Runtime Generation Interaction

Backend lifecycle lives inside the larger runtime-generation model:

- generation-owned state provides the active upstream definitions and placement context
- shared services such as lifecycle coordination and resolution storage preserve the operator view that request paths consume
- reloads should publish a coherent next generation instead of partially mutating live backend state in place

This matters because operators need backend state to stay explainable across reload, refresh, failure, and recovery events.

## What Request Paths Should Do

Forwarding and bootstrap code should:

- resolve which backend is being used
- emit request accounting and request feedback
- consume canonical lifecycle snapshots when needed

They should not:

- invent new backend health mutation logic
- directly combine resolution store state with pool state for Control API output
- treat DNS refresh and health transitions as unrelated subsystems

## Contributor Rules

When adding lifecycle-related behavior:

- put new resolution state or snapshot fields in `runtime/backend/state.rs`
- put new lifecycle inputs in `runtime/backend/event.rs`
- put new application logic in `runtime/backend/lifecycle.rs`
- keep `health_check.rs` and `backend_resolution.rs` focused on orchestration and scheduling
- keep `lb` focused on balancing substrate, not lifecycle orchestration

## Mental Model

Think about backend lifecycle this way:

- identity tells you which backend
- resolution tells you where it points
- health tells you whether it should receive traffic
- membership tells you whether it is still placed
- the lifecycle coordinator owns how refresh, checks, and request feedback update that picture

If a new feature changes backend state, it should most likely enter through the lifecycle layer rather than by adding another direct mutation path.
