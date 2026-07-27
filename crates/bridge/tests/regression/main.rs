//! Regression suite for the `spooky_bridge` request-building pipeline.
//!
//! Public-API integration tests for `build_h2_request_for_target` (h3→h2) and
//! `build_h1_request` (h3→h1): scheme selection, host/forwarded-header policy,
//! hop-by-hop stripping, spoofed-header removal, WebSocket shaping, and H1/H2
//! output parity. Shared fixtures live in `common`.

mod common;

mod h3_to_h1;
mod h3_to_h2;
mod host_forwarded_policy;
mod request_contract;
mod websocket_contract;
