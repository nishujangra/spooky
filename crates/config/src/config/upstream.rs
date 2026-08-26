use super::*;

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
    pub route: RouteMatch,
    pub backends: Vec<Backend>,
}

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
    pub(crate) fn default_timeout_ms() -> u64 {
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
    pub id: String,
    pub address: String,
    #[serde(default = "Backend::default_weight")]
    pub weight: u32,
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
    pub host: Option<String>,
    pub path_prefix: Option<String>,
    pub method: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    pub path: String,
    pub interval: u64,
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
    pub lb_type: String,
    pub key: Option<String>,
}

impl Default for LoadBalancing {
    fn default() -> Self {
        Self {
            lb_type: "round-robin".to_string(),
            key: None,
        }
    }
}
