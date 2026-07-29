//! Regression suite for `spooky_config::runtime::RuntimeConfig` lowering.
//!
//! These are public-API contract tests: they exercise `RuntimeConfig::from_config`
//! and assert the runtime lowering preserves auth/TLS/policy contracts, normalizes
//! timeout/transport knobs, and rejects invalid configurations.
//!
//! Tests that depend on crate-internal items (the private `runtime::listeners`
//! module and the `#[cfg(test)] pub(crate)` `upstreams_as_config` /
//! `upstream_policy_sets` helpers) remain as unit tests in
//! `crates/config/src/runtime.rs`.
//!
//! See `tests/regression/README.md` for the split between runtime-lowering
//! regressions kept here and interpreter-internal unit tests kept beside the
//! owning modules.

mod common;

mod auth;
mod backends;
mod policy;
mod reload_boundaries;
mod timeouts;
mod tls;
