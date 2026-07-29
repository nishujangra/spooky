//! Regression suite for the `spooky_bridge` request and protocol-shaping contracts.
//!
//! Public-API integration tests for `build_h2_request_for_target` (h3→h2) and
//! `build_h1_request` (h3→h1): scheme selection, host/forwarded-header policy,
//! hop-by-hop stripping, spoofed-header removal, WebSocket shaping, and H1/H2
//! output parity. Shared fixtures live in `common`, while response-normalization
//! contracts live under the bridge unit tests.
//!
//! See `tests/regression/README.md` for the boundary between request-shaping
//! regressions kept here and response-normalization contracts kept beside the
//! owning bridge entrypoints.

mod common;

// Request-shaping contracts.
mod host_forwarded_policy;
mod request_contract;
mod websocket_contract;

// Protocol-specific request behavior and parity.
mod h3_to_h1;
mod h3_to_h2;
