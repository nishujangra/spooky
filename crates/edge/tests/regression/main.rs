//! Regression suite for `spooky_edge` public-API surfaces.
//!
//! `metrics` exercises the Prometheus exposition contract end-to-end
//! (`Metrics::render_prometheus` output), and `hash` pins the stable-hash
//! helpers used for load-balancing key derivation. Both use only the crate's
//! public API.
//!
//! More runtime-heavy regressions stay in the dedicated integration suites
//! under `crates/edge/tests/`; see `tests/regression/README.md` for the suite
//! boundary.

mod hash;
mod metrics;
