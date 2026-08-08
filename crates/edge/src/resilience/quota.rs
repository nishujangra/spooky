use std::{collections::HashSet, future::Future, net::SocketAddr, pin::Pin, time::Duration};

use sha2::{Digest, Sha256};
use spooky_config::{
    config::{
        DistributedQuotaPolicy as RawDistributedQuotaPolicy,
        DistributedQuotaSelector as RawDistributedQuotaSelector,
        DistributedQuotaWindow as RawDistributedQuotaWindow,
        QuotaBackendFailurePolicy as RawQuotaBackendFailurePolicy,
        QuotaCounterBackend as RawQuotaCounterBackend,
        QuotaEnforcementMode as RawQuotaEnforcementMode, QuotaPolicyConfig as RawQuotaPolicyConfig,
        Resilience as ResilienceConfig,
    },
    runtime::{
        RuntimeQuotaBackendFailurePolicy as ConfigRuntimeQuotaBackendFailurePolicy,
        RuntimeQuotaCounterBackend as ConfigRuntimeQuotaCounterBackend,
        RuntimeQuotaEnforcementMode as ConfigRuntimeQuotaEnforcementMode,
        RuntimeQuotaPolicy as ConfigRuntimeQuotaPolicy,
        RuntimeQuotaPolicySet as ConfigRuntimeQuotaPolicySet,
        RuntimeQuotaSelectorMatcher as ConfigRuntimeQuotaSelectorMatcher,
        RuntimeQuotaWindow as ConfigRuntimeQuotaWindow,
        RuntimeRequestKeySpec,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaEnforcementMode {
    Shadow,
    Enforce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaBackendFailurePolicy {
    FailOpen,
    FailClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaSelectorKeySpec {
    Path,
    Authority,
    Method,
    Cid,
    StickyCid,
    PeerIp,
    ClientIp,
    BearerToken,
    Header(String),
    Cookie(String),
    Query(String),
}

pub(crate) type QuotaHeaderLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

#[derive(Clone, Copy)]
pub(crate) struct QuotaIdentityContext<'a> {
    pub route: Option<&'a str>,
    pub method: &'a str,
    pub path: &'a str,
    pub authority: Option<&'a str>,
    pub cid_key: Option<&'a str>,
    pub client_addr: Option<SocketAddr>,
    pub header_lookup: Option<&'a QuotaHeaderLookup<'a>>,
}

impl<'a> QuotaIdentityContext<'a> {
    pub(crate) fn new(
        route: Option<&'a str>,
        method: &'a str,
        path: &'a str,
        authority: Option<&'a str>,
        cid_key: Option<&'a str>,
        client_addr: Option<SocketAddr>,
        header_lookup: Option<&'a QuotaHeaderLookup<'a>>,
    ) -> Self {
        Self {
            route,
            method,
            path,
            authority,
            cid_key,
            client_addr,
            header_lookup,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaIdentityDimension {
    Route,
    Tenant,
    Token,
    Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaIdentityLabels {
    pub route: Option<String>,
    pub tenant: Option<String>,
    pub token: Option<String>,
    pub client: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaCompositeKey {
    pub policy_name: String,
    pub key: String,
    pub labels: QuotaIdentityLabels,
    pub dimensions: QuotaSelectorDimensions,
}

pub type QuotaCounterEvalFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<QuotaCounterEvaluationOutcome, QuotaCounterBackendError>>
            + Send
            + 'a,
    >,
>;

/// Distributed quota backends must evaluate all configured windows in one atomic operation.
/// Implementations should not split this into independent reads and writes, which would admit
/// request races between burst and sustained counters under concurrent load.
pub trait DistributedQuotaCounterBackend: Send + Sync {
    fn evaluate<'a>(&'a self, request: QuotaCounterEvaluationRequest) -> QuotaCounterEvalFuture<'a>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaIdentityRejection {
    pub policy_name: String,
    pub dimension: QuotaIdentityDimension,
    pub reason: QuotaDenyReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequestKeyExtraction {
    Found(String),
    Missing,
    Invalid,
}

impl From<&RuntimeRequestKeySpec> for QuotaSelectorKeySpec {
    fn from(value: &RuntimeRequestKeySpec) -> Self {
        match value {
            RuntimeRequestKeySpec::Path => Self::Path,
            RuntimeRequestKeySpec::Authority => Self::Authority,
            RuntimeRequestKeySpec::Method => Self::Method,
            RuntimeRequestKeySpec::Cid => Self::Cid,
            RuntimeRequestKeySpec::StickyCid => Self::StickyCid,
            RuntimeRequestKeySpec::PeerIp => Self::PeerIp,
            RuntimeRequestKeySpec::ClientIp => Self::ClientIp,
            RuntimeRequestKeySpec::BearerToken => Self::BearerToken,
            RuntimeRequestKeySpec::Header(name) => Self::Header(name.clone()),
            RuntimeRequestKeySpec::Cookie(name) => Self::Cookie(name.clone()),
            RuntimeRequestKeySpec::Query(name) => Self::Query(name.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaSelectorMatcher {
    pub route: bool,
    pub tenant: Option<QuotaSelectorKeySpec>,
    pub token: Option<QuotaSelectorKeySpec>,
    pub client: Option<QuotaSelectorKeySpec>,
}

impl QuotaSelectorMatcher {
    pub fn dimensions(&self) -> QuotaSelectorDimensions {
        QuotaSelectorDimensions {
            route: self.route,
            tenant: self.tenant.is_some(),
            token: self.token.is_some(),
            client: self.client.is_some(),
        }
    }

    fn from_runtime(value: &ConfigRuntimeQuotaSelectorMatcher) -> Self {
        Self {
            route: value.route,
            tenant: value.tenant.as_ref().map(QuotaSelectorKeySpec::from),
            token: value.token.as_ref().map(QuotaSelectorKeySpec::from),
            client: value.client.as_ref().map(QuotaSelectorKeySpec::from),
        }
    }

    pub fn extract_identities(
        &self,
        policy_name: &str,
        context: &QuotaIdentityContext<'_>,
    ) -> Result<QuotaIdentityLabels, QuotaIdentityRejection> {
        let route = if self.route {
            Some(
                normalize_route_identity(context.route).ok_or_else(|| QuotaIdentityRejection {
                    policy_name: policy_name.to_string(),
                    dimension: QuotaIdentityDimension::Route,
                    reason: QuotaDenyReason::SelectorIdentityMissing,
                })?,
            )
        } else {
            None
        };

        let tenant = self.extract_dimension_identity(
            policy_name,
            QuotaIdentityDimension::Tenant,
            self.tenant.as_ref(),
            context,
        )?;
        let token = self.extract_dimension_identity(
            policy_name,
            QuotaIdentityDimension::Token,
            self.token.as_ref(),
            context,
        )?;
        let client = self.extract_dimension_identity(
            policy_name,
            QuotaIdentityDimension::Client,
            self.client.as_ref(),
            context,
        )?;

        Ok(QuotaIdentityLabels {
            route,
            tenant,
            token,
            client,
        })
    }

    fn extract_dimension_identity(
        &self,
        policy_name: &str,
        dimension: QuotaIdentityDimension,
        spec: Option<&QuotaSelectorKeySpec>,
        context: &QuotaIdentityContext<'_>,
    ) -> Result<Option<String>, QuotaIdentityRejection> {
        let Some(spec) = spec else {
            return Ok(None);
        };

        let extracted = extract_quota_selector_key(spec, context);
        let sensitive = quota_dimension_is_sensitive(dimension, spec);

        match extracted {
            RequestKeyExtraction::Found(value) => Ok(Some(stable_observable_identity_value(
                &value,
                sensitive,
            ))),
            RequestKeyExtraction::Missing => Err(QuotaIdentityRejection {
                policy_name: policy_name.to_string(),
                dimension,
                reason: QuotaDenyReason::SelectorIdentityMissing,
            }),
            RequestKeyExtraction::Invalid => Err(QuotaIdentityRejection {
                policy_name: policy_name.to_string(),
                dimension,
                reason: QuotaDenyReason::SelectorIdentityInvalid,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaSelectorDimensions {
    pub route: bool,
    pub tenant: bool,
    pub token: bool,
    pub client: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaWindowPolicy {
    pub requests: u64,
    pub window: Duration,
}

impl QuotaWindowPolicy {
    fn from_runtime(value: &ConfigRuntimeQuotaWindow) -> Self {
        Self {
            requests: value.requests,
            window: value.window,
        }
    }

    fn from_raw(value: &RawDistributedQuotaWindow) -> Self {
        Self {
            requests: value.requests.max(1),
            window: Duration::from_secs(value.window_secs.max(1)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaPolicyRuntime {
    pub name: String,
    pub route_allowlist: HashSet<String>,
    pub selector: QuotaSelectorMatcher,
    pub burst: Option<QuotaWindowPolicy>,
    pub sustained: Option<QuotaWindowPolicy>,
}

impl QuotaPolicyRuntime {
    fn from_runtime(value: &ConfigRuntimeQuotaPolicy) -> Self {
        Self {
            name: value.name.clone(),
            route_allowlist: value.route_allowlist.iter().cloned().collect(),
            selector: QuotaSelectorMatcher::from_runtime(&value.selector),
            burst: value.burst.as_ref().map(QuotaWindowPolicy::from_runtime),
            sustained: value.sustained.as_ref().map(QuotaWindowPolicy::from_runtime),
        }
    }

    fn from_raw(value: &RawDistributedQuotaPolicy) -> Self {
        Self {
            name: value.name.trim().to_string(),
            route_allowlist: value
                .route_allowlist
                .iter()
                .map(|route| route.trim())
                .filter(|route| !route.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            selector: QuotaSelectorMatcher::from_raw(&value.selector),
            burst: value.burst.as_ref().map(QuotaWindowPolicy::from_raw),
            sustained: value.sustained.as_ref().map(QuotaWindowPolicy::from_raw),
        }
    }

    pub fn counter_request(&self, composite_key: QuotaCompositeKey) -> QuotaCounterEvaluationRequest {
        QuotaCounterEvaluationRequest {
            policy_name: self.name.clone(),
            composite_key,
            cost: 1,
            burst: self.burst.clone(),
            sustained: self.sustained.clone(),
        }
    }
}

impl QuotaSelectorMatcher {
    fn from_raw(value: &RawDistributedQuotaSelector) -> Self {
        Self {
            route: value.route,
            tenant: value
                .tenant
                .as_ref()
                .map(|source| QuotaSelectorKeySpec::from_raw_key(&source.key)),
            token: value
                .token
                .as_ref()
                .map(|source| QuotaSelectorKeySpec::from_raw_key(&source.key)),
            client: value
                .client
                .as_ref()
                .map(|source| QuotaSelectorKeySpec::from_raw_key(&source.key)),
        }
    }
}

impl QuotaSelectorKeySpec {
    fn from_raw_key(value: &str) -> Self {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "path" => Self::Path,
            "authority" => Self::Authority,
            "method" => Self::Method,
            "cid" => Self::Cid,
            "sticky-cid" => Self::StickyCid,
            "peer_ip" => Self::PeerIp,
            "client_ip" => Self::ClientIp,
            "bearer_token" => Self::BearerToken,
            _ => {
                if let Some((source, key)) = normalized.split_once(':') {
                    return match source {
                        "header" => Self::Header(key.trim().to_string()),
                        "cookie" => Self::Cookie(key.trim().to_string()),
                        "query" => Self::Query(key.trim().to_string()),
                        _ => Self::Header(normalized),
                    };
                }
                Self::Header(normalized)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaCounterBackend {
    InMemory {
        key_prefix: String,
    },
    Redis {
        url: String,
        key_prefix: String,
        connect_timeout: Duration,
        command_timeout: Duration,
        max_inflight: usize,
    },
}

impl QuotaCounterBackend {
    fn from_runtime(value: &ConfigRuntimeQuotaCounterBackend) -> Self {
        match value {
            ConfigRuntimeQuotaCounterBackend::InMemory { key_prefix } => Self::InMemory {
                key_prefix: key_prefix.clone(),
            },
            ConfigRuntimeQuotaCounterBackend::Redis {
                url,
                key_prefix,
                connect_timeout,
                command_timeout,
                max_inflight,
            } => Self::Redis {
                url: url.clone(),
                key_prefix: key_prefix.clone(),
                connect_timeout: *connect_timeout,
                command_timeout: *command_timeout,
                max_inflight: *max_inflight,
            },
        }
    }

    fn from_raw(value: &RawQuotaCounterBackend) -> Self {
        match value {
            RawQuotaCounterBackend::InMemory { key_prefix } => Self::InMemory {
                key_prefix: key_prefix.trim().to_string(),
            },
            RawQuotaCounterBackend::Redis {
                url,
                key_prefix,
                connect_timeout_ms,
                command_timeout_ms,
                max_inflight,
            } => Self::Redis {
                url: url.trim().to_string(),
                key_prefix: key_prefix.trim().to_string(),
                connect_timeout: Duration::from_millis(*connect_timeout_ms),
                command_timeout: Duration::from_millis(*command_timeout_ms),
                max_inflight: *max_inflight,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaRuntime {
    pub enabled: bool,
    pub enforcement: QuotaEnforcementMode,
    pub backend_failure_policy: QuotaBackendFailurePolicy,
    pub backend: QuotaCounterBackend,
    pub policies: Vec<QuotaPolicyRuntime>,
}

impl QuotaRuntime {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            enforcement: QuotaEnforcementMode::Enforce,
            backend_failure_policy: QuotaBackendFailurePolicy::FailClosed,
            backend: QuotaCounterBackend::InMemory {
                key_prefix: "spooky:quota".to_string(),
            },
            policies: Vec::new(),
        }
    }

    pub fn from_resilience_config(config: &ResilienceConfig) -> Self {
        Self::from_raw_config(&config.quota)
    }

    pub fn from_rate_limit_policies(rate_limit_policy: &spooky_config::runtime::RuntimeRateLimitPolicy) -> Self {
        Self::from_runtime_policy_set(&rate_limit_policy.quota)
    }

    pub fn from_runtime_policy_set(config: &ConfigRuntimeQuotaPolicySet) -> Self {
        Self {
            enabled: config.enabled,
            enforcement: match config.enforcement {
                ConfigRuntimeQuotaEnforcementMode::Shadow => QuotaEnforcementMode::Shadow,
                ConfigRuntimeQuotaEnforcementMode::Enforce => QuotaEnforcementMode::Enforce,
            },
            backend_failure_policy: match config.backend_failure_policy {
                ConfigRuntimeQuotaBackendFailurePolicy::FailOpen => {
                    QuotaBackendFailurePolicy::FailOpen
                }
                ConfigRuntimeQuotaBackendFailurePolicy::FailClosed => {
                    QuotaBackendFailurePolicy::FailClosed
                }
            },
            backend: QuotaCounterBackend::from_runtime(&config.backend),
            policies: config
                .policies
                .iter()
                .map(QuotaPolicyRuntime::from_runtime)
                .collect(),
        }
    }

    fn from_raw_config(config: &RawQuotaPolicyConfig) -> Self {
        Self {
            enabled: config.enabled,
            enforcement: match config.enforcement {
                RawQuotaEnforcementMode::Shadow => QuotaEnforcementMode::Shadow,
                RawQuotaEnforcementMode::Enforce => QuotaEnforcementMode::Enforce,
            },
            backend_failure_policy: match config.backend_failure_policy {
                RawQuotaBackendFailurePolicy::FailOpen => QuotaBackendFailurePolicy::FailOpen,
                RawQuotaBackendFailurePolicy::FailClosed => QuotaBackendFailurePolicy::FailClosed,
            },
            backend: QuotaCounterBackend::from_raw(&config.backend),
            policies: config
                .policies
                .iter()
                .map(QuotaPolicyRuntime::from_raw)
                .collect(),
        }
    }
}

impl QuotaPolicyRuntime {
    pub fn composite_key(
        &self,
        context: &QuotaIdentityContext<'_>,
    ) -> Result<QuotaCompositeKey, QuotaIdentityRejection> {
        let labels = self.selector.extract_identities(&self.name, context)?;
        Ok(QuotaCompositeKey {
            policy_name: self.name.clone(),
            key: compose_quota_key(&self.name, &labels),
            dimensions: self.selector.dimensions(),
            labels,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaDenyReason {
    BurstQuotaExhausted,
    SustainedQuotaExhausted,
    SelectorIdentityMissing,
    SelectorIdentityInvalid,
    BackendTimeout,
    BackendUnavailable,
    BackendError,
}

impl QuotaDenyReason {
    pub fn slug(self) -> &'static str {
        match self {
            Self::BurstQuotaExhausted => "burst_quota_exhausted",
            Self::SustainedQuotaExhausted => "sustained_quota_exhausted",
            Self::SelectorIdentityMissing => "selector_identity_missing",
            Self::SelectorIdentityInvalid => "selector_identity_invalid",
            Self::BackendTimeout => "backend_timeout",
            Self::BackendUnavailable => "backend_unavailable",
            Self::BackendError => "backend_error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaWindowUsage {
    pub limit: u64,
    pub consumed: u64,
    pub remaining: u64,
    pub window: Duration,
    pub reset_after: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaCounterResult {
    pub burst: Option<QuotaWindowUsage>,
    pub sustained: Option<QuotaWindowUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaCounterEvaluationRequest {
    pub policy_name: String,
    pub composite_key: QuotaCompositeKey,
    pub cost: u64,
    pub burst: Option<QuotaWindowPolicy>,
    pub sustained: Option<QuotaWindowPolicy>,
}

impl QuotaCounterEvaluationRequest {
    pub fn with_cost(mut self, cost: u64) -> Self {
        self.cost = cost.max(1);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaCounterEvaluationDecision {
    Allowed,
    Denied(QuotaDenyReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaCounterEvaluationOutcome {
    pub matched_policy: String,
    pub composite_key: QuotaCompositeKey,
    pub decision: QuotaCounterEvaluationDecision,
    pub counter: QuotaCounterResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaCounterBackendErrorKind {
    Timeout,
    Unavailable,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaCounterBackendError {
    pub policy_name: Option<String>,
    pub composite_key: Option<String>,
    pub kind: QuotaCounterBackendErrorKind,
    pub detail: Option<String>,
}

impl QuotaCounterBackendError {
    pub fn deny_reason(&self) -> QuotaDenyReason {
        match self.kind {
            QuotaCounterBackendErrorKind::Timeout => QuotaDenyReason::BackendTimeout,
            QuotaCounterBackendErrorKind::Unavailable => QuotaDenyReason::BackendUnavailable,
            QuotaCounterBackendErrorKind::Error => QuotaDenyReason::BackendError,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaAllowance {
    pub policy_name: String,
    pub counter: Option<QuotaCounterResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaDenial {
    pub policy_name: String,
    pub reason: QuotaDenyReason,
    pub retry_after_seconds: Option<u32>,
    pub counter: Option<QuotaCounterResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaBackendFailure {
    pub policy_name: Option<String>,
    pub reason: QuotaDenyReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaDecision {
    NotApplied,
    Allowed(QuotaAllowance),
    ShadowDenied(QuotaDenial),
    Denied(QuotaDenial),
    FailedOpen(QuotaBackendFailure),
    FailedClosed(QuotaBackendFailure),
}

impl QuotaDecision {
    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Denied(_) | Self::FailedClosed(_))
    }

    pub fn deny_reason(&self) -> Option<QuotaDenyReason> {
        match self {
            Self::ShadowDenied(denial) | Self::Denied(denial) => Some(denial.reason),
            Self::FailedOpen(failure) | Self::FailedClosed(failure) => Some(failure.reason),
            Self::NotApplied | Self::Allowed(_) => None,
        }
    }
}

pub(crate) fn extract_runtime_request_key(
    spec: &RuntimeRequestKeySpec,
    context: &QuotaIdentityContext<'_>,
) -> RequestKeyExtraction {
    match spec {
        RuntimeRequestKeySpec::Path => extract_path_value(context.path),
        RuntimeRequestKeySpec::Authority => extract_authority_value(context.authority),
        RuntimeRequestKeySpec::Method => extract_method_value(context.method),
        RuntimeRequestKeySpec::Cid | RuntimeRequestKeySpec::StickyCid => {
            extract_cid_value(context.cid_key)
        }
        RuntimeRequestKeySpec::PeerIp | RuntimeRequestKeySpec::ClientIp => {
            extract_client_ip_value(context.client_addr)
        }
        RuntimeRequestKeySpec::BearerToken => extract_bearer_token_value(context.header_lookup),
        RuntimeRequestKeySpec::Header(name) => extract_header_value(name, context.header_lookup),
        RuntimeRequestKeySpec::Cookie(name) => extract_cookie_key_value(name, context.header_lookup),
        RuntimeRequestKeySpec::Query(name) => extract_query_key_value(context.path, name),
    }
}

fn extract_quota_selector_key(
    spec: &QuotaSelectorKeySpec,
    context: &QuotaIdentityContext<'_>,
) -> RequestKeyExtraction {
    match spec {
        QuotaSelectorKeySpec::Path => extract_path_value(context.path),
        QuotaSelectorKeySpec::Authority => extract_authority_value(context.authority),
        QuotaSelectorKeySpec::Method => extract_method_value(context.method),
        QuotaSelectorKeySpec::Cid | QuotaSelectorKeySpec::StickyCid => {
            extract_cid_value(context.cid_key)
        }
        QuotaSelectorKeySpec::PeerIp | QuotaSelectorKeySpec::ClientIp => {
            extract_client_ip_value(context.client_addr)
        }
        QuotaSelectorKeySpec::BearerToken => extract_bearer_token_value(context.header_lookup),
        QuotaSelectorKeySpec::Header(name) => extract_header_value(name, context.header_lookup),
        QuotaSelectorKeySpec::Cookie(name) => extract_cookie_key_value(name, context.header_lookup),
        QuotaSelectorKeySpec::Query(name) => extract_query_key_value(context.path, name),
    }
}

fn extract_path_value(path: &str) -> RequestKeyExtraction {
    let path_only = path.split_once('?').map(|(value, _)| value).unwrap_or(path);
    let normalized = path_only.trim();
    if normalized.is_empty() {
        RequestKeyExtraction::Missing
    } else {
        RequestKeyExtraction::Found(normalized.to_string())
    }
}

fn extract_authority_value(authority: Option<&str>) -> RequestKeyExtraction {
    let Some(authority) = authority.map(str::trim).filter(|value| !value.is_empty()) else {
        return RequestKeyExtraction::Missing;
    };
    RequestKeyExtraction::Found(authority.to_string())
}

fn extract_method_value(method: &str) -> RequestKeyExtraction {
    let normalized = method.trim();
    if normalized.is_empty() {
        RequestKeyExtraction::Missing
    } else {
        RequestKeyExtraction::Found(normalized.to_ascii_uppercase())
    }
}

fn extract_cid_value(cid_key: Option<&str>) -> RequestKeyExtraction {
    let Some(cid_key) = cid_key.map(str::trim).filter(|value| !value.is_empty()) else {
        return RequestKeyExtraction::Missing;
    };
    RequestKeyExtraction::Found(cid_key.to_string())
}

fn extract_client_ip_value(client_addr: Option<SocketAddr>) -> RequestKeyExtraction {
    let Some(client_addr) = client_addr else {
        return RequestKeyExtraction::Missing;
    };
    RequestKeyExtraction::Found(client_addr.ip().to_string())
}

fn extract_bearer_token_value(
    header_lookup: Option<&QuotaHeaderLookup<'_>>,
) -> RequestKeyExtraction {
    let Some(raw) = header_lookup.and_then(|lookup| lookup(http::header::AUTHORIZATION.as_str()))
    else {
        return RequestKeyExtraction::Missing;
    };

    let raw = raw.trim();
    let Some(split) = raw.find(char::is_whitespace) else {
        return RequestKeyExtraction::Invalid;
    };
    let (scheme, rest) = raw.split_at(split);
    if !scheme.eq_ignore_ascii_case("bearer") {
        return RequestKeyExtraction::Invalid;
    }
    let token = rest.trim_start();
    if token.is_empty() {
        return RequestKeyExtraction::Invalid;
    }
    RequestKeyExtraction::Found(token.to_string())
}

fn extract_header_value(
    name: &str,
    header_lookup: Option<&QuotaHeaderLookup<'_>>,
) -> RequestKeyExtraction {
    let Some(value) = header_lookup.and_then(|lookup| lookup(name)) else {
        return RequestKeyExtraction::Missing;
    };
    let normalized = value.trim();
    if normalized.is_empty() {
        return RequestKeyExtraction::Missing;
    }
    RequestKeyExtraction::Found(normalized.to_string())
}

fn extract_cookie_key_value(
    cookie_name: &str,
    header_lookup: Option<&QuotaHeaderLookup<'_>>,
) -> RequestKeyExtraction {
    let Some(cookie_header) =
        header_lookup.and_then(|lookup| lookup(http::header::COOKIE.as_str()))
    else {
        return RequestKeyExtraction::Missing;
    };

    for pair in cookie_header.split(';') {
        let part = pair.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case(cookie_name) {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            return RequestKeyExtraction::Missing;
        }
        return RequestKeyExtraction::Found(value.to_string());
    }

    RequestKeyExtraction::Missing
}

fn extract_query_key_value(path: &str, param: &str) -> RequestKeyExtraction {
    let Some((_, query)) = path.split_once('?') else {
        return RequestKeyExtraction::Missing;
    };

    for pair in query.split('&') {
        let entry = pair.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((name, value)) = entry.split_once('=') else {
            continue;
        };
        if !name.eq_ignore_ascii_case(param) {
            continue;
        }
        if value.is_empty() {
            return RequestKeyExtraction::Missing;
        }
        return RequestKeyExtraction::Found(value.to_string());
    }

    RequestKeyExtraction::Missing
}

fn quota_dimension_is_sensitive(
    dimension: QuotaIdentityDimension,
    spec: &QuotaSelectorKeySpec,
) -> bool {
    matches!(dimension, QuotaIdentityDimension::Token) || matches!(spec, QuotaSelectorKeySpec::BearerToken)
}

fn stable_observable_identity_value(value: &str, sensitive: bool) -> String {
    let normalized = value.trim();
    if !sensitive {
        return normalized.to_string();
    }

    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let digest = hasher.finalize();
    format!("sha256:{digest:x}")
}

fn normalize_route_identity(route: Option<&str>) -> Option<String> {
    route
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn compose_quota_key(policy_name: &str, labels: &QuotaIdentityLabels) -> String {
    let mut key = String::new();
    append_key_component(&mut key, "policy", policy_name);
    if let Some(route) = labels.route.as_deref() {
        append_key_component(&mut key, "route", route);
    }
    if let Some(tenant) = labels.tenant.as_deref() {
        append_key_component(&mut key, "tenant", tenant);
    }
    if let Some(token) = labels.token.as_deref() {
        append_key_component(&mut key, "token", token);
    }
    if let Some(client) = labels.client.as_deref() {
        append_key_component(&mut key, "client", client);
    }
    key
}

fn append_key_component(output: &mut String, label: &str, value: &str) {
    output.push_str(label);
    output.push('=');
    output.push_str(&value.len().to_string());
    output.push(':');
    output.push_str(value);
    output.push('|');
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, net::SocketAddr};

    use super::*;
    use spooky_config::{
        config::{
            DistributedQuotaPolicy, DistributedQuotaSelector, DistributedQuotaSelectorSource,
            DistributedQuotaWindow, QuotaBackendFailurePolicy as RawQuotaBackendFailurePolicy,
            QuotaCounterBackend as RawQuotaCounterBackend,
            QuotaEnforcementMode as RawQuotaEnforcementMode,
            QuotaPolicyConfig as RawQuotaPolicyConfig, Resilience,
        },
        runtime::{
            RuntimeQuotaBackendFailurePolicy, RuntimeQuotaCounterBackend,
            RuntimeQuotaEnforcementMode, RuntimeQuotaPolicy, RuntimeQuotaPolicySet,
            RuntimeQuotaSelectorMatcher, RuntimeQuotaWindow, RuntimeRequestKeySpec,
        },
    };

    fn identity_context_with_headers(
        route: Option<&'static str>,
        method: &'static str,
        path: &'static str,
        authority: Option<&'static str>,
        cid_key: Option<&'static str>,
        client_addr: Option<SocketAddr>,
        headers: HashMap<String, String>,
    ) -> QuotaIdentityContext<'static> {
        let leaked = Box::leak(Box::new(headers));
        let lookup = Box::leak(Box::new(move |name: &str| {
            leaked.get(&name.to_ascii_lowercase()).cloned()
        }));
        QuotaIdentityContext::new(
            route,
            method,
            path,
            authority,
            cid_key,
            client_addr,
            Some(lookup),
        )
    }

    #[test]
    fn quota_deny_reason_slugs_are_stable() {
        assert_eq!(
            QuotaDenyReason::BurstQuotaExhausted.slug(),
            "burst_quota_exhausted"
        );
        assert_eq!(
            QuotaDenyReason::SustainedQuotaExhausted.slug(),
            "sustained_quota_exhausted"
        );
        assert_eq!(
            QuotaDenyReason::SelectorIdentityMissing.slug(),
            "selector_identity_missing"
        );
        assert_eq!(
            QuotaDenyReason::SelectorIdentityInvalid.slug(),
            "selector_identity_invalid"
        );
        assert_eq!(QuotaDenyReason::BackendTimeout.slug(), "backend_timeout");
        assert_eq!(
            QuotaDenyReason::BackendUnavailable.slug(),
            "backend_unavailable"
        );
        assert_eq!(QuotaDenyReason::BackendError.slug(), "backend_error");
    }

    #[test]
    fn runtime_quota_converts_from_runtime_policy_set() {
        let runtime = QuotaRuntime::from_runtime_policy_set(&RuntimeQuotaPolicySet {
            enabled: true,
            enforcement: RuntimeQuotaEnforcementMode::Shadow,
            backend_failure_policy: RuntimeQuotaBackendFailurePolicy::FailOpen,
            backend: RuntimeQuotaCounterBackend::Redis {
                url: "redis://127.0.0.1:6379/0".to_string(),
                key_prefix: "spooky:quota".to_string(),
                connect_timeout: Duration::from_millis(250),
                command_timeout: Duration::from_millis(100),
                max_inflight: 64,
            },
            policies: vec![RuntimeQuotaPolicy {
                name: "tenant-quota".to_string(),
                route_allowlist: vec!["api".to_string()],
                selector: RuntimeQuotaSelectorMatcher {
                    route: true,
                    tenant: Some(RuntimeRequestKeySpec::Header("x-tenant-id".to_string())),
                    token: None,
                    client: Some(RuntimeRequestKeySpec::ClientIp),
                },
                burst: Some(RuntimeQuotaWindow {
                    requests: 100,
                    window: Duration::from_secs(1),
                }),
                sustained: Some(RuntimeQuotaWindow {
                    requests: 1000,
                    window: Duration::from_secs(60),
                }),
            }],
        });

        assert!(runtime.enabled);
        assert_eq!(runtime.enforcement, QuotaEnforcementMode::Shadow);
        assert_eq!(
            runtime.backend_failure_policy,
            QuotaBackendFailurePolicy::FailOpen
        );
        assert_eq!(runtime.policies.len(), 1);
        assert_eq!(runtime.policies[0].name, "tenant-quota");
        assert!(runtime.policies[0].route_allowlist.contains("api"));
        assert_eq!(
            runtime.policies[0].selector.tenant,
            Some(QuotaSelectorKeySpec::Header("x-tenant-id".to_string()))
        );
        assert_eq!(
            runtime.policies[0].selector.client,
            Some(QuotaSelectorKeySpec::ClientIp)
        );
    }

    #[test]
    fn runtime_quota_converts_from_raw_resilience_config() {
        let mut resilience = Resilience::default();
        resilience.quota = RawQuotaPolicyConfig {
            enabled: true,
            enforcement: RawQuotaEnforcementMode::Enforce,
            backend_failure_policy: RawQuotaBackendFailurePolicy::FailClosed,
            backend: RawQuotaCounterBackend::InMemory {
                key_prefix: "spooky:quota".to_string(),
            },
            policies: vec![DistributedQuotaPolicy {
                name: "client-quota".to_string(),
                route_allowlist: vec![" public ".to_string()],
                selector: DistributedQuotaSelector {
                    route: false,
                    tenant: None,
                    token: None,
                    client: Some(DistributedQuotaSelectorSource {
                        key: "client_ip".to_string(),
                    }),
                },
                burst: Some(DistributedQuotaWindow {
                    requests: 25,
                    window_secs: 1,
                }),
                sustained: None,
            }],
        };

        let runtime = QuotaRuntime::from_resilience_config(&resilience);

        assert!(runtime.enabled);
        assert_eq!(runtime.enforcement, QuotaEnforcementMode::Enforce);
        assert_eq!(
            runtime.backend_failure_policy,
            QuotaBackendFailurePolicy::FailClosed
        );
        assert_eq!(runtime.policies[0].name, "client-quota");
        assert!(runtime.policies[0].route_allowlist.contains("public"));
        assert_eq!(
            runtime.policies[0].selector.client,
            Some(QuotaSelectorKeySpec::ClientIp)
        );
        assert_eq!(
            runtime.policies[0].burst.as_ref().map(|window| window.requests),
            Some(25)
        );
    }

    #[test]
    fn quota_decision_reports_terminal_denials() {
        let denied = QuotaDecision::Denied(QuotaDenial {
            policy_name: "tenant-quota".to_string(),
            reason: QuotaDenyReason::BurstQuotaExhausted,
            retry_after_seconds: Some(1),
            counter: None,
        });
        assert!(denied.is_denied());
        assert_eq!(
            denied.deny_reason(),
            Some(QuotaDenyReason::BurstQuotaExhausted)
        );

        let failed_closed = QuotaDecision::FailedClosed(QuotaBackendFailure {
            policy_name: None,
            reason: QuotaDenyReason::BackendUnavailable,
        });
        assert!(failed_closed.is_denied());
        assert_eq!(
            failed_closed.deny_reason(),
            Some(QuotaDenyReason::BackendUnavailable)
        );

        let allowed = QuotaDecision::Allowed(QuotaAllowance {
            policy_name: "tenant-quota".to_string(),
            counter: None,
        });
        assert!(!allowed.is_denied());
        assert_eq!(allowed.deny_reason(), None);
    }

    #[test]
    fn quota_identity_extraction_builds_stable_composite_keys() {
        let policy = QuotaPolicyRuntime {
            name: "tenant-quota".to_string(),
            route_allowlist: HashSet::from(["api".to_string()]),
            selector: QuotaSelectorMatcher {
                route: true,
                tenant: Some(QuotaSelectorKeySpec::Header("x-tenant-id".to_string())),
                token: Some(QuotaSelectorKeySpec::BearerToken),
                client: Some(QuotaSelectorKeySpec::ClientIp),
            },
            burst: Some(QuotaWindowPolicy {
                requests: 100,
                window: Duration::from_secs(1),
            }),
            sustained: Some(QuotaWindowPolicy {
                requests: 1000,
                window: Duration::from_secs(60),
            }),
        };
        let context = identity_context_with_headers(
            Some("api"),
            "POST",
            "/v1/payments?tenant=acme",
            Some("api.example.com"),
            None,
            Some("203.0.113.10:443".parse().expect("client addr")),
            HashMap::from([
                ("authorization".to_string(), "Bearer secret-token".to_string()),
                ("x-tenant-id".to_string(), "acme".to_string()),
            ]),
        );

        let composite = policy
            .composite_key(&context)
            .expect("quota identities should resolve");

        assert_eq!(composite.policy_name, "tenant-quota");
        assert_eq!(composite.labels.route.as_deref(), Some("api"));
        assert_eq!(composite.labels.tenant.as_deref(), Some("acme"));
        assert_eq!(composite.labels.client.as_deref(), Some("203.0.113.10"));
        assert!(
            composite
                .labels
                .token
                .as_deref()
                .is_some_and(|value| value.starts_with("sha256:")),
            "token-derived identities must be hashed before reuse"
        );
        assert_eq!(
            composite.key,
            format!(
                "policy=12:tenant-quota|route=3:api|tenant=4:acme|token={}:{}|client=12:203.0.113.10|",
                composite
                    .labels
                    .token
                    .as_ref()
                    .expect("hashed token label")
                    .len(),
                composite.labels.token.as_ref().expect("hashed token label")
            )
        );
    }

    #[test]
    fn quota_identity_extraction_rejects_missing_and_invalid_selector_values() {
        let policy = QuotaPolicyRuntime {
            name: "tenant-quota".to_string(),
            route_allowlist: HashSet::new(),
            selector: QuotaSelectorMatcher {
                route: true,
                tenant: Some(QuotaSelectorKeySpec::Header("x-tenant-id".to_string())),
                token: Some(QuotaSelectorKeySpec::BearerToken),
                client: None,
            },
            burst: Some(QuotaWindowPolicy {
                requests: 10,
                window: Duration::from_secs(1),
            }),
            sustained: None,
        };

        let missing_tenant = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1",
            Some("api.example.com"),
            None,
            None,
            HashMap::from([(
                "authorization".to_string(),
                "Bearer token-1".to_string(),
            )]),
        );
        let err = policy
            .composite_key(&missing_tenant)
            .expect_err("missing tenant header must reject");
        assert_eq!(err.dimension, QuotaIdentityDimension::Tenant);
        assert_eq!(err.reason, QuotaDenyReason::SelectorIdentityMissing);

        let invalid_token = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1",
            Some("api.example.com"),
            None,
            None,
            HashMap::from([
                ("authorization".to_string(), "Basic abc123".to_string()),
                ("x-tenant-id".to_string(), "acme".to_string()),
            ]),
        );
        let err = policy
            .composite_key(&invalid_token)
            .expect_err("invalid bearer token must reject");
        assert_eq!(err.dimension, QuotaIdentityDimension::Token);
        assert_eq!(err.reason, QuotaDenyReason::SelectorIdentityInvalid);
    }

    #[test]
    fn runtime_request_key_extraction_covers_canonical_identity_sources() {
        let context = identity_context_with_headers(
            Some("api"),
            "post",
            "/v1/payments?tenant=acme",
            Some("api.example.com"),
            Some("cid-123"),
            Some("203.0.113.10:443".parse().expect("client addr")),
            HashMap::from([
                ("authorization".to_string(), "Bearer token-1".to_string()),
                ("cookie".to_string(), "session=s123; theme=dark".to_string()),
                ("x-client-id".to_string(), "client-a".to_string()),
            ]),
        );

        assert_eq!(
            extract_runtime_request_key(&RuntimeRequestKeySpec::Path, &context),
            RequestKeyExtraction::Found("/v1/payments".to_string())
        );
        assert_eq!(
            extract_runtime_request_key(&RuntimeRequestKeySpec::Authority, &context),
            RequestKeyExtraction::Found("api.example.com".to_string())
        );
        assert_eq!(
            extract_runtime_request_key(&RuntimeRequestKeySpec::Method, &context),
            RequestKeyExtraction::Found("POST".to_string())
        );
        assert_eq!(
            extract_runtime_request_key(&RuntimeRequestKeySpec::Cid, &context),
            RequestKeyExtraction::Found("cid-123".to_string())
        );
        assert_eq!(
            extract_runtime_request_key(&RuntimeRequestKeySpec::ClientIp, &context),
            RequestKeyExtraction::Found("203.0.113.10".to_string())
        );
        assert_eq!(
            extract_runtime_request_key(&RuntimeRequestKeySpec::BearerToken, &context),
            RequestKeyExtraction::Found("token-1".to_string())
        );
        assert_eq!(
            extract_runtime_request_key(
                &RuntimeRequestKeySpec::Cookie("session".to_string()),
                &context
            ),
            RequestKeyExtraction::Found("s123".to_string())
        );
        assert_eq!(
            extract_runtime_request_key(
                &RuntimeRequestKeySpec::Query("tenant".to_string()),
                &context
            ),
            RequestKeyExtraction::Found("acme".to_string())
        );
        assert_eq!(
            extract_runtime_request_key(
                &RuntimeRequestKeySpec::Header("x-client-id".to_string()),
                &context
            ),
            RequestKeyExtraction::Found("client-a".to_string())
        );
    }

    #[test]
    fn quota_counter_request_clones_policy_windows_and_defaults_cost() {
        let policy = QuotaPolicyRuntime {
            name: "tenant-quota".to_string(),
            route_allowlist: HashSet::from(["api".to_string()]),
            selector: QuotaSelectorMatcher {
                route: true,
                tenant: Some(QuotaSelectorKeySpec::Header("x-tenant-id".to_string())),
                token: None,
                client: None,
            },
            burst: Some(QuotaWindowPolicy {
                requests: 50,
                window: Duration::from_secs(1),
            }),
            sustained: Some(QuotaWindowPolicy {
                requests: 500,
                window: Duration::from_secs(60),
            }),
        };
        let composite_key = QuotaCompositeKey {
            policy_name: "tenant-quota".to_string(),
            key: "policy=12:tenant-quota|route=3:api|tenant=4:acme|".to_string(),
            labels: QuotaIdentityLabels {
                route: Some("api".to_string()),
                tenant: Some("acme".to_string()),
                token: None,
                client: None,
            },
            dimensions: policy.selector.dimensions(),
        };

        let request = policy.counter_request(composite_key.clone()).with_cost(3);

        assert_eq!(request.policy_name, "tenant-quota");
        assert_eq!(request.composite_key, composite_key);
        assert_eq!(request.cost, 3);
        assert_eq!(request.burst, policy.burst);
        assert_eq!(request.sustained, policy.sustained);
    }

    #[test]
    fn quota_counter_backend_errors_map_to_canonical_deny_reasons() {
        let timeout = QuotaCounterBackendError {
            policy_name: Some("tenant-quota".to_string()),
            composite_key: Some("k1".to_string()),
            kind: QuotaCounterBackendErrorKind::Timeout,
            detail: None,
        };
        assert_eq!(timeout.deny_reason(), QuotaDenyReason::BackendTimeout);

        let unavailable = QuotaCounterBackendError {
            policy_name: Some("tenant-quota".to_string()),
            composite_key: Some("k1".to_string()),
            kind: QuotaCounterBackendErrorKind::Unavailable,
            detail: Some("redis unavailable".to_string()),
        };
        assert_eq!(
            unavailable.deny_reason(),
            QuotaDenyReason::BackendUnavailable
        );

        let error = QuotaCounterBackendError {
            policy_name: Some("tenant-quota".to_string()),
            composite_key: Some("k1".to_string()),
            kind: QuotaCounterBackendErrorKind::Error,
            detail: Some("script error".to_string()),
        };
        assert_eq!(error.deny_reason(), QuotaDenyReason::BackendError);
    }

    #[test]
    fn quota_counter_outcome_captures_multi_window_consumption_and_denial() {
        let composite_key = QuotaCompositeKey {
            policy_name: "tenant-quota".to_string(),
            key: "policy=12:tenant-quota|route=3:api|tenant=4:acme|".to_string(),
            labels: QuotaIdentityLabels {
                route: Some("api".to_string()),
                tenant: Some("acme".to_string()),
                token: None,
                client: None,
            },
            dimensions: QuotaSelectorDimensions {
                route: true,
                tenant: true,
                token: false,
                client: false,
            },
        };

        let outcome = QuotaCounterEvaluationOutcome {
            matched_policy: "tenant-quota".to_string(),
            composite_key,
            decision: QuotaCounterEvaluationDecision::Denied(
                QuotaDenyReason::BurstQuotaExhausted,
            ),
            counter: QuotaCounterResult {
                burst: Some(QuotaWindowUsage {
                    limit: 50,
                    consumed: 50,
                    remaining: 0,
                    window: Duration::from_secs(1),
                    reset_after: Some(Duration::from_millis(750)),
                }),
                sustained: Some(QuotaWindowUsage {
                    limit: 500,
                    consumed: 320,
                    remaining: 180,
                    window: Duration::from_secs(60),
                    reset_after: Some(Duration::from_secs(12)),
                }),
            },
        };

        assert_eq!(outcome.matched_policy, "tenant-quota");
        assert_eq!(
            outcome.decision,
            QuotaCounterEvaluationDecision::Denied(QuotaDenyReason::BurstQuotaExhausted)
        );
        assert_eq!(
            outcome.counter.burst.as_ref().map(|window| window.consumed),
            Some(50)
        );
        assert_eq!(
            outcome.counter.sustained.as_ref().map(|window| window.remaining),
            Some(180)
        );
    }
}
