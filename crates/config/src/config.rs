use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::default::{
    auth_default_external_timeout_ms, get_default_load_balancing, get_default_version,
    upstream_tls_default_strict_sni, upstream_tls_default_verify_certificates,
};

pub const CURRENT_CONFIG_VERSION: u32 = 1;
pub const SUPPORTED_CONFIG_VERSIONS: &[u32] = &[CURRENT_CONFIG_VERSION];

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "get_default_version")] // Make version optional with default
    pub version: u32,

    pub listen: Listen,

    #[serde(default)]
    pub listeners: Vec<Listen>,

    pub upstream: HashMap<String, Upstream>,

    #[serde(default)]
    pub load_balancing: Option<LoadBalancing>, // Global fallback load balancing

    #[serde(default)]
    pub upstream_tls: UpstreamTls,

    #[serde(default)]
    pub log: Log,

    #[serde(default)]
    pub performance: Performance,

    #[serde(default)]
    pub observability: Observability,

    #[serde(default)]
    pub resilience: Resilience,

    #[serde(default)]
    pub security: Security,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Security {
    #[serde(default)]
    pub privileges: PrivilegeDrop,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct PrivilegeDrop {
    pub enabled: bool,
    pub user: String,
    pub group: String,
}

impl Default for PrivilegeDrop {
    fn default() -> Self {
        Self {
            enabled: true,
            user: "nobody".to_string(),
            group: "nogroup".to_string(),
        }
    }
}

pub fn effective_listens(config: &Config) -> Vec<Listen> {
    if config.listeners.is_empty() {
        vec![config.listen.clone()]
    } else {
        config.listeners.clone()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Listen {
    pub protocol: String, // "http3"

    pub port: u16, // 9889

    pub address: String, // "0.0.0.0"
    pub tls: Tls,
}

impl Default for Listen {
    fn default() -> Self {
        Self {
            protocol: "http3".to_string(),
            port: 9889,
            address: "0.0.0.0".to_string(),
            tls: Tls::default(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Tls {
    #[serde(default)]
    pub cert: String, // "/path/to/cert"
    #[serde(default)]
    pub key: String, // "/path/to/key"
    #[serde(default)]
    pub certificates: Vec<TlsCertificate>, // SNI keyed certificate set
    #[serde(default)]
    pub client_auth: ClientAuth,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct TlsCertificate {
    pub server_name: String, // "api.example.com"
    pub cert: String,        // "/path/to/cert"
    pub key: String,         // "/path/to/key"
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ClientAuth {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub require_client_cert: bool,
    #[serde(default)]
    pub ca_file: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct UpstreamTls {
    #[serde(default = "upstream_tls_default_verify_certificates")]
    pub verify_certificates: bool,
    #[serde(default = "upstream_tls_default_strict_sni")]
    pub strict_sni: bool,
    #[serde(default)]
    pub ca_file: Option<String>,
    #[serde(default)]
    pub ca_dir: Option<String>,
}

impl Default for UpstreamTls {
    fn default() -> Self {
        Self {
            verify_certificates: upstream_tls_default_verify_certificates(),
            strict_sni: upstream_tls_default_strict_sni(),
            ca_file: None,
            ca_dir: None,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Upstream {
    #[serde(default = "get_default_load_balancing")]
    pub load_balancing: LoadBalancing,

    #[serde(default)]
    pub auth: RouteAuth,

    #[serde(default)]
    pub host_policy: UpstreamHostPolicy,

    #[serde(default)]
    pub forwarded_headers: ForwardedHeaderPolicy,

    #[serde(default)]
    pub tls: Option<UpstreamTls>,

    pub route: RouteMatch, // Route matching criteria for this upstream

    pub backends: Vec<Backend>,
}

/// Upstream-scoped auth policy. External auth is a single-provider contract
/// per upstream; there is no provider chaining.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct RouteAuth {
    #[serde(default)]
    pub api_key: Option<ApiKeyAuth>,
    #[serde(default)]
    pub jwt: Option<JwtAuth>,
    #[serde(default)]
    pub external_auth: Option<ExternalAuth>,
    #[serde(default)]
    pub required_scopes: Vec<String>,
    #[serde(default)]
    pub required_roles: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExternalAuthFailureMode {
    FailOpen,
    #[default]
    FailClosed,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalAuth {
    Http {
        endpoint: String,
        #[serde(default)]
        request_headers: Vec<ExternalAuthRequestHeader>,
        #[serde(default)]
        response_header_allowlist: Vec<String>,
        #[serde(default = "auth_default_external_timeout_ms")]
        timeout_ms: u64,
        #[serde(default = "ExternalAuthFailureMode::default")]
        failure_mode: ExternalAuthFailureMode,
    },
    Oidc {
        #[serde(default)]
        discovery_url: Option<String>,
        #[serde(default)]
        issuer_url: Option<String>,
        client_id: String,
        #[serde(default)]
        client_secret: Option<String>,
        #[serde(default)]
        audience: Option<String>,
        #[serde(default)]
        scopes: Vec<String>,
        #[serde(default)]
        request_headers: Vec<ExternalAuthRequestHeader>,
        #[serde(default)]
        response_header_allowlist: Vec<String>,
        #[serde(default = "auth_default_external_timeout_ms")]
        timeout_ms: u64,
        #[serde(default = "ExternalAuthFailureMode::default")]
        failure_mode: ExternalAuthFailureMode,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ExternalAuthRequestHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyAuth {
    pub header_name: String,
    #[serde(default)]
    pub keys: Vec<String>,
}

impl Default for ApiKeyAuth {
    fn default() -> Self {
        Self {
            header_name: "x-api-key".to_string(),
            keys: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct JwtAuth {
    pub secret: String,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    pub clock_skew_secs: u64,
}

impl Default for JwtAuth {
    fn default() -> Self {
        Self {
            secret: String::new(),
            issuer: None,
            audience: None,
            clock_skew_secs: 30,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamHostPolicyMode {
    #[default]
    #[serde(alias = "pass-through")]
    PassThrough,
    Rewrite,
    #[serde(
        alias = "static_upstream",
        alias = "static-upstream",
        alias = "upstream-host"
    )]
    Upstream,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ForwardedHeaderPolicyMode {
    #[default]
    #[serde(alias = "overwrite")]
    Overwrite,
    Append,
    Preserve,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ForwardedHeaderPolicy {
    pub mode: ForwardedHeaderPolicyMode,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct UpstreamHostPolicy {
    pub mode: UpstreamHostPolicyMode,
    pub host: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Backend {
    pub id: String, // "backend1"
    /// Backend endpoint.
    /// - `host:port` (defaults to verified HTTPS)
    /// - `https://host:port` (verified HTTPS)
    /// - `http://host:port` (explicit insecure opt-out)
    pub address: String,

    #[serde(default = "Backend::default_weight")]
    pub weight: u32, // 100
    #[serde(default)]
    pub health_check: Option<HealthCheck>,
}

impl Backend {
    pub(crate) fn default_weight() -> u32 {
        100
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct RouteMatch {
    #[serde(default)]
    pub host: Option<String>, // host-based routing (e.g., "api.example.com")

    #[serde(default)]
    pub path_prefix: Option<String>, // path prefix matching (e.g., "/api")

    #[serde(default)]
    pub method: Option<String>, // Optional HTTP method filtering (GET, POST, etc.)
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    pub path: String, // "/health"

    pub interval: u64, // "5000" (write in number of milli seconds)

    pub timeout_ms: u64,

    pub failure_threshold: u32,

    pub success_threshold: u32,

    pub cooldown_ms: u64,
}

impl Default for HealthCheck {
    fn default() -> Self {
        Self {
            path: "/health".to_string(),
            interval: 5_000,
            timeout_ms: 1_000,
            failure_threshold: 3,
            success_threshold: 2,
            cooldown_ms: 5_000,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct LoadBalancing {
    #[serde(rename = "type")]
    pub lb_type: String, // "random","round_robin","consistent_hash","least_connections","latency_aware","sticky_cid"

    // Configurable key source for hash-based/sticky load balancing.
    #[serde(default)]
    pub key: Option<String>, // Examples: header:x-user-id, cookie:session_id, query:user_id
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Log {
    // whisper -> trace
    // haunt -> debug
    // spooky -> info
    // scream -> warn
    // poltergeist -> error
    // silence -> off
    pub level: String, // "info, warn, error"

    pub file: LogFile,

    pub format: LogFormat,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            file: LogFile::default(),
            format: LogFormat::Plain,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Plain,
    Json,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct LogFile {
    pub enabled: bool,

    pub path: String,
}

impl Default for LogFile {
    fn default() -> Self {
        Self {
            enabled: false,
            path: "/var/log/spooky/spooky.log".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Performance {
    pub worker_threads: usize,

    /// Tokio worker threads used by the control-plane runtime.
    pub control_plane_threads: usize,

    /// Number of packet-processing shards per bound UDP worker socket.
    /// `1` preserves single-loop behavior; values >1 enable parallel shard workers.
    pub packet_shards_per_worker: usize,

    /// Capacity of bounded ingress queue per shard.
    pub packet_shard_queue_capacity: usize,

    /// Memory-aware cap for queued datagram bytes per ingress shard dispatch queue.
    pub packet_shard_queue_max_bytes: usize,

    pub reuseport: bool,

    pub pin_workers: bool,

    pub global_inflight_limit: usize,

    pub per_upstream_inflight_limit: usize,

    /// Optional micro-wait before shedding on global/upstream inflight permit acquisition.
    /// `0` disables waiting and preserves immediate-shed behavior.
    pub inflight_acquire_wait_ms: u64,

    pub backend_timeout_ms: u64,

    pub backend_connect_timeout_ms: u64,

    pub backend_body_idle_timeout_ms: u64,

    pub backend_body_total_timeout_ms: u64,

    pub backend_total_request_timeout_ms: u64,

    pub shutdown_drain_timeout_ms: u64,

    pub udp_recv_buffer_bytes: usize,

    pub udp_send_buffer_bytes: usize,

    pub h2_pool_max_idle_per_backend: usize,

    pub h2_pool_idle_timeout_ms: u64,

    /// Enables periodic DNS refresh for hostname-based upstream backends.
    pub backend_dns_refresh_enabled: bool,

    /// Control-plane interval for refreshing hostname-based backend DNS records.
    pub backend_dns_refresh_interval_ms: u64,

    pub per_backend_inflight_limit: usize,

    /// Steady-state new QUIC connections allowed per second (token-bucket refill rate).
    pub new_connections_per_sec: u32,

    /// Maximum burst of new QUIC connections above the steady-state rate.
    /// Must be >= 1; values below 1 are clamped to 1 at runtime.
    pub new_connections_burst: u32,

    /// Hard cap on concurrently tracked active QUIC connections per worker.
    /// New Initial packets above this cap are dropped deterministically.
    pub max_active_connections: usize,

    /// QUIC idle timeout: connection is closed after this many ms of inactivity.
    pub quic_max_idle_timeout_ms: u64,

    /// QUIC connection-level flow control: total bytes the client may send before
    /// receiving a MAX_DATA frame.
    pub quic_initial_max_data: u64,

    /// QUIC stream-level flow control: bytes allowed per stream (bidi and uni).
    /// Must be <= `quic_initial_max_data`.
    pub quic_initial_max_stream_data: u64,

    /// Maximum number of concurrent bidirectional streams per connection.
    pub quic_initial_max_streams_bidi: u64,

    /// Maximum number of concurrent unidirectional streams per connection.
    pub quic_initial_max_streams_uni: u64,

    /// Hard cap on upstream response body bytes per stream.
    /// Streams whose response body exceeds this size are terminated with 502.
    /// Protects against runaway or adversarial upstreams streaming unboundedly.
    pub max_response_body_bytes: usize,

    /// Hard cap on request body bytes per stream.
    /// Requests exceeding this size are rejected with 413.
    pub max_request_body_bytes: usize,

    /// Global cap for bytes buffered in request backpressure queues across a worker.
    pub request_buffer_global_cap_bytes: usize,

    /// Max bytes buffered for unknown-length upstream responses before headers are emitted.
    /// Responses exceeding this prebuffer cap are terminated with overload response.
    pub unknown_length_response_prebuffer_bytes: usize,

    /// Idle timeout for request body upload progress.
    /// If no request-body bytes arrive for this period, the stream is failed.
    pub client_body_idle_timeout_ms: u64,
}

impl Default for Performance {
    fn default() -> Self {
        Self {
            worker_threads: 1,
            control_plane_threads: 2,
            packet_shards_per_worker: 1,
            packet_shard_queue_capacity: 2048,
            packet_shard_queue_max_bytes: 64 * 1024 * 1024,
            reuseport: true,
            pin_workers: false,
            global_inflight_limit: 4096,
            per_upstream_inflight_limit: 1024,
            inflight_acquire_wait_ms: 0,
            backend_timeout_ms: 2_000,
            backend_connect_timeout_ms: 500,
            backend_body_idle_timeout_ms: 2_000,
            backend_body_total_timeout_ms: 30_000,
            backend_total_request_timeout_ms: 35_000,
            shutdown_drain_timeout_ms: 5_000,
            udp_recv_buffer_bytes: 8 * 1024 * 1024,
            udp_send_buffer_bytes: 8 * 1024 * 1024,
            h2_pool_max_idle_per_backend: 256,
            h2_pool_idle_timeout_ms: 90_000,
            backend_dns_refresh_enabled: false,
            backend_dns_refresh_interval_ms: 30_000,
            per_backend_inflight_limit: 64,
            new_connections_per_sec: 2000,
            new_connections_burst: 500,
            max_active_connections: 20_000,
            quic_max_idle_timeout_ms: 5_000,
            quic_initial_max_data: 10_000_000,
            quic_initial_max_stream_data: 1_000_000,
            quic_initial_max_streams_bidi: 100,
            quic_initial_max_streams_uni: 100,
            max_response_body_bytes: 100 * 1024 * 1024,
            max_request_body_bytes: 1_000_000,
            request_buffer_global_cap_bytes: 64 * 1024 * 1024,
            unknown_length_response_prebuffer_bytes: 2 * 1024 * 1024,
            client_body_idle_timeout_ms: 10_000,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Resilience {
    #[serde(default)]
    pub adaptive_admission: AdaptiveAdmission,
    #[serde(default)]
    pub route_queue: RouteQueue,
    #[serde(default)]
    pub scoped_rate_limits: Vec<ScopedRateLimit>,
    #[serde(default)]
    pub protocol: ProtocolPolicy,
    #[serde(default)]
    pub circuit_breaker: CircuitBreaker,
    #[serde(default)]
    pub hedging: Hedging,
    #[serde(default)]
    pub retry_budget: RetryBudget,
    #[serde(default)]
    pub brownout: Brownout,
    #[serde(default)]
    pub watchdog: Watchdog,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveAdmission {
    pub enabled: bool,
    pub min_limit: usize,
    pub max_limit: Option<usize>,
    pub decrease_step: usize,
    pub increase_step: usize,
    pub high_latency_ms: u64,
}

impl Resilience {
    pub fn validate(&self) -> Result<(), String> {
        if self.brownout.recover_inflight_percent >= self.brownout.trigger_inflight_percent {
            return Err(format!(
                "resilience.brownout: recover_inflight_percent ({}) must be \
                 less than trigger_inflight_percent ({})",
                self.brownout.recover_inflight_percent, self.brownout.trigger_inflight_percent,
            ));
        }
        if self.adaptive_admission.min_limit == 0 {
            return Err("resilience.adaptive_admission: min_limit must be > 0".into());
        }
        if let Some(max_limit) = self.adaptive_admission.max_limit {
            if max_limit == 0 {
                return Err(
                    "resilience.adaptive_admission: max_limit must be > 0 when provided".into(),
                );
            }
            if max_limit < self.adaptive_admission.min_limit {
                return Err(format!(
                    "resilience.adaptive_admission: max_limit ({}) must be >= min_limit ({})",
                    max_limit, self.adaptive_admission.min_limit
                ));
            }
        }
        if self.retry_budget.ratio_percent > 100 {
            return Err(format!(
                "resilience.retry_budget: ratio_percent ({}) must be 0-100",
                self.retry_budget.ratio_percent,
            ));
        }
        for rule in &self.scoped_rate_limits {
            let rule_name = rule.name.trim();
            if rule_name.is_empty() {
                return Err("resilience.scoped_rate_limits[].name must be non-empty".into());
            }
            if rule.requests_per_sec == 0 {
                return Err(format!(
                    "resilience.scoped_rate_limits['{}'].requests_per_sec must be > 0",
                    rule_name
                ));
            }
            if rule.burst == 0 {
                return Err(format!(
                    "resilience.scoped_rate_limits['{}'].burst must be > 0",
                    rule_name
                ));
            }
            if rule.idle_ttl_secs == 0 {
                return Err(format!(
                    "resilience.scoped_rate_limits['{}'].idle_ttl_secs must be > 0",
                    rule_name
                ));
            }
            if rule
                .route_allowlist
                .iter()
                .any(|route| route.trim().is_empty())
            {
                return Err(format!(
                    "resilience.scoped_rate_limits['{}'].route_allowlist must not contain empty values",
                    rule_name
                ));
            }
            match rule.scope {
                ScopedRateLimitScope::Route => {
                    if rule
                        .key
                        .as_ref()
                        .is_some_and(|value| !value.trim().is_empty())
                    {
                        return Err(format!(
                            "resilience.scoped_rate_limits['{}'].key is invalid for scope=route",
                            rule_name
                        ));
                    }
                }
                ScopedRateLimitScope::Tenant => {
                    let Some(key_spec) = rule.key.as_deref() else {
                        return Err(format!(
                            "resilience.scoped_rate_limits['{}'].key is required for scope=tenant",
                            rule_name
                        ));
                    };
                    if !matches!(
                        key_spec.trim().to_ascii_lowercase().as_str(),
                        "path"
                            | "authority"
                            | "method"
                            | "cid"
                            | "sticky-cid"
                            | "peer_ip"
                            | "client_ip"
                            | "bearer_token"
                    ) && !key_spec.split_once(':').is_some_and(|(source, key_name)| {
                        !key_name.trim().is_empty()
                            && matches!(
                                source.trim().to_ascii_lowercase().as_str(),
                                "header" | "cookie" | "query"
                            )
                    }) {
                        return Err(format!(
                            "resilience.scoped_rate_limits['{}'].key must be a supported request key spec",
                            rule_name
                        ));
                    }
                }
                ScopedRateLimitScope::Client | ScopedRateLimitScope::Token => {
                    if let Some(key_spec) = rule.key.as_deref()
                        && !matches!(
                            key_spec.trim().to_ascii_lowercase().as_str(),
                            "path"
                                | "authority"
                                | "method"
                                | "cid"
                                | "sticky-cid"
                                | "peer_ip"
                                | "client_ip"
                                | "bearer_token"
                        )
                        && !key_spec.split_once(':').is_some_and(|(source, key_name)| {
                            !key_name.trim().is_empty()
                                && matches!(
                                    source.trim().to_ascii_lowercase().as_str(),
                                    "header" | "cookie" | "query"
                                )
                        })
                    {
                        return Err(format!(
                            "resilience.scoped_rate_limits['{}'].key must be a supported request key spec",
                            rule_name
                        ));
                    }
                }
            }
        }
        if self.hedging.enabled && self.hedging.delay_ms == 0 {
            return Err("resilience.hedging: delay_ms must be > 0 when hedging is enabled".into());
        }
        Ok(())
    }
}

impl Default for AdaptiveAdmission {
    fn default() -> Self {
        Self {
            enabled: true,
            min_limit: 64,
            max_limit: None,
            decrease_step: 16,
            increase_step: 16,
            high_latency_ms: 500,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct RouteQueue {
    pub default_cap: usize,
    pub global_cap: usize,
    pub shed_retry_after_seconds: u32,
    pub caps: HashMap<String, usize>,
}

impl Default for RouteQueue {
    fn default() -> Self {
        Self {
            default_cap: 512,
            global_cap: 2048,
            shed_retry_after_seconds: 1,
            caps: HashMap::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScopedRateLimitScope {
    Route,
    Client,
    Tenant,
    Token,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ScopedRateLimit {
    pub name: String,
    pub scope: ScopedRateLimitScope,
    pub requests_per_sec: u32,
    pub burst: u32,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub route_allowlist: Vec<String>,
    #[serde(default = "ScopedRateLimit::default_idle_ttl_secs")]
    pub idle_ttl_secs: u64,
}

impl ScopedRateLimit {
    pub(crate) fn default_idle_ttl_secs() -> u64 {
        300
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ProtocolPolicy {
    pub allow_0rtt: bool,
    pub allow_connect: bool,
    pub early_data_safe_methods: Vec<String>,
    pub max_headers_count: usize,
    pub max_headers_bytes: usize,
    pub enforce_authority_host_match: bool,
    pub allowed_methods: Vec<String>,
    pub denied_path_prefixes: Vec<String>,
    pub connect_allowed_ports: Vec<u16>,
    pub connect_allowed_authorities: Vec<String>,
}

impl Default for ProtocolPolicy {
    fn default() -> Self {
        Self {
            allow_0rtt: false,
            allow_connect: false,
            early_data_safe_methods: vec!["GET".to_string(), "HEAD".to_string()],
            max_headers_count: 128,
            max_headers_bytes: 16 * 1024,
            enforce_authority_host_match: true,
            allowed_methods: Vec::new(),
            denied_path_prefixes: Vec::new(),
            connect_allowed_ports: Vec::new(),
            connect_allowed_authorities: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct CircuitBreaker {
    pub enabled: bool,
    pub failure_threshold: u32,
    pub open_ms: u64,
    pub half_open_max_probes: u32,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self {
            enabled: true,
            failure_threshold: 3,
            open_ms: 30_000,
            half_open_max_probes: 1,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Hedging {
    pub enabled: bool,
    pub delay_ms: u64,
    pub safe_methods: Vec<String>,
    pub route_allowlist: Vec<String>,
}

impl Default for Hedging {
    fn default() -> Self {
        Self {
            enabled: false,
            delay_ms: 100,
            safe_methods: vec!["GET".to_string(), "HEAD".to_string()],
            route_allowlist: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct RetryBudget {
    pub enabled: bool,
    pub ratio_percent: u8,
    pub per_route_ratio_percent: HashMap<String, u8>,
}

impl Default for RetryBudget {
    fn default() -> Self {
        Self {
            enabled: true,
            ratio_percent: 10,
            per_route_ratio_percent: HashMap::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Brownout {
    pub enabled: bool,
    pub trigger_inflight_percent: u8,
    pub recover_inflight_percent: u8,
    pub core_routes: Vec<String>,
}

impl Default for Brownout {
    fn default() -> Self {
        Self {
            enabled: true,
            trigger_inflight_percent: 90,
            recover_inflight_percent: 60,
            core_routes: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Watchdog {
    pub enabled: bool,
    pub check_interval_ms: u64,
    pub poll_stall_timeout_ms: u64,
    pub timeout_error_rate_percent: u8,
    pub min_requests_per_window: u64,
    pub overload_inflight_percent: u8,
    pub unhealthy_consecutive_windows: u32,
    pub drain_grace_ms: u64,
    pub restart_cooldown_ms: u64,

    /// Structured restart hook command: first element is executable, rest are args.
    /// Preferred over `restart_hook` because it avoids shell evaluation.
    pub restart_command: Vec<String>,

    /// Legacy shell command restart hook.
    /// Deprecated: use `restart_command` instead.
    pub restart_hook: Option<String>,
}

impl Default for Watchdog {
    fn default() -> Self {
        Self {
            enabled: false,
            check_interval_ms: 1_000,
            poll_stall_timeout_ms: 5_000,
            timeout_error_rate_percent: 60,
            min_requests_per_window: 20,
            overload_inflight_percent: 95,
            unhealthy_consecutive_windows: 3,
            drain_grace_ms: 8_000,
            restart_cooldown_ms: 120_000,
            restart_command: Vec::new(),
            restart_hook: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Observability {
    #[serde(default)]
    pub metrics: MetricsEndpoint,
    #[serde(default)]
    pub control_api: ControlApi,
    #[serde(default)]
    pub tracing: Tracing,
    #[serde(default)]
    pub routing: RoutingTransparency,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct MetricsEndpoint {
    pub enabled: bool,

    /// When true, startup fails if metrics endpoint cannot be bound/registered.
    pub required: bool,

    pub address: String,

    pub port: u16,

    pub path: String,

    pub max_connections: usize,

    pub connection_timeout_ms: u64,
}

impl Default for MetricsEndpoint {
    fn default() -> Self {
        Self {
            enabled: false,
            required: false,
            address: "127.0.0.1".to_string(),
            port: 9901,
            path: "/metrics".to_string(),
            max_connections: 512,
            connection_timeout_ms: 30_000,
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ControlApi {
    pub enabled: bool,

    /// When true, startup fails if control API endpoint cannot be bound/registered.
    pub required: bool,

    pub address: String,

    pub port: u16,

    pub health_path: String,

    pub ready_path: String,

    pub runtime_path: String,

    pub restart_path: String,

    pub reload_path: String,

    pub reload_certs_path: String,

    // Admin credential: never emitted by Serialize (e.g. the /admin/runtime
    // dump) and redacted in Debug; still accepted on deserialize.
    #[serde(default, skip_serializing)]
    pub auth_token: Option<String>,

    pub max_connections: usize,

    pub connection_timeout_ms: u64,
}

impl Default for ControlApi {
    fn default() -> Self {
        Self {
            enabled: false,
            required: false,
            address: "127.0.0.1".to_string(),
            port: 9902,
            health_path: "/health".to_string(),
            ready_path: "/ready".to_string(),
            runtime_path: "/admin/runtime".to_string(),
            restart_path: "/admin/runtime/restart".to_string(),
            reload_path: "/admin/runtime/reload".to_string(),
            reload_certs_path: "/admin/runtime/reload-certs".to_string(),
            auth_token: None,
            max_connections: 256,
            connection_timeout_ms: 30_000,
        }
    }
}

impl std::fmt::Debug for ControlApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlApi")
            .field("enabled", &self.enabled)
            .field("required", &self.required)
            .field("address", &self.address)
            .field("port", &self.port)
            .field("health_path", &self.health_path)
            .field("ready_path", &self.ready_path)
            .field("runtime_path", &self.runtime_path)
            .field("restart_path", &self.restart_path)
            .field("reload_path", &self.reload_path)
            .field("reload_certs_path", &self.reload_certs_path)
            // Redacted: show presence, never the value.
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "<redacted>"),
            )
            .field("max_connections", &self.max_connections)
            .field("connection_timeout_ms", &self.connection_timeout_ms)
            .finish()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Tracing {
    pub enabled: bool,

    pub service_name: String,

    pub otlp_endpoint: Option<String>,

    pub sample_ratio: f64,
}

impl Default for Tracing {
    fn default() -> Self {
        Self {
            enabled: false,
            service_name: "spooky".to_string(),
            otlp_endpoint: None,
            sample_ratio: 1.0,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct RoutingTransparency {
    pub enabled: bool,
    pub include_reason: bool,
    pub expose_header: bool,
    pub header_name: String,
}

impl Default for RoutingTransparency {
    fn default() -> Self {
        Self {
            enabled: false,
            include_reason: true,
            expose_header: false,
            header_name: "x-spooky-route-decision".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApiKeyAuth, Config, ControlApi, ForwardedHeaderPolicy, JwtAuth, Listen, Log,
        MetricsEndpoint, Performance, PrivilegeDrop, Resilience, RoutingTransparency, Tracing,
        UpstreamHostPolicy, Watchdog,
    };

    #[test]
    fn minimal_yaml_applies_documented_defaults() {
        let yaml = r#"
listen:
  tls: {}
upstream:
  api:
    route: {}
    backends:
      - id: backend1
        address: "http://127.0.0.1:7001"
"#;

        let config: Config = serde_yaml::from_str(yaml).expect("minimal config should parse");
        let upstream = config.upstream.get("api").expect("missing api upstream");
        let backend = upstream
            .backends
            .first()
            .expect("missing backend in minimal config");

        assert_eq!(config.version, 1);
        assert_eq!(config.listen.protocol, "http3");
        assert_eq!(config.listen.port, 9889);
        assert_eq!(config.listen.address, "0.0.0.0");
        assert_eq!(config.log.level, "info");
        assert_eq!(backend.weight, 100);
    }

    #[test]
    fn backend_health_check_defaults_are_filled_by_serde() {
        let yaml = r#"
listen:
  tls: {}
upstream:
  api:
    route: {}
    backends:
      - id: backend1
        address: "http://127.0.0.1:7001"
        health_check: {}
"#;

        let config: Config =
            serde_yaml::from_str(yaml).expect("config with empty health_check should parse");
        let health_check = config.upstream["api"].backends[0]
            .health_check
            .as_ref()
            .expect("missing defaulted health check");

        assert_eq!(health_check.path, "/health");
        assert_eq!(health_check.interval, 5_000);
        assert_eq!(health_check.timeout_ms, 1_000);
        assert_eq!(health_check.failure_threshold, 3);
        assert_eq!(health_check.success_threshold, 2);
        assert_eq!(health_check.cooldown_ms, 5_000);
    }

    #[test]
    fn privilege_drop_defaults_are_filled_by_serde_via_type_default() {
        let yaml = r#"
listen:
  tls: {}
upstream:
  api:
    route: {}
    backends:
      - id: backend1
        address: "http://127.0.0.1:7001"
security:
  privileges: {}
"#;

        let config: Config =
            serde_yaml::from_str(yaml).expect("config with empty privileges should parse");

        assert_eq!(config.security.privileges.enabled, PrivilegeDrop::default().enabled);
        assert_eq!(config.security.privileges.user, PrivilegeDrop::default().user);
        assert_eq!(config.security.privileges.group, PrivilegeDrop::default().group);
    }

    #[test]
    fn serde_defaults_for_leaf_structs_match_type_defaults() {
        let listen: Listen =
            serde_yaml::from_str("{}").expect("empty listen should parse via type defaults");
        assert_eq!(listen.protocol, Listen::default().protocol);
        assert_eq!(listen.port, Listen::default().port);
        assert_eq!(listen.address, Listen::default().address);

        let log: Log = serde_yaml::from_str("{}").expect("empty log should parse via type defaults");
        assert_eq!(log.level, Log::default().level);
        assert_eq!(log.file, Log::default().file);
        assert_eq!(log.format, Log::default().format);

        let api_key: ApiKeyAuth =
            serde_yaml::from_str("{}").expect("empty api key auth should parse via type defaults");
        assert_eq!(api_key.header_name, ApiKeyAuth::default().header_name);
        assert_eq!(api_key.keys, ApiKeyAuth::default().keys);

        let jwt: JwtAuth =
            serde_yaml::from_str(r#"secret: test-secret"#).expect("jwt auth should parse");
        assert_eq!(jwt.issuer, JwtAuth::default().issuer);
        assert_eq!(jwt.audience, JwtAuth::default().audience);
        assert_eq!(jwt.clock_skew_secs, JwtAuth::default().clock_skew_secs);
    }

    #[test]
    fn serde_defaults_for_small_policy_structs_match_type_defaults() {
        let forwarded: ForwardedHeaderPolicy =
            serde_yaml::from_str("{}").expect("empty forwarded header policy should parse");
        assert_eq!(forwarded, ForwardedHeaderPolicy::default());

        let host_policy: UpstreamHostPolicy =
            serde_yaml::from_str("{}").expect("empty upstream host policy should parse");
        assert_eq!(host_policy, UpstreamHostPolicy::default());

        let metrics: MetricsEndpoint =
            serde_yaml::from_str("{}").expect("empty metrics endpoint should parse");
        assert_eq!(metrics.address, MetricsEndpoint::default().address);
        assert_eq!(metrics.port, MetricsEndpoint::default().port);
        assert_eq!(metrics.path, MetricsEndpoint::default().path);
        assert_eq!(
            metrics.max_connections,
            MetricsEndpoint::default().max_connections
        );
        assert_eq!(
            metrics.connection_timeout_ms,
            MetricsEndpoint::default().connection_timeout_ms
        );

        let control_api: ControlApi =
            serde_yaml::from_str("{}").expect("empty control api should parse");
        assert_eq!(control_api.address, ControlApi::default().address);
        assert_eq!(control_api.port, ControlApi::default().port);
        assert_eq!(control_api.health_path, ControlApi::default().health_path);
        assert_eq!(control_api.ready_path, ControlApi::default().ready_path);
        assert_eq!(control_api.runtime_path, ControlApi::default().runtime_path);
        assert_eq!(control_api.restart_path, ControlApi::default().restart_path);
        assert_eq!(control_api.reload_path, ControlApi::default().reload_path);
        assert_eq!(
            control_api.reload_certs_path,
            ControlApi::default().reload_certs_path
        );
        assert_eq!(
            control_api.max_connections,
            ControlApi::default().max_connections
        );
        assert_eq!(
            control_api.connection_timeout_ms,
            ControlApi::default().connection_timeout_ms
        );

        let tracing: Tracing =
            serde_yaml::from_str("{}").expect("empty tracing config should parse");
        assert_eq!(tracing.enabled, Tracing::default().enabled);
        assert_eq!(tracing.service_name, Tracing::default().service_name);
        assert_eq!(tracing.otlp_endpoint, Tracing::default().otlp_endpoint);
        assert_eq!(tracing.sample_ratio, Tracing::default().sample_ratio);

        let routing: RoutingTransparency =
            serde_yaml::from_str("{}").expect("empty routing transparency should parse");
        assert_eq!(routing.enabled, RoutingTransparency::default().enabled);
        assert_eq!(
            routing.include_reason,
            RoutingTransparency::default().include_reason
        );
        assert_eq!(
            routing.expose_header,
            RoutingTransparency::default().expose_header
        );
        assert_eq!(routing.header_name, RoutingTransparency::default().header_name);
    }

    #[test]
    fn serde_defaults_for_performance_match_type_defaults() {
        let performance: Performance =
            serde_yaml::from_str("{}").expect("empty performance config should parse");

        assert_eq!(performance.worker_threads, Performance::default().worker_threads);
        assert_eq!(
            performance.control_plane_threads,
            Performance::default().control_plane_threads
        );
        assert_eq!(
            performance.packet_shards_per_worker,
            Performance::default().packet_shards_per_worker
        );
        assert_eq!(
            performance.global_inflight_limit,
            Performance::default().global_inflight_limit
        );
        assert_eq!(
            performance.backend_timeout_ms,
            Performance::default().backend_timeout_ms
        );
        assert_eq!(
            performance.backend_dns_refresh_enabled,
            Performance::default().backend_dns_refresh_enabled
        );
        assert_eq!(
            performance.max_response_body_bytes,
            Performance::default().max_response_body_bytes
        );
        assert_eq!(
            performance.unknown_length_response_prebuffer_bytes,
            Performance::default().unknown_length_response_prebuffer_bytes
        );
    }

    #[test]
    fn serde_defaults_for_resilience_match_type_defaults() {
        let resilience: Resilience =
            serde_yaml::from_str("{}").expect("empty resilience config should parse");

        assert_eq!(
            resilience.adaptive_admission.enabled,
            Resilience::default().adaptive_admission.enabled
        );
        assert_eq!(
            resilience.route_queue.default_cap,
            Resilience::default().route_queue.default_cap
        );
        assert_eq!(
            resilience.protocol.max_headers_count,
            Resilience::default().protocol.max_headers_count
        );
        assert_eq!(
            resilience.hedging.delay_ms,
            Resilience::default().hedging.delay_ms
        );
        assert_eq!(
            resilience.retry_budget.ratio_percent,
            Resilience::default().retry_budget.ratio_percent
        );
        assert_eq!(
            resilience.brownout.trigger_inflight_percent,
            Resilience::default().brownout.trigger_inflight_percent
        );
        assert_eq!(
            resilience.watchdog.restart_cooldown_ms,
            Resilience::default().watchdog.restart_cooldown_ms
        );
    }

    #[test]
    fn watchdog_and_scoped_rate_limit_local_defaults_remain_stable() {
        let watchdog: Watchdog =
            serde_yaml::from_str("{}").expect("empty watchdog should parse via type defaults");
        assert_eq!(watchdog.enabled, Watchdog::default().enabled);
        assert_eq!(
            watchdog.unhealthy_consecutive_windows,
            Watchdog::default().unhealthy_consecutive_windows
        );
        assert_eq!(super::ScopedRateLimit::default_idle_ttl_secs(), 300);
    }
}
