use std::{cell::RefCell, collections::HashMap, error::Error as StdError, fmt, net::IpAddr};

use log::{info, warn};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};

use crate::{
    backend_endpoint::{BackendEndpoint, BackendScheme},
    config::{
        CURRENT_CONFIG_VERSION, Config, ControlApi, ControlApiAuditSink,
        ControlApiClientAuthMode, ExternalAuth, Listen, SUPPORTED_CONFIG_VERSIONS,
        ScopedRateLimitScope, UpstreamHostPolicyMode, UpstreamTls,
    },
};

#[path = "validator/helpers.rs"]
mod helpers;
use helpers::*;

pub const VALID_LOG_LEVELS: &[&str] = &[
    "whisper",
    "haunt",
    "spooky",
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

macro_rules! validation_error {
    ($($arg:tt)*) => {{
        let message = format!($($arg)*);
        record_validation_error(message.clone());
        log::error!("{}", message);
    }};
}

type RouteMatcherKey = (Option<String>, Option<String>, Option<String>);

fn valid_prefix_len(addr: &str, prefix: &str) -> bool {
    let Ok(prefix) = prefix.parse::<u8>() else {
        return false;
    };
    match addr.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => prefix <= 32,
        Ok(IpAddr::V6(_)) => prefix <= 128,
        Err(_) => false,
    }
}

fn is_valid_cidr(cidr: &str) -> bool {
    let Some((addr, prefix)) = cidr.split_once('/') else {
        return false;
    };
    valid_prefix_len(addr.trim(), prefix.trim())
}

fn validate_control_api_authentication(control_api: &ControlApi) -> bool {
    if let Some(token) = control_api.auth_token.as_ref()
        && token.trim().is_empty()
    {
        validation_error!("observability.control_api.auth_token cannot be empty when provided");
        return false;
    }

    for (idx, token) in control_api.auth.bearer_tokens.iter().enumerate() {
        if token.token.trim().is_empty() {
            validation_error!(
                "observability.control_api.auth.bearer_tokens[{}].token cannot be empty",
                idx
            );
            return false;
        }
        if let Some(actor_id) = token.actor_id.as_ref()
            && actor_id.trim().is_empty()
        {
            validation_error!(
                "observability.control_api.auth.bearer_tokens[{}].actor_id cannot be empty when provided",
                idx
            );
            return false;
        }
    }

    if let Some(identity_source) = control_api.auth.identity_source.as_ref() {
        if identity_source.kind.trim().is_empty() {
            validation_error!(
                "observability.control_api.auth.identity_source.kind cannot be empty when provided"
            );
            return false;
        }
        if matches!(
            control_api.tls.client_auth.mode,
            ControlApiClientAuthMode::Disabled
        ) {
            validation_error!(
                "observability.control_api.auth.identity_source requires observability.control_api.tls.client_auth.mode to be optional or required"
            );
            return false;
        }
    }

    let has_legacy_token = control_api.auth_token.is_some();
    let has_static_tokens = !control_api.auth.bearer_tokens.is_empty();
    match control_api.tls.client_auth.mode {
        ControlApiClientAuthMode::Disabled => {
            if !has_legacy_token && !has_static_tokens {
                validation_error!(
                    "observability.control_api requires at least one admin auth mechanism when enabled: auth_token, auth.bearer_tokens, or tls.client_auth.mode=required"
                );
                return false;
            }
        }
        ControlApiClientAuthMode::Optional => {
            if !has_legacy_token && !has_static_tokens {
                validation_error!(
                    "observability.control_api.tls.client_auth.mode=optional cannot be the only admin auth mechanism; configure auth_token, auth.bearer_tokens, or use mode=required"
                );
                return false;
            }
        }
        ControlApiClientAuthMode::Required => {}
    }

    true
}

fn validate_control_api_security(control_api: &ControlApi) -> bool {
    let client_auth = &control_api.tls.client_auth;
    if matches!(
        client_auth.mode,
        ControlApiClientAuthMode::Optional | ControlApiClientAuthMode::Required
    ) && client_auth.ca_file.is_none()
        && client_auth.ca_dir.is_none()
    {
        validation_error!(
            "observability.control_api.tls.client_auth.ca_file or ca_dir is required when client_auth.mode is optional or required"
        );
        return false;
    }

    if let Some(ca_file) = client_auth.ca_file.as_ref()
        && ca_file.trim().is_empty()
    {
        validation_error!(
            "observability.control_api.tls.client_auth.ca_file cannot be empty when provided"
        );
        return false;
    }
    if let Some(ca_dir) = client_auth.ca_dir.as_ref()
        && ca_dir.trim().is_empty()
    {
        validation_error!(
            "observability.control_api.tls.client_auth.ca_dir cannot be empty when provided"
        );
        return false;
    }

    if control_api.authorization.runtime_mutate_role < control_api.authorization.runtime_read_role {
        validation_error!(
            "observability.control_api.authorization.runtime_mutate_role must be at least as privileged as runtime_read_role"
        );
        return false;
    }
    if control_api.authorization.restart_role < control_api.authorization.runtime_mutate_role {
        validation_error!(
            "observability.control_api.authorization.restart_role must be at least as privileged as runtime_mutate_role"
        );
        return false;
    }

    for (idx, cidr) in control_api.ip_allowlist.cidrs.iter().enumerate() {
        if cidr.trim().is_empty() {
            validation_error!(
                "observability.control_api.ip_allowlist.cidrs[{}] cannot be empty",
                idx
            );
            return false;
        }
        if !is_valid_cidr(cidr.trim()) {
            validation_error!(
                "observability.control_api.ip_allowlist.cidrs[{}] must be a valid CIDR",
                idx
            );
            return false;
        }
    }

    if let Some(path) = control_api.audit.file_path.as_ref()
        && path.trim().is_empty()
    {
        validation_error!(
            "observability.control_api.audit.file_path cannot be empty when provided"
        );
        return false;
    }
    match control_api.audit.sink {
        ControlApiAuditSink::Log => {
            if control_api.audit.file_path.is_some() {
                validation_error!(
                    "observability.control_api.audit.file_path is only valid when audit.sink=file"
                );
                return false;
            }
        }
        ControlApiAuditSink::File => {
            if control_api.audit.file_path.is_none() {
                validation_error!(
                    "observability.control_api.audit.file_path is required when audit.sink=file"
                );
                return false;
            }
        }
    }

    validate_control_api_authentication(control_api)
}

