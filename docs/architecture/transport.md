# Transport Boundary

This document explains what the transport layer owns, what it deliberately hides, and where `edge` should stop reasoning about H1/H2 backend details.

## Purpose

The transport refactor established one rule:

- `edge` owns request orchestration
- `transport` owns backend protocol execution

Callers should depend on a canonical transport façade, not on protocol-specific pools or client behavior.

## Canonical Transport Surface

The main façade is:

- `crates/transport/src/transport_pool.rs`

`UpstreamTransportPool` is the transport contract that the rest of the system should use.

Its surface is intentionally transport-shaped:

- execute a canonical upstream request
- rotate a backend client when refresh or lifecycle logic requires it
- build the runtime transport pool from interpreted runtime config

This is the place where backend protocol choice becomes a resolved runtime concern.

## What Transport Owns

Transport owns the parts of backend execution that should not leak into request orchestration code.

### Runtime-selected protocol dispatch

Transport decides whether a backend runs over:

- HTTP/1.1
- HTTP/2

That mapping is resolved from runtime config and hidden behind the transport façade.

### Connection reuse

Transport owns:

- per-backend reusable clients
- idle connection reuse
- protocol-specific pool behavior

Callers should not know how reuse differs internally between H1 and H2.

### Transport-level timeout application

Transport owns:

- connect-level timeout behavior
- execution timeout around backend send operations
- protocol/client-level timeout handling that belongs to transport execution

Edge still owns higher-level request lifecycle deadlines and streaming deadlines.

### Client rotation

Transport owns backend client rotation when:

- DNS refresh changes the effective backend resolution
- lifecycle code asks transport to rotate or recreate the backend client

The result is exposed as a canonical transport rotation result instead of protocol-specific return shapes.

## What Edge Owns

Edge should still own the policy and orchestration around transport.

That includes:

- request preparation
- admission
- auth
- route and backend selection
- retry and hedge policy
- request streaming orchestration
- response emission
- outcome recording

Edge should not own how a chosen backend gets executed as H1 or H2.

## Internal Transport Structure

The transport façade hides the protocol-specific implementation modules.

Internally, transport still has:

- H1 client and H1 pool logic
- H2 client and H2 pool logic
- backend transport entry resolution

But those are implementation details behind the façade, not surfaces for higher-level orchestration.

## H1/H2 Hiding

The goal is not to pretend H1 and H2 are identical on the wire. The goal is to prevent those differences from leaking upward into the wrong layer.

Higher-level code should not branch because:

- H1 acquires or rotates clients one way
- H2 does it another way
- one protocol has a slightly different pool surface

Those differences should be absorbed inside transport so `edge` only consumes canonical outcomes.

## Request Execution Flow

The intended execution flow is:

1. edge resolves the backend target
2. edge builds a canonical upstream request
3. edge calls transport with backend identity plus request
4. transport chooses the internal backend transport entry
5. transport dispatches to H1 or H2 pool/client behavior
6. transport maps protocol-specific failures into canonical transport errors
7. edge consumes the result through shared error classification and retry policy

This keeps execution flow readable from the outside while still allowing protocol-specific implementation internally.

## Runtime Interpretation Boundary

The runtime/config layer interprets raw config into canonical runtime transport policy.

Transport then consumes that interpreted policy and builds the internal execution topology.

That means:

- raw config parsing is not transport's job
- route-level backend selection is not transport's job
- protocol realization from runtime backend transport kind is transport's job

## Timeout Ownership Split

The clean split is:

### Transport owns

- connection and client execution timeouts
- protocol-execution timeout application

### Edge owns

- end-to-end request lifecycle deadlines
- body streaming and idle guardrails
- admission and inflight waiting deadlines where policy requires them

If a timeout only exists because of the backend protocol execution path, it probably belongs in transport.

## DNS Refresh and Client Rotation

Backend DNS refresh and lifecycle logic may decide that transport clients should rotate.

That decision should not require callers to know:

- how H1 rotates clients
- how H2 rotates clients
- whether one protocol exposes generation movement differently

Transport should expose one canonical rotation result so lifecycle code can reason in transport-neutral terms.

## Error Mapping Boundary

Transport is responsible for mapping protocol/pool execution failures into canonical transport-facing errors.

Shared upstream error classification then interprets those errors for:

- retryability
- health-failure mapping
- metrics/logging reason mapping

This keeps transport from owning request policy while also keeping edge from digging into protocol implementation details.

## Contributor Rules

When adding code:

- put H1/H2 protocol behavior in transport protocol modules
- keep `transport_pool.rs` as the façade
- keep edge-side dispatch protocol-neutral
- expose canonical results when transport behavior matters to callers

Do not:

- add direct H1/H2 branching in forwarding or bootstrap code unless the concern is purely ingress compatibility
- leak protocol-specific helper types upward as public contracts
- make lifecycle or retry logic inspect protocol internals directly

## Mental Model

The simplest correct model is:

- edge decides whether to send
- transport decides how to send
- shared error and outcome layers decide what the send meant

If a change breaks that separation, it likely belongs in a different layer.
