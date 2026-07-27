//! Request/response execution state shared across listener paths.
//!
//! This module owns the canonical connection-domain types that both QUIC and
//! bootstrap request handling use: auth decisions, request/response envelopes,
//! stream state, and outcome recording. Body guardrails remain crate-private so
//! callers depend on the stable execution types rather than the enforcement
//! internals.

pub mod auth;
pub(crate) mod guardrails;
pub mod outcome;
pub mod quic;
pub mod request;
pub mod response;
pub mod stream;
