# Invariants

This page records the behavioral rules that contributors should preserve when making changes.

## Routing Invariants

- route selection must be deterministic
- longer path prefixes beat shorter prefixes
- exact host matches beat wildcard or host-agnostic matches
- method-specific matches beat any-method matches
- ambiguous normalized routes should be rejected at startup

## Connection And CID Invariants

- a connection’s primary CID must remain consistent with the primary key in the connection map
- alias CIDs must resolve back to the correct primary CID
- CID cleanup must not leave orphaned alias mappings
- draining or timeout cleanup must not leak connection-tracking state

## Stream Lifecycle Invariants

- terminated streams must release all associated resource reservations
- client-side reset and upstream timeout paths must not leak inflight permits
- a finished or failed stream must not block progress of unrelated streams
- body caps and idle timeouts should terminate with the intended HTTP behavior when applicable

## Health And Backend Invariants

- 2xx and 3xx responses are success signals
- 4xx responses are neutral health signals
- 5xx, timeout, and transport errors are unhealthy signals
- backend health transitions must respect configured thresholds and cooldowns

## Control-Plane Invariants

- certificate reload is not full config reload
- runtime inspection endpoints must remain informative but constrained
- control-plane auth requirements must remain explicit in docs and code
