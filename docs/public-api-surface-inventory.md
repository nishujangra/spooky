# Public API Surface Inventory

Phase 1 baseline for `refactor/final-api-lockdown-and-deletion`.

Purpose:
- re-state the current crate façades exactly as the branch exposes them
- classify every non-private boundary item in the primary architecture crates
- identify which exposed items are canonical API, crate-local collaboration seams, compatibility surfaces, or stale leftovers

Scope for this baseline:
- `crates/edge`
- `crates/config`
- `crates/bridge`
- `crates/lb`
- `crates/transport`

Classification keys:
- `canonical public API`: intended external consumer surface
- `internal-to-crate collaboration surface`: `pub(crate)` seam used inside a crate boundary
- `temporary compatibility surface`: intentionally exposed, but not the recommended long-term owning surface
- `stale leftover`: exposed item that no longer has a clear architectural reason to stay visible

This document classifies current visibility. It does not by itself change policy.

## `spooky-edge`

### Crate façade
- `crates/edge/src/lib.rs`

### Current façade declarations

| Item | Visibility | Classification | Notes |
| --- | --- | --- | --- |
| `benchmark` | `pub mod` | canonical public API | Explicit support surface at crate root. |
| `body` | `pub mod` | canonical public API | Owning module for `ChannelBody`. |
| `cid_radix` | `pub mod` | canonical public API | Still treated as a direct crate surface on this branch. |
| `constants` | `pub mod` | stale leftover | Broad constant export is not described as a deliberate consumer façade elsewhere. |
| `hash` | `pub mod` | stale leftover | Mixed public helpers plus crate-internal counter exposure suggests historical convenience surface. |
| `metrics` | `pub mod` | canonical public API | Owns exported metrics-facing runtime types. |
| `observability` | `pub mod` | stale leftover | Public at root but absent from prior intentional-surface docs. |
| `quic_listener` | private `mod` | n/a | Correctly private listener subsystem façade. |
| `resilience` | `pub mod` | canonical public API | Deliberate subsystem surface. |
| `routing` | `pub mod` | canonical public API | Deliberate subsystem surface. |
| `runtime` | `pub mod` | canonical public API | Deliberate subsystem surface for stable runtime types. |
| `watchdog` | `pub mod` | canonical public API | Small public façade with internal helpers behind it. |
| `ChannelBody` | `pub use` | canonical public API | Root-level convenience re-export from owning public module. |
| `REQUEST_ID_COUNTER` | `pub(crate) use` | internal-to-crate collaboration surface | Crate-wide shared counter, not external API. |
| `stable_hash_socket_addr` | `pub use` | canonical public API | Explicit root helper export. |
| `stable_hash64` | `pub use` | canonical public API | Explicit root helper export. |
| `Metrics` | `pub use` | canonical public API | Root convenience export for core metrics type. |
| `OverloadShedReason` | `pub use` | canonical public API | Root export for public overload outcome vocabulary. |
| `RouteOutcome` | `pub use` | canonical public API | Root export for public request outcome vocabulary. |
| `configure_async_runtime` | `pub use` | canonical public API | Narrow external worker/runtime entrypoint. |
| `ListenerWorkerRuntimeState` | `pub use` | canonical public API | Narrow external worker/runtime entrypoint. |
| `ListenerWorkerGroupConfig` | `pub use` | canonical public API | Narrow external worker/runtime entrypoint. |
| `spawn_listener_worker_group` | `pub use` | canonical public API | Narrow external worker/runtime entrypoint. |
| `release_shard_queue_bytes` | `pub use` | canonical public API | Narrow external worker/runtime entrypoint. |
| `shard_index_for_peer` | `pub use` | canonical public API | Narrow external worker/runtime entrypoint. |
| `try_reserve_shard_queue_bytes` | `pub use` | canonical public API | Narrow external worker/runtime entrypoint. |
| `HealthFailureReason` | `pub use` | temporary compatibility surface | Useful shared type, but re-exported from another crate rather than owned here. |

### Public subsystem seams under `runtime`

| Item | Visibility | Classification | Notes |
| --- | --- | --- | --- |
| `runtime::backend` | `pub mod` | canonical public API | Declared stable runtime subsystem surface. |
| `runtime::bundle` | `pub mod` | canonical public API | Public runtime state grouping surface. |
| `runtime::connection` | `pub(crate) mod` | internal-to-crate collaboration surface | Correctly hidden request/response plumbing. |
| `runtime::generation` | `pub(crate) mod` | internal-to-crate collaboration surface | Generation swap internals. |
| `runtime::health` | `pub mod` | canonical public API | Stable health classification/output surface. |
| `runtime::listener` | `pub mod` | canonical public API | Public listener state type owner. |
| `runtime::policy` | `pub mod` | canonical public API | Public runtime policy access surface. |
| `runtime::shared_state` | `pub mod` | canonical public API | Stable shared-state surface on this branch. |
| `runtime::tasks` | `pub(crate) mod` | internal-to-crate collaboration surface | Runtime task lifecycle internals. |
| `runtime::tls` | `pub(crate) mod` | internal-to-crate collaboration surface | Runtime TLS loading/reload internals. |

