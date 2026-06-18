# Testing Strategy

This page explains what the current test suite is trying to protect.

## Test Layers

### Unit Tests

Unit tests sit next to implementation and protect local behavior such as:

- validators
- data-structure behavior
- metric rendering
- load-balancer transitions
- helper semantics

### Integration Tests

The strongest behavioral guarantees come from integration tests, especially under `crates/edge/tests/`.

Important coverage areas include:

- H3 to H2 round-trip behavior
- trailer preservation
- TLS and SNI behavior
- client-auth behavior
- malformed packet handling
- connection churn and teardown
- overload shedding
- request and response body caps
- draining and forced close behavior
- retry, timeout, and error mapping semantics

### Benchmark Suite

`crates/bench/` is used for:

- micro-benchmarks
- macro workload models
- baseline comparison
- release-regression gating

## What A New Feature Should Usually Add

- unit coverage for the local logic
- integration coverage for externally visible behavior
- metrics assertions when the feature changes operator visibility
- docs updates when the feature changes supported behavior or operational posture

## What Still Needs More Coverage

- fuzzing
- property tests for determinism and cleanup invariants
- broader interoperability validation against external ecosystems
- longer soak and chaos validation outside the normal unit/integration loop
