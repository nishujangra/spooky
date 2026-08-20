use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub const CURRENT_CONFIG_VERSION: u32 = 1;
pub const SUPPORTED_CONFIG_VERSIONS: &[u32] = &[CURRENT_CONFIG_VERSION];

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "Config::default_version")] // Make version optional with default
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
    pub secrets: Secrets,

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

impl Config {
    fn default_version() -> u32 {
        CURRENT_CONFIG_VERSION
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Security {
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

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Secrets {
    pub default_provider: Option<String>,
    pub providers: HashMap<String, SecretProvider>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretProvider {
    File {
        #[serde(default)]
        base_dir: Option<String>,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SecretRef {
    #[serde(rename = "ref")]
    pub reference: String,
}

impl SecretRef {
    pub fn scheme(&self) -> Option<&str> {
        self.reference.split_once(':').map(|(scheme, _)| scheme)
    }

    pub fn raw_value(&self) -> &str {
        &self.reference
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
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Tls {
    pub cert: String,                      // "/path/to/cert"
    pub key: String,                       // "/path/to/key"
    pub certificates: Vec<TlsCertificate>, // SNI keyed certificate set
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
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ClientAuth {
    pub enabled: bool,
    pub require_client_cert: bool,
    pub ca_file: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct UpstreamTls {
    pub verify_certificates: bool,
    pub strict_sni: bool,
    pub ca_file: Option<String>,
    pub ca_dir: Option<String>,
    pub client_certificate: Option<String>,
    pub client_certificate_ref: Option<SecretRef>,
    pub client_key: Option<String>,
    pub client_key_ref: Option<SecretRef>,
}

impl Default for UpstreamTls {
    fn default() -> Self {
        Self {
            verify_certificates: true,
            strict_sni: true,
            ca_file: None,
            ca_dir: None,
            client_certificate: None,
            client_certificate_ref: None,
            client_key: None,
            client_key_ref: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Upstream {
    #[serde(default)]
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
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct RouteAuth {
    pub api_key: Option<ApiKeyAuth>,
    pub jwt: Option<JwtAuth>,
    pub external_auth: Option<ExternalAuth>,
    pub required_scopes: Vec<String>,
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
        #[serde(default = "ExternalAuth::default_timeout_ms")]
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
        client_secret_ref: Option<SecretRef>,
        #[serde(default)]
        audience: Option<String>,
        #[serde(default)]
        scopes: Vec<String>,
        #[serde(default)]
        request_headers: Vec<ExternalAuthRequestHeader>,
        #[serde(default)]
        response_header_allowlist: Vec<String>,
        #[serde(default = "ExternalAuth::default_timeout_ms")]
        timeout_ms: u64,
        #[serde(default = "ExternalAuthFailureMode::default")]
        failure_mode: ExternalAuthFailureMode,
    },
}

impl ExternalAuth {
    fn default_timeout_ms() -> u64 {
        1_000
    }
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
    pub secret_ref: Option<SecretRef>,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub issuers: Option<Vec<String>>,
    pub audiences: Option<Vec<String>>,
    #[serde(default = "JwtAuth::default_allowed_algorithms")]
    pub allowed_algorithms: Vec<JwtAlgorithm>,
    pub require_kid: bool,
    pub static_keys: Vec<JwtVerificationKey>,
    pub jwks_url: Option<String>,
    #[serde(default = "JwtAuth::default_jwks_refresh_interval_secs")]
    pub jwks_refresh_interval_secs: u64,
    #[serde(default = "JwtAuth::default_jwks_request_timeout_ms")]
    pub jwks_request_timeout_ms: u64,
    #[serde(default = "JwtAuth::default_jwks_cache_ttl_secs")]
    pub jwks_cache_ttl_secs: u64,
    #[serde(default = "JwtAuth::default_jwks_stale_if_error_secs")]
    pub jwks_stale_if_error_secs: u64,
    #[serde(default = "JwksStartupBehavior::default")]
    pub jwks_startup_behavior: JwksStartupBehavior,
    pub clock_skew_secs: u64,
}

impl Default for JwtAuth {
    fn default() -> Self {
        Self {
            secret: String::new(),
            secret_ref: None,
            issuer: None,
            audience: None,
            issuers: None,
            audiences: None,
            allowed_algorithms: Self::default_allowed_algorithms(),
            require_kid: false,
            static_keys: Vec::new(),
            jwks_url: None,
            jwks_refresh_interval_secs: Self::default_jwks_refresh_interval_secs(),
            jwks_request_timeout_ms: Self::default_jwks_request_timeout_ms(),
            jwks_cache_ttl_secs: Self::default_jwks_cache_ttl_secs(),
            jwks_stale_if_error_secs: Self::default_jwks_stale_if_error_secs(),
            jwks_startup_behavior: JwksStartupBehavior::default(),
            clock_skew_secs: 30,
        }
    }
}

impl JwtAuth {
    fn default_allowed_algorithms() -> Vec<JwtAlgorithm> {
        vec![JwtAlgorithm::Hs256]
    }

    const fn default_jwks_refresh_interval_secs() -> u64 {
        300
    }

    const fn default_jwks_request_timeout_ms() -> u64 {
        2_000
    }

    const fn default_jwks_cache_ttl_secs() -> u64 {
        900
    }

    const fn default_jwks_stale_if_error_secs() -> u64 {
        3_600
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JwtAlgorithm {
    #[serde(rename = "HS256", alias = "hs256")]
    Hs256,
    #[serde(rename = "RS256", alias = "rs256")]
    Rs256,
    #[serde(rename = "ES256", alias = "es256")]
    Es256,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JwksStartupBehavior {
    #[default]
    RequireReady,
    AllowDegraded,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JwtVerificationKey {
    Pem {
        #[serde(default)]
        kid: Option<String>,
        #[serde(default)]
        alg: Option<JwtAlgorithm>,
        public_key_pem: String,
    },
    Jwk {
        #[serde(default)]
        kid: Option<String>,
        #[serde(default)]
        alg: Option<JwtAlgorithm>,
        jwk: String,
    },
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

#[derive(Debug, Deserialize, Serialize, Clone)]
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

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct RouteMatch {
    pub host: Option<String>, // host-based routing (e.g., "api.example.com")

    pub path_prefix: Option<String>, // path prefix matching (e.g., "/api")

    pub method: Option<String>, // Optional HTTP method filtering (GET, POST, etc.)
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct LoadBalancing {
    #[serde(rename = "type")]
    pub lb_type: String, // "random","round_robin","consistent_hash","least_connections","latency_aware","sticky_cid"

    // Configurable key source for hash-based/sticky load balancing.
    pub key: Option<String>, // Examples: header:x-user-id, cookie:session_id, query:user_id
}

impl Default for LoadBalancing {
    fn default() -> Self {
        Self {
            lb_type: "round-robin".to_string(),
            key: None,
        }
    }
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
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Resilience {
    pub adaptive_admission: AdaptiveAdmission,
    pub route_queue: RouteQueue,
    pub scoped_rate_limits: Vec<ScopedRateLimit>,
    pub quota: QuotaPolicyConfig,
    pub protocol: ProtocolPolicy,
    pub circuit_breaker: CircuitBreaker,
    pub hedging: Hedging,
    pub retry_budget: RetryBudget,
    pub brownout: Brownout,
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
                    if !is_supported_request_key_spec(key_spec) {
                        return Err(format!(
                            "resilience.scoped_rate_limits['{}'].key must be a supported request key spec",
                            rule_name
                        ));
                    }
                }
                ScopedRateLimitScope::Client | ScopedRateLimitScope::Token => {
                    if let Some(key_spec) = rule.key.as_deref()
                        && !is_supported_request_key_spec(key_spec)
                    {
                        return Err(format!(
                            "resilience.scoped_rate_limits['{}'].key must be a supported request key spec",
                            rule_name
                        ));
                    }
                }
            }
        }
        self.quota.validate()?;
        if self.hedging.enabled && self.hedging.delay_ms == 0 {
            return Err("resilience.hedging: delay_ms must be > 0 when hedging is enabled".into());
        }
        Ok(())
    }
}

fn is_supported_request_key_spec(spec: &str) -> bool {
    matches!(
        spec.trim().to_ascii_lowercase().as_str(),
        "path"
            | "authority"
            | "method"
            | "cid"
            | "sticky-cid"
            | "peer_ip"
            | "client_ip"
            | "bearer_token"
    ) || spec.split_once(':').is_some_and(|(source, key_name)| {
        !key_name.trim().is_empty()
            && matches!(
                source.trim().to_ascii_lowercase().as_str(),
                "header" | "cookie" | "query"
            )
    })
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

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaEnforcementMode {
    Shadow,
    #[default]
    Enforce,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaBackendFailurePolicy {
    FailOpen,
    #[default]
    FailClosed,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct QuotaPolicyConfig {
    pub enabled: bool,
    pub enforcement: QuotaEnforcementMode,
    pub backend_failure_policy: QuotaBackendFailurePolicy,
    pub backend: QuotaCounterBackend,
    pub local_fallback: Option<QuotaLocalFallbackConfig>,
    pub policies: Vec<DistributedQuotaPolicy>,
}

impl QuotaPolicyConfig {
    fn validate(&self) -> Result<(), String> {
        for policy in &self.policies {
            policy.validate()?;
        }

        match &self.backend {
            QuotaCounterBackend::InMemory { key_prefix } => {
                if key_prefix.trim().is_empty() {
                    return Err(
                        "resilience.quota.backend.key_prefix must be non-empty for kind=in_memory"
                            .into(),
                    );
                }
                if self.local_fallback.is_some() {
                    return Err(
                        "resilience.quota.local_fallback is only supported when backend.kind=redis"
                            .into(),
                    );
                }
            }
            QuotaCounterBackend::Redis {
                url,
                key_prefix,
                connect_timeout_ms,
                command_timeout_ms,
                max_inflight,
            } => {
                if url.trim().is_empty() {
                    return Err(
                        "resilience.quota.backend.url must be non-empty for kind=redis".into(),
                    );
                }
                if key_prefix.trim().is_empty() {
                    return Err(
                        "resilience.quota.backend.key_prefix must be non-empty for kind=redis"
                            .into(),
                    );
                }
                if *connect_timeout_ms == 0 {
                    return Err(
                        "resilience.quota.backend.connect_timeout_ms must be > 0 for kind=redis"
                            .into(),
                    );
                }
                if *command_timeout_ms == 0 {
                    return Err(
                        "resilience.quota.backend.command_timeout_ms must be > 0 for kind=redis"
                            .into(),
                    );
                }
                if *max_inflight == 0 {
                    return Err(
                        "resilience.quota.backend.max_inflight must be > 0 for kind=redis".into(),
                    );
                }
            }
        }

        if let Some(local_fallback) = &self.local_fallback {
            local_fallback.validate()?;
        }

        if self.enabled && self.policies.is_empty() {
            return Err("resilience.quota.policies must not be empty when quota is enabled".into());
        }

        Ok(())
    }

    fn default_key_prefix() -> String {
        "spooky:quota".to_string()
    }

    fn default_connect_timeout_ms() -> u64 {
        250
    }

    fn default_command_timeout_ms() -> u64 {
        100
    }

    fn default_max_inflight() -> usize {
        1024
    }

    fn default_local_fallback_key_prefix() -> String {
        "spooky:quota:fallback".to_string()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct QuotaLocalFallbackConfig {
    #[serde(default = "QuotaPolicyConfig::default_local_fallback_key_prefix")]
    pub key_prefix: String,
    pub max_entries: usize,
}

impl QuotaLocalFallbackConfig {
    fn validate(&self) -> Result<(), String> {
        if self.key_prefix.trim().is_empty() {
            return Err("resilience.quota.local_fallback.key_prefix must be non-empty".to_string());
        }
        if self.max_entries == 0 {
            return Err("resilience.quota.local_fallback.max_entries must be > 0".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaCounterBackend {
    InMemory {
        #[serde(default = "QuotaPolicyConfig::default_key_prefix")]
        key_prefix: String,
    },
    Redis {
        url: String,
        #[serde(default = "QuotaPolicyConfig::default_key_prefix")]
        key_prefix: String,
        #[serde(default = "QuotaPolicyConfig::default_connect_timeout_ms")]
        connect_timeout_ms: u64,
        #[serde(default = "QuotaPolicyConfig::default_command_timeout_ms")]
        command_timeout_ms: u64,
        #[serde(default = "QuotaPolicyConfig::default_max_inflight")]
        max_inflight: usize,
    },
}

impl Default for QuotaCounterBackend {
    fn default() -> Self {
        Self::InMemory {
            key_prefix: QuotaPolicyConfig::default_key_prefix(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DistributedQuotaPolicy {
    pub name: String,
    #[serde(default)]
    pub route_allowlist: Vec<String>,
    #[serde(default)]
    pub selector: DistributedQuotaSelector,
    #[serde(default)]
    pub burst: Option<DistributedQuotaWindow>,
    #[serde(default)]
    pub sustained: Option<DistributedQuotaWindow>,
}

impl DistributedQuotaPolicy {
    fn validate(&self) -> Result<(), String> {
        let policy_name = self.name.trim();
        if policy_name.is_empty() {
            return Err("resilience.quota.policies[].name must be non-empty".into());
        }
        if self
            .route_allowlist
            .iter()
            .any(|route| route.trim().is_empty())
        {
            return Err(format!(
                "resilience.quota.policies['{}'].route_allowlist must not contain empty values",
                policy_name
            ));
        }
        if !self.selector.has_dimension() {
            return Err(format!(
                "resilience.quota.policies['{}'].selector must include at least one dimension",
                policy_name
            ));
        }
        self.selector.validate(policy_name)?;

        if self.burst.is_none() && self.sustained.is_none() {
            return Err(format!(
                "resilience.quota.policies['{}'] must define at least one of burst or sustained",
                policy_name
            ));
        }

        if let Some(window) = &self.burst {
            window.validate(&format!(
                "resilience.quota.policies['{}'].burst",
                policy_name
            ))?;
        }
        if let Some(window) = &self.sustained {
            window.validate(&format!(
                "resilience.quota.policies['{}'].sustained",
                policy_name
            ))?;
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct DistributedQuotaSelector {
    pub route: bool,
    pub tenant: Option<DistributedQuotaSelectorSource>,
    pub token: Option<DistributedQuotaSelectorSource>,
    pub client: Option<DistributedQuotaSelectorSource>,
}

impl DistributedQuotaSelector {
    fn has_dimension(&self) -> bool {
        self.route || self.tenant.is_some() || self.token.is_some() || self.client.is_some()
    }

    fn validate(&self, policy_name: &str) -> Result<(), String> {
        if let Some(source) = &self.tenant {
            source.validate(&format!(
                "resilience.quota.policies['{}'].selector.tenant",
                policy_name
            ))?;
        }
        if let Some(source) = &self.token {
            source.validate(&format!(
                "resilience.quota.policies['{}'].selector.token",
                policy_name
            ))?;
        }
        if let Some(source) = &self.client {
            source.validate(&format!(
                "resilience.quota.policies['{}'].selector.client",
                policy_name
            ))?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DistributedQuotaSelectorSource {
    pub key: String,
}

impl DistributedQuotaSelectorSource {
    fn validate(&self, field_path: &str) -> Result<(), String> {
        if self.key.trim().is_empty() {
            return Err(format!("{field_path}.key must be non-empty"));
        }
        if !is_supported_request_key_spec(&self.key) {
            return Err(format!(
                "{field_path}.key must be a supported request key spec"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DistributedQuotaWindow {
    pub requests: u64,
    pub window_secs: u64,
}

impl DistributedQuotaWindow {
    fn validate(&self, field_path: &str) -> Result<(), String> {
        if self.requests == 0 {
            return Err(format!("{field_path}.requests must be > 0"));
        }
        if self.window_secs == 0 {
            return Err(format!("{field_path}.window_secs must be > 0"));
        }
        Ok(())
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
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Observability {
    pub metrics: MetricsEndpoint,
    pub control_api: ControlApi,
    pub tracing: Tracing,
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

#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ControlApiClientAuthMode {
    #[default]
    Disabled,
    Optional,
    Required,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum ControlApiRole {
    Viewer,
    Operator,
    #[default]
    Admin,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ControlApiAuditFormat {
    #[default]
    Json,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ControlApiAuditSink {
    #[default]
    Log,
    File,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, Eq, PartialEq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ControlApiTls {
    pub client_auth: ControlApiTlsClientAuth,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, Eq, PartialEq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ControlApiTlsClientAuth {
    pub mode: ControlApiClientAuthMode,
    pub ca_file: Option<String>,
    pub ca_dir: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Default, Eq, PartialEq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ControlApiAuth {
    pub bearer_tokens: Vec<ControlApiBearerToken>,
    pub identity_source: Option<ControlApiIdentitySource>,
}

impl std::fmt::Debug for ControlApiAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlApiAuth")
            .field(
                "bearer_tokens",
                &format_args!("{} configured", self.bearer_tokens.len()),
            )
            .field("identity_source", &self.identity_source)
            .finish()
    }
}

#[derive(Deserialize, Serialize, Clone, Eq, PartialEq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ControlApiBearerToken {
    #[serde(skip_serializing)]
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_ref: Option<SecretRef>,
    pub role: ControlApiRole,
    pub actor_id: Option<String>,
}

impl Default for ControlApiBearerToken {
    fn default() -> Self {
        Self {
            token: String::new(),
            token_ref: None,
            role: ControlApiRole::Admin,
            actor_id: None,
        }
    }
}

impl std::fmt::Debug for ControlApiBearerToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlApiBearerToken")
            .field("token", &"<redacted>")
            .field("token_ref", &self.token_ref.as_ref().map(|_| "<configured>"))
            .field("role", &self.role)
            .field("actor_id", &self.actor_id)
            .finish()
        }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, Eq, PartialEq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ControlApiIdentitySource {
    pub kind: String,
    pub role_attribute: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Eq, PartialEq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ControlApiAuthorization {
    pub protect_health: bool,
    pub protect_ready: bool,
    pub runtime_read_role: ControlApiRole,
    pub runtime_mutate_role: ControlApiRole,
    pub restart_role: ControlApiRole,
}

impl Default for ControlApiAuthorization {
    fn default() -> Self {
        Self {
            protect_health: false,
            protect_ready: false,
            runtime_read_role: ControlApiRole::Viewer,
            runtime_mutate_role: ControlApiRole::Operator,
            restart_role: ControlApiRole::Admin,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, Eq, PartialEq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ControlApiIpAllowlist {
    pub cidrs: Vec<String>,
    pub trust_proxy_headers: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, Eq, PartialEq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ControlApiAudit {
    pub enabled: bool,
    pub format: ControlApiAuditFormat,
    pub sink: ControlApiAuditSink,
    pub file_path: Option<String>,
}

impl Default for ControlApiAudit {
    fn default() -> Self {
        Self {
            enabled: false,
            format: ControlApiAuditFormat::Json,
            sink: ControlApiAuditSink::Log,
            file_path: None,
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

    // Admin credential: plaintext auth_token is never emitted by Serialize
    // (e.g. the /admin/runtime dump) and redacted in Debug; still accepted on
    // deserialize. auth_token_ref carries only a reference, not secret
    // material, so it is safe to serialize (and must be, so it survives a
    // config write-back/reload round trip).
    #[serde(skip_serializing)]
    pub auth_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token_ref: Option<SecretRef>,

    pub tls: ControlApiTls,

    pub auth: ControlApiAuth,

    pub authorization: ControlApiAuthorization,

    pub ip_allowlist: ControlApiIpAllowlist,

    pub audit: ControlApiAudit,

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
            auth_token_ref: None,
            tls: ControlApiTls::default(),
            auth: ControlApiAuth::default(),
            authorization: ControlApiAuthorization::default(),
            ip_allowlist: ControlApiIpAllowlist::default(),
            audit: ControlApiAudit::default(),
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
            .field(
                "auth_token_ref",
                &self.auth_token_ref.as_ref().map(|_| "<configured>"),
            )
            .field("tls", &self.tls)
            .field("auth", &self.auth)
            .field("authorization", &self.authorization)
            .field("ip_allowlist", &self.ip_allowlist)
            .field("audit", &self.audit)
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
        ApiKeyAuth, Config, ControlApi, ControlApiAudit, ControlApiAuditFormat,
        ControlApiAuditSink, ControlApiAuth, ControlApiAuthorization, ControlApiClientAuthMode,
        ControlApiIpAllowlist, ControlApiRole, ControlApiTls, DistributedQuotaPolicy,
        DistributedQuotaSelector, DistributedQuotaSelectorSource, DistributedQuotaWindow,
        ExternalAuth, ForwardedHeaderPolicy, JwtAuth, Listen, LoadBalancing, Log, MetricsEndpoint,
        Performance, PrivilegeDrop, QuotaBackendFailurePolicy, QuotaCounterBackend,
        QuotaEnforcementMode, QuotaLocalFallbackConfig, QuotaPolicyConfig, Resilience, RouteAuth,
        RoutingTransparency, SecretProvider, SecretRef, Secrets, Tracing, UpstreamHostPolicy,
        UpstreamTls, Watchdog,
    };
    use crate::config::CURRENT_CONFIG_VERSION;

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

        assert_eq!(
            config.security.privileges.enabled,
            PrivilegeDrop::default().enabled
        );
        assert_eq!(
            config.security.privileges.user,
            PrivilegeDrop::default().user
        );
        assert_eq!(
            config.security.privileges.group,
            PrivilegeDrop::default().group
        );
    }

    #[test]
    fn serde_defaults_for_leaf_structs_match_type_defaults() {
        let listen: Listen =
            serde_yaml::from_str("{}").expect("empty listen should parse via type defaults");
        assert_eq!(listen.protocol, Listen::default().protocol);
        assert_eq!(listen.port, Listen::default().port);
        assert_eq!(listen.address, Listen::default().address);

        let log: Log =
            serde_yaml::from_str("{}").expect("empty log should parse via type defaults");
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
        assert_eq!(jwt.issuers, JwtAuth::default().issuers);
        assert_eq!(jwt.audiences, JwtAuth::default().audiences);
        assert_eq!(
            jwt.allowed_algorithms,
            JwtAuth::default().allowed_algorithms
        );
        assert_eq!(jwt.require_kid, JwtAuth::default().require_kid);
        assert_eq!(jwt.static_keys.len(), JwtAuth::default().static_keys.len());
        assert_eq!(jwt.jwks_url, JwtAuth::default().jwks_url);
        assert_eq!(
            jwt.jwks_refresh_interval_secs,
            JwtAuth::default().jwks_refresh_interval_secs
        );
        assert_eq!(
            jwt.jwks_request_timeout_ms,
            JwtAuth::default().jwks_request_timeout_ms
        );
        assert_eq!(
            jwt.jwks_cache_ttl_secs,
            JwtAuth::default().jwks_cache_ttl_secs
        );
        assert_eq!(
            jwt.jwks_stale_if_error_secs,
            JwtAuth::default().jwks_stale_if_error_secs
        );
        assert_eq!(
            jwt.jwks_startup_behavior,
            JwtAuth::default().jwks_startup_behavior
        );
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
            control_api.tls.client_auth.mode,
            ControlApiClientAuthMode::Disabled
        );
        assert!(control_api.tls.client_auth.ca_file.is_none());
        assert!(control_api.tls.client_auth.ca_dir.is_none());
        assert!(control_api.auth.bearer_tokens.is_empty());
        assert!(control_api.auth.identity_source.is_none());
        assert_eq!(
            control_api.authorization.runtime_read_role,
            ControlApiRole::Viewer
        );
        assert_eq!(
            control_api.authorization.runtime_mutate_role,
            ControlApiRole::Operator
        );
        assert_eq!(
            control_api.authorization.restart_role,
            ControlApiRole::Admin
        );
        assert!(control_api.ip_allowlist.cidrs.is_empty());
        assert!(!control_api.ip_allowlist.trust_proxy_headers);
        assert!(!control_api.audit.enabled);
        assert_eq!(control_api.audit.format, ControlApiAuditFormat::Json);
        assert_eq!(control_api.audit.sink, ControlApiAuditSink::Log);
        assert!(control_api.audit.file_path.is_none());
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
        assert_eq!(
            routing.header_name,
            RoutingTransparency::default().header_name
        );
    }

    #[test]
    fn serde_defaults_for_performance_match_type_defaults() {
        let performance: Performance =
            serde_yaml::from_str("{}").expect("empty performance config should parse");

        assert_eq!(
            performance.worker_threads,
            Performance::default().worker_threads
        );
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
        assert_eq!(
            resilience.quota.enabled,
            Resilience::default().quota.enabled
        );
        assert_eq!(
            resilience.quota.enforcement,
            Resilience::default().quota.enforcement
        );
        assert_eq!(
            resilience.quota.backend_failure_policy,
            Resilience::default().quota.backend_failure_policy
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

    #[test]
    fn remaining_type_owned_defaults_match_documented_contract() {
        assert_eq!(Config::default_version(), CURRENT_CONFIG_VERSION);

        let lb: LoadBalancing =
            serde_yaml::from_str("{}").expect("empty lb config should parse via type default");
        assert_eq!(lb.lb_type, LoadBalancing::default().lb_type);
        assert_eq!(lb.key, LoadBalancing::default().key);

        let upstream_tls: UpstreamTls =
            serde_yaml::from_str("{}").expect("empty upstream tls config should parse");
        assert!(upstream_tls.verify_certificates);
        assert!(upstream_tls.strict_sni);
        assert_eq!(upstream_tls.ca_file, None);
        assert_eq!(upstream_tls.ca_dir, None);

        assert_eq!(ExternalAuth::default_timeout_ms(), 1_000);
    }

    #[test]
    fn serde_defaults_for_secret_types_match_type_defaults() {
        let secrets: Secrets =
            serde_yaml::from_str("{}").expect("empty secrets config should parse");
        assert_eq!(secrets, Secrets::default());

        let secret_ref: SecretRef =
            serde_yaml::from_str("ref: literal:test-secret").expect("secret ref should parse");
        assert_eq!(secret_ref.reference, "literal:test-secret");
        assert_eq!(secret_ref.scheme(), Some("literal"));

        let file_provider: SecretProvider =
            serde_yaml::from_str("kind: file\nbase_dir: /etc/spooky/secrets\n")
                .expect("file secret provider should parse");
        assert_eq!(
            file_provider,
            SecretProvider::File {
                base_dir: Some("/etc/spooky/secrets".to_string())
            }
        );
    }

    #[test]
    fn partial_secret_reference_inputs_parse_into_compatibility_fields() {
        let jwt: JwtAuth = serde_yaml::from_str(
            r#"
secret_ref:
  ref: file:///etc/spooky/secrets/jwt-signing.key
"#,
        )
        .expect("jwt secret ref should parse");
        assert!(jwt.secret.is_empty());
        assert_eq!(
            jwt.secret_ref
                .as_ref()
                .map(|secret_ref| secret_ref.reference.as_str()),
            Some("file:///etc/spooky/secrets/jwt-signing.key")
        );

        let control_api: ControlApi = serde_yaml::from_str(
            r#"
auth_token_ref:
  ref: literal:admin-token
auth:
  bearer_tokens:
    - token_ref:
        ref: file:///etc/spooky/secrets/viewer-token
      role: viewer
"#,
        )
        .expect("control api secret refs should parse");
        assert!(control_api.auth_token.is_none());
        assert_eq!(
            control_api
                .auth_token_ref
                .as_ref()
                .map(|secret_ref| secret_ref.reference.as_str()),
            Some("literal:admin-token")
        );
        assert_eq!(control_api.auth.bearer_tokens.len(), 1);
        assert!(control_api.auth.bearer_tokens[0].token.is_empty());
        assert_eq!(
            control_api.auth.bearer_tokens[0]
                .token_ref
                .as_ref()
                .map(|secret_ref| secret_ref.reference.as_str()),
            Some("file:///etc/spooky/secrets/viewer-token")
        );
    }

    #[test]
    fn quota_type_defaults_match_documented_contract() {
        let quota: QuotaPolicyConfig =
            serde_yaml::from_str("{}").expect("empty quota config should parse");

        assert!(!quota.enabled);
        assert_eq!(quota.enforcement, QuotaEnforcementMode::Enforce);
        assert_eq!(
            quota.backend_failure_policy,
            QuotaBackendFailurePolicy::FailClosed
        );
        assert!(quota.local_fallback.is_none());
        assert!(quota.policies.is_empty());
        match quota.backend {
            QuotaCounterBackend::InMemory { key_prefix } => {
                assert_eq!(key_prefix, "spooky:quota");
            }
            QuotaCounterBackend::Redis { .. } => {
                panic!("default quota backend must be in_memory");
            }
        }
    }

    #[test]
    fn resilience_validate_accepts_well_formed_quota_policy() {
        let resilience = Resilience {
            quota: QuotaPolicyConfig {
                enabled: true,
                enforcement: QuotaEnforcementMode::Shadow,
                backend_failure_policy: QuotaBackendFailurePolicy::FailOpen,
                backend: QuotaCounterBackend::Redis {
                    url: "redis://127.0.0.1:6379/0".to_string(),
                    key_prefix: "spooky:quota".to_string(),
                    connect_timeout_ms: 250,
                    command_timeout_ms: 100,
                    max_inflight: 128,
                },
                local_fallback: Some(QuotaLocalFallbackConfig {
                    key_prefix: "spooky:quota:fallback".to_string(),
                    max_entries: 512,
                }),
                policies: vec![DistributedQuotaPolicy {
                    name: "tenant-burst".to_string(),
                    route_allowlist: vec!["api".to_string()],
                    selector: DistributedQuotaSelector {
                        route: true,
                        tenant: Some(DistributedQuotaSelectorSource {
                            key: "header:x-tenant-id".to_string(),
                        }),
                        token: None,
                        client: None,
                    },
                    burst: Some(DistributedQuotaWindow {
                        requests: 100,
                        window_secs: 1,
                    }),
                    sustained: Some(DistributedQuotaWindow {
                        requests: 5000,
                        window_secs: 60,
                    }),
                }],
            },
            ..Resilience::default()
        };

        resilience
            .validate()
            .expect("well-formed distributed quota config should validate");
    }

    #[test]
    fn resilience_validate_rejects_invalid_quota_local_fallback_settings() {
        let unsupported_backend = Resilience {
            quota: QuotaPolicyConfig {
                enabled: true,
                local_fallback: Some(QuotaLocalFallbackConfig {
                    key_prefix: "spooky:quota:fallback".to_string(),
                    max_entries: 128,
                }),
                policies: vec![DistributedQuotaPolicy {
                    name: "tenant-burst".to_string(),
                    route_allowlist: vec!["api".to_string()],
                    selector: DistributedQuotaSelector {
                        route: true,
                        tenant: Some(DistributedQuotaSelectorSource {
                            key: "header:x-tenant-id".to_string(),
                        }),
                        token: None,
                        client: None,
                    },
                    burst: Some(DistributedQuotaWindow {
                        requests: 100,
                        window_secs: 1,
                    }),
                    sustained: None,
                }],
                ..QuotaPolicyConfig::default()
            },
            ..Resilience::default()
        };
        assert_eq!(
            unsupported_backend
                .validate()
                .expect_err("in-memory quota backend must reject local fallback"),
            "resilience.quota.local_fallback is only supported when backend.kind=redis"
        );

        let invalid_capacity = Resilience {
            quota: QuotaPolicyConfig {
                enabled: true,
                backend: QuotaCounterBackend::Redis {
                    url: "redis://127.0.0.1:6379/0".to_string(),
                    key_prefix: "spooky:quota".to_string(),
                    connect_timeout_ms: 250,
                    command_timeout_ms: 100,
                    max_inflight: 128,
                },
                local_fallback: Some(QuotaLocalFallbackConfig {
                    key_prefix: "spooky:quota:fallback".to_string(),
                    max_entries: 0,
                }),
                policies: vec![DistributedQuotaPolicy {
                    name: "tenant-burst".to_string(),
                    route_allowlist: vec!["api".to_string()],
                    selector: DistributedQuotaSelector {
                        route: true,
                        tenant: Some(DistributedQuotaSelectorSource {
                            key: "header:x-tenant-id".to_string(),
                        }),
                        token: None,
                        client: None,
                    },
                    burst: Some(DistributedQuotaWindow {
                        requests: 100,
                        window_secs: 1,
                    }),
                    sustained: None,
                }],
                ..QuotaPolicyConfig::default()
            },
            ..Resilience::default()
        };
        assert_eq!(
            invalid_capacity
                .validate()
                .expect_err("zero local fallback capacity must be rejected"),
            "resilience.quota.local_fallback.max_entries must be > 0"
        );
    }

    #[test]
    fn resilience_validate_rejects_quota_policy_without_selector_dimensions() {
        let resilience = Resilience {
            quota: QuotaPolicyConfig {
                enabled: true,
                policies: vec![DistributedQuotaPolicy {
                    name: "missing-selector".to_string(),
                    route_allowlist: Vec::new(),
                    selector: DistributedQuotaSelector::default(),
                    burst: Some(DistributedQuotaWindow {
                        requests: 10,
                        window_secs: 1,
                    }),
                    sustained: None,
                }],
                ..QuotaPolicyConfig::default()
            },
            ..Resilience::default()
        };

        let err = resilience
            .validate()
            .expect_err("selector-less quota policy must be rejected");
        assert!(err.contains("selector must include at least one dimension"));
    }

    #[test]
    fn partial_struct_inputs_still_fill_missing_fields_from_type_defaults() {
        let auth: ApiKeyAuth = serde_yaml::from_str(
            r#"
header_name: x-custom-key
"#,
        )
        .expect("partial api key auth should parse");
        assert_eq!(auth.header_name, "x-custom-key");
        assert!(auth.keys.is_empty());

        let control_api: ControlApi = serde_yaml::from_str(
            r#"
enabled: true
"#,
        )
        .expect("partial control api should parse");
        assert!(control_api.enabled);
        assert_eq!(control_api.port, ControlApi::default().port);
        assert_eq!(control_api.auth_token, None);
        assert_eq!(
            control_api.authorization,
            ControlApiAuthorization::default()
        );
        assert_eq!(control_api.tls, ControlApiTls::default());
        assert_eq!(control_api.auth, ControlApiAuth::default());
        assert_eq!(control_api.ip_allowlist, ControlApiIpAllowlist::default());
        assert_eq!(control_api.audit, ControlApiAudit::default());

        let route_auth: RouteAuth = serde_yaml::from_str(
            r#"
required_scopes:
  - payments.read
"#,
        )
        .expect("partial route auth should parse");
        assert_eq!(route_auth.required_scopes, vec!["payments.read"]);
        assert!(route_auth.required_roles.is_empty());
        assert!(route_auth.api_key.is_none());
        assert!(route_auth.jwt.is_none());
        assert!(route_auth.external_auth.is_none());
    }
}