### Public subsystem seams under `watchdog`

| Item | Visibility | Classification | Notes |
| --- | --- | --- | --- |
| `watchdog::config` | `pub(crate) mod` | internal-to-crate collaboration surface | Internal config translation. |
| `watchdog::coordinator` | `pub mod` | canonical public API | Intended owning public surface. |
| `watchdog::service` | `pub(crate) mod` | internal-to-crate collaboration surface | Service execution internals. |
| `watchdog::state` | `pub(crate) mod` | internal-to-crate collaboration surface | Internal state carrier. |
| `watchdog::time` | `pub(crate) mod` | internal-to-crate collaboration surface | Internal timing helpers. |

### Phase 1 baseline calls

- `quic_listener` staying private is correct and should remain the baseline.
- `runtime::{connection,generation,tasks,tls}` and `watchdog::{config,service,state,time}` are legitimate crate-local collaboration seams.
- `constants`, `hash`, and `observability` are the main root-level exposures that still look under-justified.
- `HealthFailureReason` is intentionally useful but is still a compatibility-style re-export rather than an owned edge API.

## `spooky-config`

### Crate façade
- `crates/config/src/lib.rs`

### Current façade declarations

| Item | Visibility | Classification | Notes |
| --- | --- | --- | --- |
| `backend_endpoint` | `pub mod` | canonical public API | Shared endpoint parsing/runtime shaping surface. |
| `config` | `pub mod` | canonical public API | User-facing raw config schema owner. |
| `default` | `pub mod` | canonical public API | Explicit defaults surface on this branch. |
| `loader` | `pub mod` | canonical public API | Canonical config-loading entrypoint. |
| `runtime` | `pub mod` | canonical public API | Canonical normalized runtime output surface. |
| `validator` | `pub mod` | canonical public API | Canonical validation surface. |

### Runtime lowering seams

| Item | Visibility | Classification | Notes |
| --- | --- | --- | --- |
| `runtime::listeners` | private `mod` | n/a | Correctly internal lowering module. |
| `runtime::policies` | private `mod` | n/a | Correctly internal lowering owner behind runtime re-exports. |
| `runtime::upstreams` | private `mod` | n/a | Correctly internal lowering module. |
| `runtime` re-exports from `policies` | `pub use` | canonical public API | Public runtime policy vocabulary is intentionally flattened here. |
| `RuntimeConfig` | `pub struct` | canonical public API | Primary normalized config output. |
| `RuntimeConfigError` | `pub enum` | canonical public API | Public validation/lowering error contract. |
| `RuntimeListener` | `pub struct` | canonical public API | Public normalized listener shape. |
| `ListenerRuntimeConfig` | `pub struct` | canonical public API | Canonical listener-scoped runtime view. |
| `RuntimeListenerSource` | `pub enum` | canonical public API | Public listener-origin vocabulary. |
| `RuntimeListenerTls` | `pub struct` | canonical public API | Public listener TLS policy output. |
| `RuntimeTlsIdentity` | `pub struct` | canonical public API | Public normalized TLS identity. |
| `RuntimeUpstream` | `pub struct` | canonical public API | Public normalized upstream shape. |
| `RuntimeBackend` | `pub struct` | canonical public API | Public normalized backend shape. |
| `RuntimeHostPolicy` | `pub struct` | canonical public API | Public wrapper type used by downstream crates. |
| `RuntimeForwardedHeaderPolicy` | `pub struct` | canonical public API | Public wrapper type used by downstream crates. |
| `RuntimeProtocolPolicy` | `pub struct` | canonical public API | Public wrapper type used by downstream crates. |
| `RuntimeUpstreamPolicy` | `pub struct` | canonical public API | Canonical runtime policy bundle for one upstream. |
| `RuntimeConfig::upstreams_as_config` | `#[cfg(test)] pub(crate) fn` | internal-to-crate collaboration surface | Test-only visibility shim; not public API. |
| `RuntimeUpstream::backend_tls_policy` field | `pub(crate)` field | internal-to-crate collaboration surface | Internal escape hatch on an otherwise public type. |

### Phase 1 baseline calls

- `runtime` remains the correct canonical API owner for lowered policy/config state.
- policy interpreter modules are correctly private already.
- the only notable visibility shim in scope is `RuntimeConfig::upstreams_as_config`.
- no obvious stale public root modules were found in the config crate façade.

## `spooky-bridge`

### Crate façade
- `crates/bridge/src/lib.rs`

### Current façade declarations

| Item | Visibility | Classification | Notes |
| --- | --- | --- | --- |
| `forwarded` | private `mod` | n/a | Internal helper. |
| `h3_to_h1` | private `mod` | n/a | Internal protocol-specific builder. |
| `h3_to_h2` | private `mod` | n/a | Internal protocol-specific builder. |
| `headers` | private `mod` | n/a | Internal helper. |
| `host` | private `mod` | n/a | Internal helper. |
| `request` | `pub mod` | canonical public API | Canonical request construction and header policy surface. |
| `response` | `pub mod` | canonical public API | Canonical response normalization surface. |
| `websocket` | `pub mod` | canonical public API | Canonical websocket and upgrade helper surface. |
| `BridgeError` | `pub use` | temporary compatibility surface | Publicly useful shared error, but owned by `spooky-errors`. |

