use std::{cell::RefCell, error::Error as StdError, fmt};

use log::{info, warn};

use crate::{
    backend_endpoint::{BackendEndpoint, BackendScheme},
    config::{
        CURRENT_CONFIG_VERSION, Config, ControlApi, ControlApiAuditSink, ControlApiClientAuthMode,
        ExternalAuth, JwtVerificationKey, SUPPORTED_CONFIG_VERSIONS, ScopedRateLimitScope,
        SecretProvider, SecretRef, UpstreamHostPolicyMode,
    },
    runtime::RuntimeLoadBalancingStrategy,
};

mod auth;
mod control_plane;
mod helpers;
mod performance;
mod secrets;
#[cfg(test)]
mod tests;
mod upstreams;

pub(crate) use helpers::{is_valid_https_or_loopback_http_url, is_valid_https_url};

pub const VALID_LOG_LEVELS: &[&str] = &[
    "whisper",
    "haunt",
    "impulse",
    "scream",
    "poltergeist",
    "silence",
    "trace",
    "debug",
    "info",
    "warn",
    "error",
    "off",
];

pub const VALID_LB_TYPES: &[&str] = &[
    "random",
    "round-robin",
    "round_robin",
    "rr",
    "consistent-hash",
    "consistent_hash",
    "ch",
    "least-connections",
    "least_connections",
    "lc",
    "latency-aware",
    "latency_aware",
    "la",
    "sticky-cid",
    "sticky_cid",
    "cid-sticky",
    "cid_sticky",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
}

impl ValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for ValidationError {}

thread_local! {
    static LAST_VALIDATION_ERROR: RefCell<Option<ValidationError>> = const { RefCell::new(None) };
}

const VALID_CONTROL_API_IDENTITY_SOURCE_KINDS: &[&str] = &[
    "mtls_subject_cn",
    "mtls_san_dns",
    "mtls_san_uri",
    "mtls_subject",
];

type RouteMatcherKey = (Option<String>, Option<String>, Option<String>);

fn clear_validation_error() {
    LAST_VALIDATION_ERROR.with(|slot| *slot.borrow_mut() = None);
}

fn record_validation_error(message: String) {
    LAST_VALIDATION_ERROR.with(|slot| {
        let mut guard = slot.borrow_mut();
        if guard.is_none() {
            *guard = Some(ValidationError::new(message));
        }
    });
}

fn take_validation_error() -> Option<ValidationError> {
    LAST_VALIDATION_ERROR.with(|slot| slot.borrow_mut().take())
}

pub fn validate(config: &Config) -> Result<(), ValidationError> {
    clear_validation_error();
    if validate_inner(config) {
        Ok(())
    } else {
        Err(take_validation_error().unwrap_or_else(|| {
            ValidationError::new("configuration validation failed for an unspecified reason")
        }))
    }
}

fn validate_inner(config: &Config) -> bool {
    info!("Starting configuration validation...");

    if !SUPPORTED_CONFIG_VERSIONS.contains(&config.version) {
        let supported = SUPPORTED_CONFIG_VERSIONS
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        record_validation_error(format!(
            "Invalid version: found '{}', supported versions are [{}]",
            config.version, supported
        ));
        log::error!(
            "Invalid version: found '{}', supported versions are [{}]",
            config.version,
            supported
        );
        return false;
    }
    if config.version != CURRENT_CONFIG_VERSION {
        warn!(
            "Config version '{}' is supported but not current (current={}); please migrate when possible",
            config.version, CURRENT_CONFIG_VERSION
        );
    }

    if !validate_secrets_config(config) {
        return false;
    }
    if !performance::validate_global_config(config) {
        return false;
    }
    if !upstreams::validate_upstream_routes(config) {
        return false;
    }
    if !upstreams::validate_upstreams(config) {
        return false;
    }

    info!("Configuration validation passed successfully\n");
    true
}

use self::secrets::validate_secrets_config;
