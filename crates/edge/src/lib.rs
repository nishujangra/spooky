//! Public API for the Spooky edge runtime.
//!
//! The crate root exposes the small set of entrypoints that other crates should
//! depend on directly. Listener orchestration, runtime wiring, and control-plane
//! mechanics stay behind internal subsystem modules.

pub mod benchmark;
pub mod body;
pub mod cid_radix;
pub mod constants;
pub mod hash;
pub mod metrics;
pub mod observability;
mod quic_listener;
pub mod resilience;
pub mod routing;
pub mod runtime;
pub mod watchdog;

pub use body::ChannelBody;
pub(crate) use hash::REQUEST_ID_COUNTER;
pub use hash::{stable_hash_socket_addr, stable_hash64};
pub use metrics::{Metrics, OverloadShedReason, RouteOutcome};
pub use quic_listener::{
    ListenerWorkerGroupConfig, ListenerWorkerRuntimeState, configure_async_runtime,
    release_shard_queue_bytes, shard_index_for_peer, spawn_listener_worker_group,
    try_reserve_shard_queue_bytes,
};
pub use spooky_lb::health::HealthFailureReason;