### Phase 1 baseline calls

- the bridge crate façade is already narrow and aligned with intended ownership.
- `BridgeError` is the only compatibility-style re-export in the root surface.
- no stale leftover module exposure remains at the crate root.

## `spooky-lb`

### Crate façade
- `crates/lb/src/lib.rs`

### Current façade declarations

| Item | Visibility | Classification | Notes |
| --- | --- | --- | --- |
| `algorithms` | `#[doc(hidden)] pub mod` | temporary compatibility surface | Hidden but still public substrate. |
| `alternate_backend` | `pub mod` | canonical public API | Deliberate public subsystem. |
| `backend` | `#[doc(hidden)] pub mod` | temporary compatibility surface | Hidden but still public substrate. |
| `backend_pool` | `#[doc(hidden)] pub mod` | temporary compatibility surface | Hidden but still public substrate. |
| `hash` | `pub(crate) mod` | internal-to-crate collaboration surface | Correctly crate-private helper module. |
| `health` | `pub mod` | canonical public API | Deliberate public subsystem. |
| `load_balancing` | `pub mod` | canonical public API | Deliberate public subsystem. |
| `upstream_pool` | `pub mod` | canonical public API | Deliberate public subsystem. |

### Phase 1 baseline calls

- `algorithms`, `backend`, and `backend_pool` are explicit compatibility/testing surfaces and should be treated as temporary until re-justified.
- no stale leftover crate-root exposure is hiding behind ordinary `pub` here; the questionable items are already marked hidden.

## `spooky-transport`

### Crate façade
- `crates/transport/src/lib.rs`
- `crates/transport/src/transport_pool.rs`

### Current façade declarations

| Item | Visibility | Classification | Notes |
| --- | --- | --- | --- |
| `client_rotation` | private `mod` | n/a | Internal rotation-state machinery. |
| `h1_client` | private `mod` | n/a | Internal protocol implementation. |
| `h1_pool` | private `mod` | n/a | Internal protocol implementation. |
| `h2_client` | private `mod` | n/a | Internal protocol implementation. |
| `h2_pool` | private `mod` | n/a | Internal protocol implementation. |
| `transport_pool` | private `mod` | n/a | Private owning module behind façade re-exports. |
| `ConnectObservation` | `pub use` | canonical public API | Public transport observability type. |
| `ConnectObserver` | `pub use` | canonical public API | Public transport observability callback type. |
| `SharedDnsResolver` | `pub use` | canonical public API | Public DNS coordination type. |
| `TlsClientConfig` | `pub use` | canonical public API | Public upstream TLS config type. |
| `TransportClientRotation` | `pub use` | canonical public API | Public backend-client rotation result type. |
| `UpstreamTransportPool` | `pub use` | canonical public API | Canonical transport façade. |

### Public façade types owned by `transport_pool`

| Item | Visibility | Classification | Notes |
| --- | --- | --- | --- |
| `TransportClientRotation` | `pub struct` | canonical public API | Narrow public wrapper around internal rotation state. |
| `UpstreamTransportPool` | `pub struct` | canonical public API | Canonical execution façade for downstream callers. |

### Phase 1 baseline calls

- the transport crate boundary is already in the intended shape: private protocol modules plus narrow façade re-exports.
- no hidden compatibility modules remain at the crate root.
- no stale leftover root exposure was found in the transport crate façade.

## Baseline Summary

### Canonical public API surfaces

- `edge`: `benchmark`, `body`, `cid_radix`, `metrics`, `resilience`, `routing`, `runtime`, `watchdog`, and the narrow worker/runtime re-exports
- `config`: `backend_endpoint`, `config`, `default`, `loader`, `runtime`, `validator`, plus runtime policy and normalized runtime types
- `bridge`: `request`, `response`, `websocket`
- `lb`: `alternate_backend`, `health`, `load_balancing`, `upstream_pool`
- `transport`: `ConnectObservation`, `ConnectObserver`, `SharedDnsResolver`, `TlsClientConfig`, `TransportClientRotation`, `UpstreamTransportPool`

### Internal-to-crate collaboration surfaces

- `edge`: `REQUEST_ID_COUNTER`, `runtime::{connection,generation,tasks,tls}`, `watchdog::{config,service,state,time}`
- `config`: `RuntimeConfig::upstreams_as_config`, `RuntimeUpstream::backend_tls_policy` field
- `lb`: `hash`

### Temporary compatibility surfaces

- `edge`: `HealthFailureReason` root re-export from `spooky-lb`
- `bridge`: `BridgeError` root re-export from `spooky-errors`
- `lb`: `#[doc(hidden)]` modules `algorithms`, `backend`, `backend_pool`

### Stale leftovers requiring Phase 2 review

- `edge::constants`
- `edge::hash`
- `edge::observability`

## Phase 1 Exit Check

- every non-private boundary item in the scoped crates has an explicit classification
- the current baseline no longer relies on “probably needed” wording
- the main unresolved Phase 2 targets are now named rather than implicit
