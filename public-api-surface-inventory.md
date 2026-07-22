# Public API Surface Inventory

Current Phase 2 surface for `refactor/public-api-and-visibility-hardening`.

Scope:
- crate `lib.rs` façades
- intentionally public subsystem entrypoints
- remaining hidden or compatibility-oriented surfaces that are still exposed on purpose

This document reflects the current branch state after the visibility-hardening work, not the original baseline audit.

## `spooky-edge`

### Crate façade
- `crates/edge/src/lib.rs`

### Public surface
- modules:
  - `benchmark`
  - `body`
  - `cid_radix`
  - `constants`
  - `hash`
  - `metrics`
  - `resilience`
  - `routing`
  - `runtime`
  - `watchdog`
- re-exports:
  - `ChannelBody`
  - `stable_hash_socket_addr`
  - `stable_hash64`
  - `Metrics`
  - `OverloadShedReason`
  - `RouteOutcome`
  - `HealthFailureReason`
  - `configure_async_runtime`
  - `ListenerWorkerRuntimeState`
  - `ListenerWorkerGroupConfig`
  - `spawn_listener_worker_group`
  - `release_shard_queue_bytes`
  - `shard_index_for_peer`
  - `try_reserve_shard_queue_bytes`

### Internal façades intentionally kept private
- `quic_listener`
- runtime connection/task/tls internals
- bootstrap/control-plane/request-path decomposition modules under `quic_listener`

### Phase 2 hardening result
- `quic_listener` is no longer crate-public
- `quic_listener::workers` is no longer a public module
- worker/runtime entrypoints are exposed only through the crate root and listener façade re-exports
- `HealthFailureReason` is re-exported from its owning crate path, not through `metrics`

### Remaining intentional exposure
- `runtime`, `routing`, `resilience`, and `watchdog` remain public because cross-crate tests and consumers still use stable types from those subsystems
- `benchmark` remains public as an explicitly exposed support surface

## `spooky-config`

### Crate façade
- `crates/config/src/lib.rs`

### Public surface
- modules:
  - `backend_endpoint`
  - `config`
  - `default`
  - `loader`
  - `runtime`
  - `validator`

### Canonical consumer paths
- raw config shapes from `config`
- runtime-ready policy/config output from `runtime`
- backend endpoint parsing/runtime shaping from `backend_endpoint`

### Internal façades intentionally kept private
- `runtime::listeners`
- `runtime::policies` domain modules
- `runtime::upstreams`

### Phase 2 hardening result
- runtime policy interpretation is centralized under `runtime`
- domain interpreter modules are no longer exposed as their own public surface
- crate-level ownership docs now direct consumers to `runtime` rather than policy internals

## `spooky-bridge`

### Crate façade
- `crates/bridge/src/lib.rs`

### Public surface
- modules:
  - `request`
  - `response`
  - `websocket`
- re-exports:
  - `BridgeError`

### Internal façades intentionally kept private
- `h3_to_h1`
- `h3_to_h2`
- forwarded/header/host helper modules

### Phase 2 hardening result
- protocol-specific request builder modules are internal
- public surface is now the canonical request/response/websocket policy layer only

## `spooky-errors`

### Crate façade
- `crates/errors/src/lib.rs`

### Public surface
- re-exported error and policy types from:
  - `bridge`
  - `pool`
  - `proxy`
  - `retry`
  - `upstream`

### Internal façades intentionally kept private
- module files themselves are private; consumers use only crate-root re-exports

### Phase 2 hardening result
- stale `spooky_lb::alternate_backend::*` compatibility re-exports are gone
- formatting and normalization helpers remain internal to the module implementations
- consumers see one shared classifier/error contract from the crate root

## `spooky-lb`

### Crate façade
- `crates/lb/src/lib.rs`

### Public surface
- canonical modules:
  - `alternate_backend`
  - `health`
  - `load_balancing`
  - `upstream_pool`
- hidden compatibility/testing substrate:
  - `algorithms`
  - `backend`
  - `backend_pool`
- crate-private:
  - `hash`

### Phase 2 hardening result
- `UpstreamPool` internals are private
- callers use narrow pool-level methods instead of reaching through internal fields
- implementation algorithms remain exposed only as hidden compatibility/test substrate, not as the recommended crate surface

## `spooky-transport`

### Crate façade
- `crates/transport/src/lib.rs`
- `crates/transport/src/transport_pool.rs`

### Public surface
- re-exports from owning modules:
  - `ConnectObservation`
  - `ConnectObserver`
  - `SharedDnsResolver`
  - `TlsClientConfig`
  - `TransportClientRotation`
  - `UpstreamTransportPool`

### Internal façades intentionally kept private
- `client_rotation`
- `h1_client`
- `h1_pool`
- `h2_client`
- `h2_pool`
- `transport_pool` module itself remains internal; only its façade types are re-exported

### Phase 2 hardening result
- protocol-specific pools/clients remain implementation details
- DNS/connect configuration types are re-exported from their owning module path, not via a secondary `transport_pool` shim
- internal client rotation state is no longer public

## Summary of Resolved Baseline Leaks

Resolved during this branch:
- `edge::quic_listener` is no longer public API
- `edge::quic_listener::workers` is no longer a public module
- `bridge::h3_to_h1` and `bridge::h3_to_h2` are no longer public
- `errors` no longer re-exports `lb` alternate-backend types
- `lb::hash` is no longer public
- `transport_pool` no longer acts as a compatibility re-export hop for DNS/connect config types

## Remaining Intentional Public Areas

These are still public by design or current test/consumer need:
- `edge::runtime` subsystem types
- `edge::routing` and `edge::resilience` subsystems
- `config::{default, validator}` modules
- hidden-but-public `lb` substrate modules used by compatibility/tests

These should be treated as the current deliberate surface, not accidental leftovers from the Phase 2 refactors.