fn validate_external_auth_headers(
    upstream_name: &str,
    field_prefix: &str,
    request_headers: &[crate::config::ExternalAuthRequestHeader],
    response_header_allowlist: &[String],
) -> bool {
    let mut seen_request_headers = std::collections::HashSet::new();
    for (idx, header) in request_headers.iter().enumerate() {
        let header_name = header.name.trim();
        if header_name.is_empty() {
            validation_error!(
                "upstream '{}' {}.request_headers[{}].name must be non-empty",
                upstream_name,
                field_prefix,
                idx
            );
            return false;
        }
        if http::header::HeaderName::from_bytes(header_name.as_bytes()).is_err() {
            validation_error!(
                "upstream '{}' {}.request_headers[{}].name must be a valid HTTP header name",
                upstream_name,
                field_prefix,
                idx
            );
            return false;
        }
        if http::HeaderValue::from_str(header.value.as_str()).is_err() {
            validation_error!(
                "upstream '{}' {}.request_headers[{}].value must be a valid HTTP header value",
                upstream_name,
                field_prefix,
                idx
            );
            return false;
        }
        let normalized_name = header_name.to_ascii_lowercase();
        if !seen_request_headers.insert(normalized_name) {
            validation_error!(
                "upstream '{}' {}.request_headers contains duplicate header names",
                upstream_name,
                field_prefix
            );
            return false;
        }
    }

    let mut seen_allowed_headers = std::collections::HashSet::new();
    for (idx, header_name) in response_header_allowlist.iter().enumerate() {
        let header_name = header_name.trim();
        if header_name.is_empty() {
            validation_error!(
                "upstream '{}' {}.response_header_allowlist[{}] must be non-empty",
                upstream_name,
                field_prefix,
                idx
            );
            return false;
        }
        if http::header::HeaderName::from_bytes(header_name.as_bytes()).is_err() {
            validation_error!(
                "upstream '{}' {}.response_header_allowlist[{}] must be a valid HTTP header name",
                upstream_name,
                field_prefix,
                idx
            );
            return false;
        }
        let normalized_name = header_name.to_ascii_lowercase();
        if !seen_allowed_headers.insert(normalized_name) {
            validation_error!(
                "upstream '{}' {}.response_header_allowlist contains duplicate header names",
                upstream_name,
                field_prefix
            );
            return false;
        }
    }

    true
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

    // --- Validate version ---
    if !SUPPORTED_CONFIG_VERSIONS.contains(&config.version) {
        let supported = SUPPORTED_CONFIG_VERSIONS
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        validation_error!(
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

    // --- Validate effective listen blocks ---
    if config.listeners.is_empty() {
        if !validate_listen_config(&config.listen, "listen") {
            return false;
        }
    } else {
        for (idx, listen) in config.listeners.iter().enumerate() {
            if !validate_listen_config(listen, &format!("listeners[{idx}]")) {
                return false;
            }
        }
    }

    let effective_listeners: Vec<(String, &crate::config::Listen)> = if config.listeners.is_empty()
    {
        vec![("listen".to_string(), &config.listen)]
    } else {
        config
            .listeners
            .iter()
            .enumerate()
            .map(|(idx, listen)| (format!("listeners[{idx}]"), listen))
            .collect()
    };

    let mut seen_listener_bindings: HashMap<(String, u16), String> = HashMap::new();
    for (label, listen) in effective_listeners {
        let key = (listen.address.clone(), listen.port);
        if let Some(existing) = seen_listener_bindings.insert(key, label.clone()) {
            validation_error!(
                "listener binding conflict: {} duplicates {} on {}:{}",
                label,
                existing,
                listen.address,
                listen.port
            );
            return false;
        }
    }

    // --- Validate log level ---
    if !VALID_LOG_LEVELS
        .iter()
        .any(|lvl| lvl.eq_ignore_ascii_case(&config.log.level))
    {
        validation_error!("Invalid log level: {}", config.log.level);
        return false;
    }

    // --- Validate global load balancing type (if present) ---
    if let Some(ref lb) = config.load_balancing
        && !VALID_LB_TYPES
            .iter()
            .any(|lb_type| lb_type.eq_ignore_ascii_case(&lb.lb_type))
    {
        validation_error!("Invalid global load balancing type: {}", lb.lb_type);
        return false;
    }

    // --- Validate performance controls ---
    if config.performance.worker_threads == 0 {
        validation_error!("performance.worker_threads must be greater than 0");
        return false;
    }

    if config.performance.worker_threads > 1024 {
        validation_error!(
            "performance.worker_threads={} exceeds the maximum of 1024",
            config.performance.worker_threads
        );
        return false;
    }

    if config.performance.control_plane_threads == 0 {
        validation_error!("performance.control_plane_threads must be greater than 0");
        return false;
    }

    if config.performance.control_plane_threads > 1024 {
        validation_error!(
            "performance.control_plane_threads={} exceeds the maximum of 1024",
            config.performance.control_plane_threads
        );
        return false;
    }

    if config.performance.packet_shards_per_worker == 0 {
        validation_error!("performance.packet_shards_per_worker must be greater than 0");
        return false;
    }

    if config.performance.packet_shards_per_worker > 256 {
        validation_error!(
            "performance.packet_shards_per_worker={} exceeds the maximum of 256",
            config.performance.packet_shards_per_worker
        );
        return false;
    }

    if config.performance.packet_shard_queue_capacity == 0 {
        validation_error!("performance.packet_shard_queue_capacity must be greater than 0");
        return false;
    }

    if config.performance.packet_shard_queue_max_bytes == 0 {
        validation_error!("performance.packet_shard_queue_max_bytes must be greater than 0");
        return false;
    }

    if config.performance.worker_threads > 1 && !config.performance.reuseport {
        validation_error!("performance.reuseport must be true when performance.worker_threads > 1");
        return false;
    }

    if config.performance.global_inflight_limit == 0 {
        validation_error!("performance.global_inflight_limit must be greater than 0");
        return false;
    }

    if config.performance.per_upstream_inflight_limit == 0 {
        validation_error!("performance.per_upstream_inflight_limit must be greater than 0");
        return false;
    }

    if config.performance.inflight_acquire_wait_ms > 25 {
        warn!(
            "performance.inflight_acquire_wait_ms={} may increase tail latency under sustained load; keep it small (0-25ms) for burst smoothing only",
            config.performance.inflight_acquire_wait_ms
        );
    }

    if config.performance.backend_timeout_ms == 0 {
        validation_error!("performance.backend_timeout_ms must be greater than 0");
        return false;
    }

    if config.performance.backend_connect_timeout_ms == 0 {
        validation_error!("performance.backend_connect_timeout_ms must be greater than 0");
        return false;
    }

    if config.performance.backend_body_idle_timeout_ms == 0 {
        validation_error!("performance.backend_body_idle_timeout_ms must be greater than 0");
        return false;
    }

    if config.performance.backend_body_total_timeout_ms == 0 {
        validation_error!("performance.backend_body_total_timeout_ms must be greater than 0");
        return false;
    }

    if config.performance.backend_total_request_timeout_ms == 0 {
        validation_error!("performance.backend_total_request_timeout_ms must be greater than 0");
        return false;
    }

    if config.performance.shutdown_drain_timeout_ms == 0 {
        validation_error!("performance.shutdown_drain_timeout_ms must be greater than 0");
        return false;
    }

    if config.performance.udp_recv_buffer_bytes == 0 {
        validation_error!("performance.udp_recv_buffer_bytes must be greater than 0");
        return false;
    }

    if config.performance.udp_send_buffer_bytes == 0 {
        validation_error!("performance.udp_send_buffer_bytes must be greater than 0");
        return false;
    }

    if config.performance.h2_pool_max_idle_per_backend == 0 {
        validation_error!("performance.h2_pool_max_idle_per_backend must be greater than 0");
        return false;
    }

    if config.performance.h2_pool_idle_timeout_ms == 0 {
        validation_error!("performance.h2_pool_idle_timeout_ms must be greater than 0");
        return false;
    }

    if config.performance.backend_dns_refresh_interval_ms == 0 {
        validation_error!("performance.backend_dns_refresh_interval_ms must be greater than 0");
        return false;
    }

    if config.performance.per_backend_inflight_limit == 0 {
        validation_error!("performance.per_backend_inflight_limit must be greater than 0");
        return false;
    }

    if config.performance.new_connections_per_sec == 0 {
        validation_error!("performance.new_connections_per_sec must be greater than 0");
        return false;
    }

    if config.performance.new_connections_burst == 0 {
        validation_error!("performance.new_connections_burst must be greater than 0");
        return false;
    }

    if config.performance.max_active_connections == 0 {
        validation_error!("performance.max_active_connections must be greater than 0");
        return false;
    }

    if config.performance.quic_max_idle_timeout_ms == 0 {
        validation_error!("performance.quic_max_idle_timeout_ms must be greater than 0");
        return false;
    }

    if config.performance.quic_initial_max_data == 0 {
        validation_error!("performance.quic_initial_max_data must be greater than 0");
        return false;
    }

    if config.performance.quic_initial_max_stream_data == 0 {
        validation_error!("performance.quic_initial_max_stream_data must be greater than 0");
        return false;
    }

    if config.performance.quic_initial_max_stream_data > config.performance.quic_initial_max_data {
        validation_error!(
            "performance.quic_initial_max_stream_data ({}) must be <= quic_initial_max_data ({})",
            config.performance.quic_initial_max_stream_data,
            config.performance.quic_initial_max_data
        );
        return false;
    }

    if config.performance.quic_initial_max_streams_bidi == 0 {
        validation_error!("performance.quic_initial_max_streams_bidi must be greater than 0");
        return false;
    }

    if config.performance.quic_initial_max_streams_uni == 0 {
        validation_error!("performance.quic_initial_max_streams_uni must be greater than 0");
        return false;
    }

    if config.performance.max_response_body_bytes == 0 {
        validation_error!("performance.max_response_body_bytes must be greater than 0");
        return false;
    }

    if config.performance.max_request_body_bytes == 0 {
        validation_error!("performance.max_request_body_bytes must be greater than 0");
        return false;
    }

    if config.performance.request_buffer_global_cap_bytes == 0 {
        validation_error!("performance.request_buffer_global_cap_bytes must be greater than 0");
        return false;
    }

    if config.performance.unknown_length_response_prebuffer_bytes == 0 {
        validation_error!(
            "performance.unknown_length_response_prebuffer_bytes must be greater than 0"
        );
        return false;
    }

    if config.performance.client_body_idle_timeout_ms == 0 {
        validation_error!("performance.client_body_idle_timeout_ms must be greater than 0");
        return false;
    }

    if config.performance.backend_connect_timeout_ms > config.performance.backend_timeout_ms {
        validation_error!("performance.backend_connect_timeout_ms must be <= backend_timeout_ms");
        return false;
    }

    if config.performance.backend_timeout_ms > config.performance.backend_body_idle_timeout_ms {
        validation_error!("performance.backend_timeout_ms must be <= backend_body_idle_timeout_ms");
        return false;
    }

    if config.performance.backend_body_idle_timeout_ms
        > config.performance.backend_body_total_timeout_ms
    {
        validation_error!(
            "performance.backend_body_idle_timeout_ms must be <= backend_body_total_timeout_ms"
        );
        return false;
    }

    if config.performance.backend_body_total_timeout_ms
        > config.performance.backend_total_request_timeout_ms
    {
        validation_error!(
            "performance.backend_body_total_timeout_ms must be <= backend_total_request_timeout_ms"
        );
        return false;
    }

    if config.performance.max_request_body_bytes
        > config.performance.quic_initial_max_stream_data as usize
    {
        validation_error!(
            "performance.max_request_body_bytes ({}) must be <= quic_initial_max_stream_data ({})",
            config.performance.max_request_body_bytes,
            config.performance.quic_initial_max_stream_data
        );
        return false;
    }

    if config.performance.request_buffer_global_cap_bytes
        < config.performance.max_request_body_bytes
    {
        validation_error!(
            "performance.request_buffer_global_cap_bytes ({}) must be >= max_request_body_bytes ({})",
            config.performance.request_buffer_global_cap_bytes,
            config.performance.max_request_body_bytes
        );
        return false;
    }

    if config.performance.unknown_length_response_prebuffer_bytes
        > config.performance.max_response_body_bytes
    {
        validation_error!(
            "performance.unknown_length_response_prebuffer_bytes ({}) must be <= max_response_body_bytes ({})",
            config.performance.unknown_length_response_prebuffer_bytes,
            config.performance.max_response_body_bytes
        );
        return false;
    }

    if config.resilience.adaptive_admission.min_limit == 0 {
        validation_error!("resilience.adaptive_admission.min_limit must be greater than 0");
        return false;
    }
    if let Some(max_limit) = config.resilience.adaptive_admission.max_limit {
        if max_limit == 0 {
            validation_error!("resilience.adaptive_admission.max_limit must be greater than 0");
            return false;
        }
        if max_limit < config.resilience.adaptive_admission.min_limit {
            validation_error!(
                "resilience.adaptive_admission.max_limit ({}) must be >= min_limit ({})",
                max_limit,
                config.resilience.adaptive_admission.min_limit
            );
            return false;
        }
        if max_limit > config.performance.global_inflight_limit {
            validation_error!(
                "resilience.adaptive_admission.max_limit ({}) must be <= performance.global_inflight_limit ({})",
                max_limit,
                config.performance.global_inflight_limit
            );
            return false;
        }
    }

    if config.resilience.adaptive_admission.decrease_step == 0 {
        validation_error!("resilience.adaptive_admission.decrease_step must be greater than 0");
        return false;
    }

    if config.resilience.adaptive_admission.increase_step == 0 {
        validation_error!("resilience.adaptive_admission.increase_step must be greater than 0");
        return false;
    }

    if config.resilience.route_queue.default_cap == 0 {
        validation_error!("resilience.route_queue.default_cap must be greater than 0");
        return false;
    }

    if config.resilience.route_queue.global_cap == 0 {
        validation_error!("resilience.route_queue.global_cap must be greater than 0");
        return false;
    }

    if config.resilience.route_queue.shed_retry_after_seconds == 0 {
        validation_error!("resilience.route_queue.shed_retry_after_seconds must be greater than 0");
        return false;
    }

    if config
        .resilience
        .route_queue
        .caps
        .values()
        .any(|cap| *cap == 0)
    {
        validation_error!("resilience.route_queue.caps values must be greater than 0");
        return false;
    }

    if config.resilience.protocol.max_headers_count == 0 {
        validation_error!("resilience.protocol.max_headers_count must be greater than 0");
        return false;
    }

    if config.resilience.protocol.max_headers_bytes == 0 {
        validation_error!("resilience.protocol.max_headers_bytes must be greater than 0");
        return false;
    }

    if config
        .resilience
        .protocol
        .early_data_safe_methods
        .iter()
        .any(|method| method.trim().is_empty())
    {
        validation_error!(
            "resilience.protocol.early_data_safe_methods must not contain empty values"
        );
        return false;
    }

    if config
        .resilience
        .protocol
        .allowed_methods
        .iter()
        .any(|method| method.trim().is_empty())
    {
        validation_error!("resilience.protocol.allowed_methods must not contain empty values");
        return false;
    }

    if config
        .resilience
        .protocol
        .allowed_methods
        .iter()
        .any(|method| !is_valid_http_token(method))
    {
        validation_error!(
            "resilience.protocol.allowed_methods must contain valid HTTP method tokens"
        );
        return false;
    }

    if config
        .resilience
        .protocol
        .denied_path_prefixes
        .iter()
        .any(|prefix| prefix.is_empty() || !prefix.starts_with('/'))
    {
        validation_error!(
            "resilience.protocol.denied_path_prefixes must contain '/'-prefixed paths"
        );
        return false;
    }

    if !config.resilience.protocol.allow_connect
        && (!config.resilience.protocol.connect_allowed_ports.is_empty()
            || !config
                .resilience
                .protocol
                .connect_allowed_authorities
                .is_empty())
    {
        validation_error!(
            "resilience.protocol.connect_allowed_ports/connect_allowed_authorities require allow_connect=true"
        );
        return false;
    }

    if config
        .resilience
        .protocol
        .connect_allowed_ports
        .contains(&0)
    {
        validation_error!(
            "resilience.protocol.connect_allowed_ports must contain ports in range 1-65535"
        );
        return false;
    }

    if config
        .resilience
        .protocol
        .connect_allowed_authorities
        .iter()
        .any(|authority| !is_valid_connect_authority(authority))
    {
        validation_error!(
            "resilience.protocol.connect_allowed_authorities must contain authority-form host:port targets"
        );
        return false;
    }

    if config.resilience.protocol.allow_0rtt
        && config
            .resilience
            .protocol
            .early_data_safe_methods
            .is_empty()
    {
        validation_error!(
            "resilience.protocol.early_data_safe_methods must be non-empty when allow_0rtt=true"
        );
        return false;
    }

    if config.resilience.circuit_breaker.failure_threshold == 0 {
        validation_error!("resilience.circuit_breaker.failure_threshold must be greater than 0");
        return false;
    }

    if config.resilience.circuit_breaker.open_ms == 0 {
        validation_error!("resilience.circuit_breaker.open_ms must be greater than 0");
        return false;
    }

    if config.resilience.circuit_breaker.half_open_max_probes == 0 {
        validation_error!("resilience.circuit_breaker.half_open_max_probes must be greater than 0");
        return false;
    }

    if config.resilience.retry_budget.ratio_percent > 100 {
        validation_error!("resilience.retry_budget.ratio_percent must be <= 100");
        return false;
    }

    if config
        .resilience
        .retry_budget
        .per_route_ratio_percent
        .values()
        .any(|ratio| *ratio > 100)
    {
        validation_error!("resilience.retry_budget.per_route_ratio_percent values must be <= 100");
        return false;
    }

    let mut seen_scoped_rate_limit_names = std::collections::HashSet::new();
    for rule in &config.resilience.scoped_rate_limits {
        let rule_name = rule.name.trim();
        if rule_name.is_empty() {
            validation_error!("resilience.scoped_rate_limits[].name must be non-empty");
            return false;
        }
        if !seen_scoped_rate_limit_names.insert(rule_name.to_string()) {
            validation_error!(
                "resilience.scoped_rate_limits contains duplicate rule name '{}'",
                rule_name
            );
            return false;
        }
        if rule.requests_per_sec == 0 {
            validation_error!(
                "resilience.scoped_rate_limits['{}'].requests_per_sec must be greater than 0",
                rule_name
            );
            return false;
        }
        if rule.burst == 0 {
            validation_error!(
                "resilience.scoped_rate_limits['{}'].burst must be greater than 0",
                rule_name
            );
            return false;
        }
        if rule.idle_ttl_secs == 0 {
            validation_error!(
                "resilience.scoped_rate_limits['{}'].idle_ttl_secs must be greater than 0",
                rule_name
            );
            return false;
        }
        if rule
            .route_allowlist
            .iter()
            .any(|route| route.trim().is_empty())
        {
            validation_error!(
                "resilience.scoped_rate_limits['{}'].route_allowlist must not contain empty values",
                rule_name
            );
            return false;
        }
        match rule.scope {
            ScopedRateLimitScope::Route => {
                if rule
                    .key
                    .as_ref()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    validation_error!(
                        "resilience.scoped_rate_limits['{}'].key is invalid for scope=route",
                        rule_name
                    );
                    return false;
                }
            }
            ScopedRateLimitScope::Tenant => {
                let Some(key_spec) = rule.key.as_deref() else {
                    validation_error!(
                        "resilience.scoped_rate_limits['{}'].key is required for scope=tenant",
                        rule_name
                    );
                    return false;
                };
                if !is_valid_request_key_spec(key_spec) {
                    validation_error!(
                        "resilience.scoped_rate_limits['{}'].key must be a supported request key spec",
                        rule_name
                    );
                    return false;
                }
            }
            ScopedRateLimitScope::Client | ScopedRateLimitScope::Token => {
                if let Some(key_spec) = rule.key.as_deref()
                    && !is_valid_request_key_spec(key_spec)
                {
                    validation_error!(
                        "resilience.scoped_rate_limits['{}'].key must be a supported request key spec",
                        rule_name
                    );
                    return false;
                }
            }
        }
    }

    if config.resilience.brownout.trigger_inflight_percent > 100
        || config.resilience.brownout.recover_inflight_percent > 100
    {
        validation_error!("resilience.brownout inflight percentages must be <= 100");
        return false;
    }

    if config.resilience.brownout.recover_inflight_percent
        >= config.resilience.brownout.trigger_inflight_percent
    {
        validation_error!(
            "resilience.brownout.recover_inflight_percent must be < trigger_inflight_percent"
        );
        return false;
    }

    if config.resilience.watchdog.check_interval_ms == 0 {
        validation_error!("resilience.watchdog.check_interval_ms must be greater than 0");
        return false;
    }

    if config.resilience.watchdog.poll_stall_timeout_ms == 0 {
        validation_error!("resilience.watchdog.poll_stall_timeout_ms must be greater than 0");
        return false;
    }

    if config.resilience.watchdog.timeout_error_rate_percent > 100 {
        validation_error!("resilience.watchdog.timeout_error_rate_percent must be <= 100");
        return false;
    }

    if config.resilience.watchdog.min_requests_per_window == 0 {
        validation_error!("resilience.watchdog.min_requests_per_window must be greater than 0");
        return false;
    }

    if config.resilience.watchdog.overload_inflight_percent > 100 {
        validation_error!("resilience.watchdog.overload_inflight_percent must be <= 100");
        return false;
    }

    if config.resilience.watchdog.unhealthy_consecutive_windows == 0 {
        validation_error!(
            "resilience.watchdog.unhealthy_consecutive_windows must be greater than 0"
        );
        return false;
    }

    if config.resilience.watchdog.drain_grace_ms == 0 {
        validation_error!("resilience.watchdog.drain_grace_ms must be greater than 0");
        return false;
    }

    if config.resilience.watchdog.restart_cooldown_ms == 0 {
        validation_error!("resilience.watchdog.restart_cooldown_ms must be greater than 0");
        return false;
    }

    if !config.resilience.watchdog.restart_command.is_empty()
        && config.resilience.watchdog.restart_command[0]
            .trim()
            .is_empty()
    {
        validation_error!(
            "resilience.watchdog.restart_command[0] must be a non-empty executable path"
        );
        return false;
    }

    if config.resilience.watchdog.restart_hook.is_some() {
        validation_error!(
            "resilience.watchdog.restart_hook is deprecated and unsupported; use restart_command instead"
        );
        return false;
    }

    // --- Validate observability ---
    if config.observability.metrics.enabled {
        if config.observability.metrics.address.is_empty() {
            validation_error!(
                "observability.metrics.address cannot be empty when metrics are enabled"
            );
            return false;
        }

        if config.observability.metrics.port == 0 {
            validation_error!("observability.metrics.port must be between 1 and 65535");
            return false;
        }

        if !config.observability.metrics.path.starts_with('/') {
            validation_error!("observability.metrics.path must start with '/'");
            return false;
        }

        if config.observability.metrics.max_connections == 0 {
            validation_error!("observability.metrics.max_connections must be greater than 0");
            return false;
        }

        if config.observability.metrics.connection_timeout_ms == 0 {
            validation_error!("observability.metrics.connection_timeout_ms must be greater than 0");
            return false;
        }

        if !is_loopback_bind_address(&config.observability.metrics.address) {
            warn!(
                "observability.metrics is bound to non-loopback address {}; ensure network ACLs protect plaintext metrics endpoint",
                config.observability.metrics.address
            );
        }
    }

    if config.observability.control_api.enabled {
        if config.observability.control_api.address.is_empty() {
            validation_error!(
                "observability.control_api.address cannot be empty when control_api is enabled"
            );
            return false;
        }

        if config.observability.control_api.port == 0 {
            validation_error!("observability.control_api.port must be between 1 and 65535");
            return false;
        }

        let paths = [
            (
                "observability.control_api.health_path",
                config.observability.control_api.health_path.as_str(),
            ),
            (
                "observability.control_api.ready_path",
                config.observability.control_api.ready_path.as_str(),
            ),
            (
                "observability.control_api.runtime_path",
                config.observability.control_api.runtime_path.as_str(),
            ),
            (
                "observability.control_api.restart_path",
                config.observability.control_api.restart_path.as_str(),
            ),
            (
                "observability.control_api.reload_path",
                config.observability.control_api.reload_path.as_str(),
            ),
            (
                "observability.control_api.reload_certs_path",
                config.observability.control_api.reload_certs_path.as_str(),
            ),
        ];
        for (name, path) in paths {
            if !path.starts_with('/') {
                validation_error!("{} must start with '/'", name);
                return false;
            }
        }

        if config.observability.control_api.max_connections == 0 {
            validation_error!("observability.control_api.max_connections must be greater than 0");
            return false;
        }

        if config.observability.control_api.connection_timeout_ms == 0 {
            validation_error!(
                "observability.control_api.connection_timeout_ms must be greater than 0"
            );
            return false;
        }

        if !validate_control_api_security(&config.observability.control_api) {
            return false;
        }
    }

    if config.observability.routing.expose_header
        && config.observability.routing.header_name.trim().is_empty()
    {
        validation_error!(
            "observability.routing.header_name must be non-empty when expose_header=true"
        );
        return false;
    }

    // --- Validate privilege-drop security controls ---
    if config.security.privileges.enabled {
        if config.security.privileges.user.trim().is_empty() {
            validation_error!(
                "security.privileges.user must be non-empty when privilege drop is enabled"
            );
            return false;
        }
        if config.security.privileges.group.trim().is_empty() {
            validation_error!(
                "security.privileges.group must be non-empty when privilege drop is enabled"
            );
            return false;
        }
    }

    if config.observability.tracing.enabled {
        if config.observability.tracing.service_name.trim().is_empty() {
            validation_error!(
                "observability.tracing.service_name cannot be empty when tracing is enabled"
            );
            return false;
        }
        if !(0.0..=1.0).contains(&config.observability.tracing.sample_ratio) {
            validation_error!("observability.tracing.sample_ratio must be between 0.0 and 1.0");
            return false;
        }
        if let Some(endpoint) = config.observability.tracing.otlp_endpoint.as_ref()
            && endpoint.trim().is_empty()
        {
            validation_error!("observability.tracing.otlp_endpoint cannot be empty when provided");
            return false;
        }
    }

    // --- Validate upstream routes ---
    for (upstream_name, upstream) in &config.upstream {
        // Validate route matcher has at least one condition
        let has_host = upstream.route.host.is_some();
        let has_path = upstream.route.path_prefix.is_some();

        if !has_host && !has_path {
            validation_error!(
                "Upstream '{}' must have either 'host' or 'path_prefix' route matcher",
                upstream_name
            );
            return false;
        }

        // Validate path_prefix is not empty if present
        if let Some(ref path) = upstream.route.path_prefix {
            if path.is_empty() {
                validation_error!(
                    "Route path_prefix cannot be empty for upstream '{}'",
                    upstream_name
                );
                return false;
            }
            if !path.starts_with('/') {
                validation_error!(
                    "Route path_prefix must start with '/' for upstream '{}': {}",
                    upstream_name,
                    path
                );
                return false;
            }
        }

        match upstream.host_policy.mode {
            UpstreamHostPolicyMode::PassThrough | UpstreamHostPolicyMode::Upstream => {
                if upstream.host_policy.host.is_some() {
                    validation_error!(
                        "upstream {}.host_policy.host is invalid unless mode is rewrite",
                        upstream_name
                    );
                    return false;
                }
            }
            UpstreamHostPolicyMode::Rewrite => match upstream.host_policy.host.as_deref() {
                Some(host) if valid_static_host_header(host) => {}
                _ => {
                    validation_error!(
                        "upstream {}.host_policy.mode=rewrite requires a valid non-empty host_policy.host",
                        upstream_name
                    );
                    return false;
                }
            },
        }
    }

    // --- Validate upstreams ---
    if config.upstream.is_empty() {
        validation_error!("No upstreams configured");
        return false;
    }

    let mut seen_route_matchers: HashMap<RouteMatcherKey, String> = HashMap::new();

    for (upstream_name, upstream) in &config.upstream {
        let route_key = (
            upstream.route.host.as_deref().map(normalize_route_host),
            upstream.route.path_prefix.clone(),
            normalized_route_method(upstream.route.method.as_deref()),
        );

        if let Some(existing_upstream) =
            seen_route_matchers.insert(route_key.clone(), upstream_name.clone())
        {
            validation_error!(
                "Ambiguous route matcher detected: upstream '{}' conflicts with upstream '{}' for host={:?} path_prefix={:?} method={:?}",
                upstream_name,
                existing_upstream,
                route_key.0,
                route_key.1,
                route_key.2
            );
            return false;
        }
    }

    let mut seen_backend_origins: HashMap<String, (String, String)> = HashMap::new();
    let mut validate_global_upstream_tls = false;

    for (upstream_name, upstream) in &config.upstream {
        if upstream_name.is_empty() {
            validation_error!("Upstream name is empty");
            return false;
        }

        // Validate load balancing type for this upstream
        if !VALID_LB_TYPES
            .iter()
            .any(|lb_type| lb_type.eq_ignore_ascii_case(&upstream.load_balancing.lb_type))
        {
            validation_error!(
                "Invalid load balancing type '{}' for upstream '{}'",
                upstream.load_balancing.lb_type,
                upstream_name
            );
            return false;
        }

        if let Some(api_key) = upstream.auth.api_key.as_ref() {
            let header_name = api_key.header_name.trim();
            if header_name.is_empty() {
                validation_error!(
                    "upstream '{}' auth.api_key.header_name must be non-empty",
                    upstream_name
                );
                return false;
            }
            if !is_valid_http_token(header_name) {
                validation_error!(
                    "upstream '{}' auth.api_key.header_name must be a valid HTTP header name",
                    upstream_name
                );
                return false;
            }
            if api_key.keys.is_empty() || api_key.keys.iter().any(|value| value.trim().is_empty()) {
                validation_error!(
                    "upstream '{}' auth.api_key.keys must contain at least one non-empty key",
                    upstream_name
                );
                return false;
            }
            let mut seen_api_keys = std::collections::HashSet::new();
            for key in &api_key.keys {
                if !seen_api_keys.insert(key.trim().to_string()) {
                    validation_error!(
                        "upstream '{}' auth.api_key.keys contains duplicate values",
                        upstream_name
                    );
                    return false;
                }
            }
        }

        if let Some(external_auth) = upstream.auth.external_auth.as_ref() {
            if upstream.auth.api_key.is_some() || upstream.auth.jwt.is_some() {
                validation_error!(
                    "upstream '{}' auth.external_auth cannot be combined with auth.api_key or auth.jwt in v1",
                    upstream_name
                );
                return false;
            }
            if !upstream.auth.required_scopes.is_empty() || !upstream.auth.required_roles.is_empty()
            {
                validation_error!(
                    "upstream '{}' auth.external_auth cannot be combined with auth.required_scopes or auth.required_roles in v1",
                    upstream_name
                );
                return false;
            }

            match external_auth {
                ExternalAuth::Http {
                    endpoint,
                    request_headers,
                    response_header_allowlist,
                    timeout_ms,
                    ..
                } => {
                    if !is_valid_http_url(endpoint) {
                        validation_error!(
                            "upstream '{}' auth.external_auth.http.endpoint must be an absolute http(s) URL",
                            upstream_name
                        );
                        return false;
                    }
                    if !validate_external_auth_headers(
                        upstream_name,
                        "auth.external_auth.http",
                        request_headers,
                        response_header_allowlist,
                    ) {
                        return false;
                    }
                    if *timeout_ms == 0 {
                        validation_error!(
                            "upstream '{}' auth.external_auth.http.timeout_ms must be greater than 0",
                            upstream_name
                        );
                        return false;
                    }
                }
                ExternalAuth::Oidc {
                    discovery_url,
                    issuer_url,
                    client_id,
                    client_secret,
                    audience,
                    scopes,
                    request_headers,
                    response_header_allowlist,
                    timeout_ms,
                    ..
                } => {
                    let has_discovery_url = discovery_url
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty());
                    let has_issuer_url = issuer_url
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty());
                    if !has_discovery_url && !has_issuer_url {
                        validation_error!(
                            "upstream '{}' auth.external_auth.oidc requires discovery_url or issuer_url",
                            upstream_name
                        );
                        return false;
                    }
                    if let Some(discovery_url) = discovery_url.as_deref()
                        && !discovery_url.trim().is_empty()
                        && !is_valid_https_url(discovery_url)
                    {
                        validation_error!(
                            "upstream '{}' auth.external_auth.oidc.discovery_url must be an absolute https URL",
                            upstream_name
                        );
                        return false;
                    }
                    if let Some(issuer_url) = issuer_url.as_deref()
                        && !issuer_url.trim().is_empty()
                        && !is_valid_https_url(issuer_url)
                    {
                        validation_error!(
                            "upstream '{}' auth.external_auth.oidc.issuer_url must be an absolute https URL",
                            upstream_name
                        );
                        return false;
                    }
                    if client_id.trim().is_empty() {
                        validation_error!(
                            "upstream '{}' auth.external_auth.oidc.client_id must be non-empty",
                            upstream_name
                        );
                        return false;
                    }
                    if client_secret
                        .as_deref()
                        .is_some_and(|value| value.trim().is_empty())
                    {
                        validation_error!(
                            "upstream '{}' auth.external_auth.oidc.client_secret must be non-empty when provided",
                            upstream_name
                        );
                        return false;
                    }
                    if audience
                        .as_deref()
                        .is_some_and(|value| value.trim().is_empty())
                    {
                        validation_error!(
                            "upstream '{}' auth.external_auth.oidc.audience must be non-empty when provided",
                            upstream_name
                        );
                        return false;
                    }
                    if scopes.iter().any(|scope| scope.trim().is_empty()) {
                        validation_error!(
                            "upstream '{}' auth.external_auth.oidc.scopes must not contain empty values",
                            upstream_name
                        );
                        return false;
                    }
                    if !validate_external_auth_headers(
                        upstream_name,
                        "auth.external_auth.oidc",
                        request_headers,
                        response_header_allowlist,
                    ) {
                        return false;
                    }
                    if !response_header_allowlist.is_empty() {
                        validation_error!(
                            "upstream '{}' auth.external_auth.oidc.response_header_allowlist is not supported in v1",
                            upstream_name
                        );
                        return false;
                    }
                    if *timeout_ms == 0 {
                        validation_error!(
                            "upstream '{}' auth.external_auth.oidc.timeout_ms must be greater than 0",
                            upstream_name
                        );
                        return false;
                    }
                }
            }
        }

        if let Some(jwt) = upstream.auth.jwt.as_ref() {
            if jwt.secret.trim().is_empty() {
                validation_error!(
                    "upstream '{}' auth.jwt.secret must be non-empty",
                    upstream_name
                );
                return false;
            }
            if jwt
                .issuer
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            {
                validation_error!(
                    "upstream '{}' auth.jwt.issuer must be non-empty when provided",
                    upstream_name
                );
                return false;
            }
            if jwt
                .audience
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            {
                validation_error!(
                    "upstream '{}' auth.jwt.audience must be non-empty when provided",
                    upstream_name
                );
                return false;
            }
        }
        if upstream
            .auth
            .required_scopes
            .iter()
            .any(|value| value.trim().is_empty())
        {
            validation_error!(
                "upstream '{}' auth.required_scopes must not contain empty values",
                upstream_name
            );
            return false;
        }
        if upstream
            .auth
            .required_roles
            .iter()
            .any(|value| value.trim().is_empty())
        {
            validation_error!(
                "upstream '{}' auth.required_roles must not contain empty values",
                upstream_name
            );
            return false;
        }
        if (!upstream.auth.required_scopes.is_empty() || !upstream.auth.required_roles.is_empty())
            && upstream.auth.jwt.is_none()
        {
            validation_error!(
                "upstream '{}' auth.required_scopes/auth.required_roles require auth.jwt",
                upstream_name
            );
            return false;
        }

        // Validate backends
        if upstream.backends.is_empty() {
            validation_error!("Upstream '{}' has no backends configured", upstream_name);
            return false;
        }

        let mut upstream_uses_https_backends = false;
        for backend in &upstream.backends {
            // Validate backend ID
            if backend.id.is_empty() {
                validation_error!("Backend ID is empty in upstream '{}'", upstream_name);
                return false;
            }

            // Validate backend address
            if backend.address.is_empty() {
                validation_error!(
                    "Backend address is empty for backend '{}' in upstream '{}'",
                    backend.id,
                    upstream_name
                );
                return false;
            }

            let endpoint = match BackendEndpoint::parse(&backend.address) {
                Ok(endpoint) => endpoint,
                Err(reason) => {
                    validation_error!(
                        "Backend address '{}' in upstream '{}' is invalid: {}",
                        backend.address,
                        upstream_name,
                        reason
                    );
                    return false;
                }
            };
            if endpoint.scheme() == BackendScheme::Http {
                warn!(
                    "Backend '{}' in upstream '{}' uses explicit insecure cleartext transport ({})",
                    backend.id, upstream_name, backend.address
                );
            } else {
                upstream_uses_https_backends = true;
            }

            let origin = endpoint.origin();
            if let Some((existing_upstream, existing_backend)) = seen_backend_origins
                .insert(origin.clone(), (upstream_name.clone(), backend.id.clone()))
            {
                validation_error!(
                    "Duplicate backend address '{}' detected: upstream '{}' backend '{}' conflicts with upstream '{}' backend '{}'",
                    origin,
                    upstream_name,
                    backend.id,
                    existing_upstream,
                    existing_backend
                );
                return false;
            }

            // Validate weight
            if backend.weight == 0 || backend.weight > 1000 {
                validation_error!(
                    "Backend '{}' in upstream '{}' has invalid weight {} (must be 1–1000)",
                    backend.id,
                    upstream_name,
                    backend.weight
                );
                return false;
            }

            // Validate health check (optional — omitting it disables active health checks)
            if let Some(hc) = &backend.health_check {
                if hc.interval == 0 {
                    validation_error!(
                        "Health check interval is invalid (0) for backend '{}' in upstream '{}'",
                        backend.id,
                        upstream_name
                    );
                    return false;
                }

                if hc.timeout_ms == 0 {
                    validation_error!(
                        "Health check timeout is invalid (0) for backend '{}' in upstream '{}'",
                        backend.id,
                        upstream_name
                    );
                    return false;
                }

                if hc.failure_threshold == 0 {
                    validation_error!(
                        "Health check failure threshold is invalid (0) for backend '{}' in upstream '{}'",
                        backend.id,
                        upstream_name
                    );
                    return false;
                }

                if hc.success_threshold == 0 {
                    validation_error!(
                        "Health check success threshold is invalid (0) for backend '{}' in upstream '{}'",
                        backend.id,
                        upstream_name
                    );
                    return false;
                }

                if hc.cooldown_ms == 0 {
                    validation_error!(
                        "Health check cooldown is invalid (0) for backend '{}' in upstream '{}'",
                        backend.id,
                        upstream_name
                    );
                    return false;
                }
            }
        }

        if upstream_uses_https_backends {
            if let Some(tls) = upstream.tls.as_ref() {
                if !validate_upstream_tls(&format!("upstream['{}'].tls", upstream_name), tls) {
                    return false;
                }
            } else {
                validate_global_upstream_tls = true;
            }
        }
    }

    if validate_global_upstream_tls && !validate_upstream_tls("upstream_tls", &config.upstream_tls)
    {
        return false;
    }

    info!("Configuration validation passed successfully\n");
    true
}

#[cfg(test)]
mod tests;
