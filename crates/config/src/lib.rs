//! Canonical configuration surface for Impulse.
//!
//! This crate owns raw config parsing/validation plus the runtime-ready policy
//! interpretation that downstream crates consume. Use:
//! - [`config`] for deserialized user-facing configuration shapes
//! - [`validator`] for config validation rules
//! - [`runtime`] for normalized, validated runtime policy output
//! - [`backend_endpoint`] for shared backend endpoint parsing/runtime shaping

pub mod backend_endpoint;
pub mod config;
pub mod loader;
pub mod runtime;
pub mod validator;
