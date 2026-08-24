use std::{
    collections::HashSet,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, OnceLock, RwLock, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use impulse_config::{
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
        RuntimeQuotaLocalFallback as ConfigRuntimeQuotaLocalFallback,
        RuntimeQuotaPolicy as ConfigRuntimeQuotaPolicy,
        RuntimeQuotaPolicySet as ConfigRuntimeQuotaPolicySet,
        RuntimeQuotaSelectorMatcher as ConfigRuntimeQuotaSelectorMatcher,
        RuntimeQuotaWindow as ConfigRuntimeQuotaWindow, RuntimeRequestKeySpec,
    },
};
use log::{debug, warn};
use sha2::{Digest, Sha256};

use crate::{
    Metrics,
    observability::{QuotaBackendHealthReason, QuotaPolicyDecision, QuotaPolicyReason},
};

mod memory;
mod redis;

pub use memory::{IN_MEMORY_QUOTA_PROTOCOL_VERSION, InMemoryDistributedQuotaCounterStore};
pub use redis::{REDIS_QUOTA_PROTOCOL_VERSION, RedisDistributedQuotaCounterStore};

const LOCAL_FALLBACK_PROTOCOL_VERSION: &str = "degraded-local-fallback/v1";
const LOCAL_FALLBACK_BACKEND_SEPARATOR: &str = "_local_fallback_";
/// Upper bound for request-derived quota identity values handled by this
/// module. Keep this local to quota extraction so later hardening can enforce a
/// single canonical limit without introducing broader runtime config surface.
const MAX_REQUEST_DERIVED_QUOTA_IDENTITY_BYTES: usize = 256;
const MAX_REQUEST_DERIVED_QUOTA_IDENTITY_COMPONENTS: usize = 4;

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

impl QuotaEnforcementMode {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::Enforce => "enforce",
        }
    }
}

impl QuotaBackendFailurePolicy {
    pub fn slug(self) -> &'static str {
        match self {
            Self::FailOpen => "fail_open",
            Self::FailClosed => "fail_closed",
        }
    }
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
    LegacyFallback(Box<QuotaSelectorKeySpec>),
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

impl QuotaIdentityLabels {
    fn canonicalize_for_storage(&self) -> Self {
        Self {
            route: canonical_stored_quota_identity(
                QuotaIdentityDimension::Route,
                self.route.as_deref(),
            ),
            tenant: canonical_stored_quota_identity(
                QuotaIdentityDimension::Tenant,
                self.tenant.as_deref(),
            ),
            token: canonical_stored_quota_identity(
                QuotaIdentityDimension::Token,
                self.token.as_deref(),
            ),
            client: canonical_stored_quota_identity(
                QuotaIdentityDimension::Client,
                self.client.as_deref(),
            ),
        }
    }
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
    fn evaluate<'a>(&'a self, request: QuotaCounterEvaluationRequest)
    -> QuotaCounterEvalFuture<'a>;
}

pub type SharedDistributedQuotaCounterBackend = Arc<dyn DistributedQuotaCounterBackend>;

static QUOTA_METRICS_SINK: OnceLock<RwLock<Weak<Metrics>>> = OnceLock::new();
static QUOTA_INTROSPECTION_STATE: OnceLock<RwLock<QuotaIntrospectionState>> = OnceLock::new();

const QUOTA_RECENT_BACKEND_ERRORS_LIMIT: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaSelectorIntrospectionSnapshot {
    pub route: bool,
    pub tenant: Option<String>,
    pub token: Option<String>,
    pub client: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaWindowIntrospectionSnapshot {
    pub requests: u64,
    pub window_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaPolicyIntrospectionSnapshot {
    pub name: String,
    pub route_allowlist: Vec<String>,
    pub selector: QuotaSelectorIntrospectionSnapshot,
    pub burst: Option<QuotaWindowIntrospectionSnapshot>,
    pub sustained: Option<QuotaWindowIntrospectionSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaBackendErrorSnapshot {
    pub observed_at_unix_ms: Option<u64>,
    pub policy_name: Option<String>,
    pub reason: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaBackendStatusSnapshot {
    pub backend_mode: String,
    pub availability: String,
    pub degraded: bool,
    pub health_reason: Option<String>,
    pub last_observed_at_unix_ms: Option<u64>,
    pub recent_errors: Vec<QuotaBackendErrorSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuotaBackendAvailability {
    Disabled,
    Unknown,
    Available,
    Degraded,
}

impl QuotaBackendAvailability {
    fn slug(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Unknown => "unknown",
            Self::Available => "available",
            Self::Degraded => "degraded",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct QuotaIntrospectionState {
    backend_mode: Option<String>,
    availability: Option<QuotaBackendAvailability>,
    health_reason: Option<QuotaBackendHealthReason>,
    last_observed_at_unix_ms: Option<u64>,
    recent_errors: Vec<QuotaBackendErrorSnapshot>,
}

#[derive(Debug, Clone)]
pub struct UnavailableDistributedQuotaCounterStore {
    error: QuotaCounterBackendError,
}

impl UnavailableDistributedQuotaCounterStore {
    pub fn new(error: QuotaCounterBackendError) -> Self {
        Self { error }
    }
}

impl DistributedQuotaCounterBackend for UnavailableDistributedQuotaCounterStore {
    fn evaluate<'a>(
        &'a self,
        request: QuotaCounterEvaluationRequest,
    ) -> QuotaCounterEvalFuture<'a> {
        let mut error = self.error.clone();
        if error.policy_name.is_none() {
            error.policy_name = Some(request.policy_name);
        }
        if error.composite_key.is_none() {
            error.composite_key = Some(request.composite_key.key);
        }
        Box::pin(async move { Err(error) })
    }
}

#[derive(Clone)]
pub struct DegradedQuotaCounterBackend {
    primary: SharedDistributedQuotaCounterBackend,
    primary_backend_kind: String,
    local_fallback: Arc<InMemoryDistributedQuotaCounterStore>,
}

impl DegradedQuotaCounterBackend {
    pub fn new(
        primary: SharedDistributedQuotaCounterBackend,
        primary_backend_kind: &str,
        local_fallback: Arc<InMemoryDistributedQuotaCounterStore>,
    ) -> Self {
        Self {
            primary,
            primary_backend_kind: primary_backend_kind.to_string(),
            local_fallback,
        }
    }
}

impl DistributedQuotaCounterBackend for DegradedQuotaCounterBackend {
    fn evaluate<'a>(
        &'a self,
        request: QuotaCounterEvaluationRequest,
    ) -> QuotaCounterEvalFuture<'a> {
        Box::pin(async move {
            match self.primary.evaluate(request.clone()).await {
                Ok(outcome) => Ok(outcome),
                Err(primary_error) if should_attempt_local_fallback(&primary_error) => {
                    match self.local_fallback.evaluate(request).await {
                        Ok(mut fallback_outcome) => {
                            fallback_outcome.backend_metadata.backend_kind =
                                local_fallback_backend_mode(
                                    &self.primary_backend_kind,
                                    primary_error.deny_reason(),
                                );
                            fallback_outcome.backend_metadata.protocol_version = format!(
                                "{}+{}",
                                LOCAL_FALLBACK_PROTOCOL_VERSION,
                                fallback_outcome.backend_metadata.protocol_version
                            );
                            Ok(fallback_outcome)
                        }
                        Err(fallback_error) => Err(combine_primary_and_fallback_error(
                            primary_error,
                            fallback_error,
                        )),
                    }
                }
                Err(primary_error) => Err(primary_error),
            }
        })
    }
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

    pub(crate) fn extract_identities(
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

        match extracted {
            RequestKeyExtraction::Found(value) => {
                Ok(Some(canonical_quota_identity_value(dimension, &value)))
            }
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

impl QuotaSelectorDimensions {
    pub fn slug(self) -> String {
        let mut parts = Vec::with_capacity(4);
        if self.route {
            parts.push("route");
        }
        if self.tenant {
            parts.push("tenant");
        }
        if self.token {
            parts.push("token");
        }
        if self.client {
            parts.push("client");
        }
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join("+")
        }
    }
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

    fn introspection_snapshot(&self) -> QuotaWindowIntrospectionSnapshot {
        QuotaWindowIntrospectionSnapshot {
            requests: self.requests,
            window_secs: self.window.as_secs(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaLocalFallbackPolicy {
    pub key_prefix: String,
    pub max_entries: usize,
}

impl QuotaLocalFallbackPolicy {
    fn from_runtime(value: &ConfigRuntimeQuotaLocalFallback) -> Self {
        Self {
            key_prefix: value.key_prefix.clone(),
            max_entries: value.max_entries,
        }
    }

    fn from_raw(value: &impulse_config::config::QuotaLocalFallbackConfig) -> Self {
        Self {
            key_prefix: value.key_prefix.trim().to_string(),
            max_entries: value.max_entries.max(1),
        }
    }

    fn build_store(&self) -> Arc<InMemoryDistributedQuotaCounterStore> {
        Arc::new(InMemoryDistributedQuotaCounterStore::bounded(
            &self.key_prefix,
            self.max_entries,
        ))
    }
}

impl QuotaPolicyRuntime {
    fn from_runtime(value: &ConfigRuntimeQuotaPolicy) -> Self {
        Self {
            name: value.name.clone(),
            route_allowlist: value.route_allowlist.iter().cloned().collect(),
            selector: QuotaSelectorMatcher::from_runtime(&value.selector),
            burst: value.burst.as_ref().map(QuotaWindowPolicy::from_runtime),
            sustained: value
                .sustained
                .as_ref()
                .map(QuotaWindowPolicy::from_runtime),
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

    pub fn counter_request(
        &self,
        composite_key: QuotaCompositeKey,
    ) -> QuotaCounterEvaluationRequest {
        QuotaCounterEvaluationRequest {
            policy_name: self.name.clone(),
            composite_key,
            cost: 1,
            burst: self.burst.clone(),
            sustained: self.sustained.clone(),
        }
    }

    fn applies_to_route(&self, route: &str) -> bool {
        self.route_allowlist.is_empty() || self.route_allowlist.contains(route)
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
    pub(crate) fn from_raw_key(value: &str) -> Self {
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

    pub(crate) fn with_legacy_default_fallback(self) -> Self {
        Self::LegacyFallback(Box::new(self))
    }

    pub fn descriptor(&self) -> String {
        match self {
            Self::Path => "path".to_string(),
            Self::Authority => "authority".to_string(),
            Self::Method => "method".to_string(),
            Self::Cid => "cid".to_string(),
            Self::StickyCid => "sticky-cid".to_string(),
            Self::PeerIp => "peer_ip".to_string(),
            Self::ClientIp => "client_ip".to_string(),
            Self::BearerToken => "bearer_token".to_string(),
            Self::Header(name) => format!("header:{name}"),
            Self::Cookie(name) => format!("cookie:{name}"),
            Self::Query(name) => format!("query:{name}"),
            Self::LegacyFallback(inner) => inner.descriptor(),
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
    pub fn backend_kind(&self) -> &'static str {
        match self {
            Self::InMemory { .. } => "in_memory",
            Self::Redis { .. } => "redis",
        }
    }

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

    pub fn redis_store(
        &self,
    ) -> Result<Option<Arc<RedisDistributedQuotaCounterStore>>, QuotaCounterBackendError> {
        match self {
            Self::InMemory { .. } => Ok(None),
            Self::Redis {
                url,
                key_prefix,
                connect_timeout,
                command_timeout,
                max_inflight,
            } => RedisDistributedQuotaCounterStore::new(
                url,
                key_prefix,
                *connect_timeout,
                *command_timeout,
                *max_inflight,
            )
            .map(|store| Some(Arc::new(store))),
        }
    }

    pub fn in_memory_store(
        &self,
    ) -> Result<Option<Arc<InMemoryDistributedQuotaCounterStore>>, QuotaCounterBackendError> {
        match self {
            Self::InMemory { key_prefix } => Ok(Some(Arc::new(
                InMemoryDistributedQuotaCounterStore::new(key_prefix),
            ))),
            Self::Redis { .. } => Ok(None),
        }
    }

    pub fn distributed_store(
        &self,
    ) -> Result<SharedDistributedQuotaCounterBackend, QuotaCounterBackendError> {
        match self {
            Self::InMemory { key_prefix } => Ok(Arc::new(
                InMemoryDistributedQuotaCounterStore::new(key_prefix),
            )),
            Self::Redis {
                url,
                key_prefix,
                connect_timeout,
                command_timeout,
                max_inflight,
            } => Ok(Arc::new(RedisDistributedQuotaCounterStore::new(
                url,
                key_prefix,
                *connect_timeout,
                *command_timeout,
                *max_inflight,
            )?)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaRuntime {
    pub enabled: bool,
    pub enforcement: QuotaEnforcementMode,
    pub backend_failure_policy: QuotaBackendFailurePolicy,
    pub backend: QuotaCounterBackend,
    pub local_fallback: Option<QuotaLocalFallbackPolicy>,
    pub policies: Vec<QuotaPolicyRuntime>,
}

impl QuotaRuntime {
    pub fn register_metrics(metrics: &Arc<Metrics>) {
        if let Ok(mut sink) = quota_metrics_sink().write() {
            *sink = Arc::downgrade(metrics);
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            enforcement: QuotaEnforcementMode::Enforce,
            backend_failure_policy: QuotaBackendFailurePolicy::FailClosed,
            backend: QuotaCounterBackend::InMemory {
                key_prefix: "impulse:quota".to_string(),
            },
            local_fallback: None,
            policies: Vec::new(),
        }
    }

    pub fn from_resilience_config(config: &ResilienceConfig) -> Self {
        Self::from_raw_config(&config.quota)
    }

    pub fn from_rate_limit_policies(
        rate_limit_policy: &impulse_config::runtime::RuntimeRateLimitPolicy,
    ) -> Self {
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
            local_fallback: config
                .local_fallback
                .as_ref()
                .map(QuotaLocalFallbackPolicy::from_runtime),
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
            local_fallback: config
                .local_fallback
                .as_ref()
                .map(QuotaLocalFallbackPolicy::from_raw),
            policies: config
                .policies
                .iter()
                .map(QuotaPolicyRuntime::from_raw)
                .collect(),
        }
    }

    pub fn redis_store(
        &self,
    ) -> Result<Option<Arc<RedisDistributedQuotaCounterStore>>, QuotaCounterBackendError> {
        self.backend.redis_store()
    }

    pub fn in_memory_store(
        &self,
    ) -> Result<Option<Arc<InMemoryDistributedQuotaCounterStore>>, QuotaCounterBackendError> {
        self.backend.in_memory_store()
    }

    pub fn distributed_store(
        &self,
    ) -> Result<SharedDistributedQuotaCounterBackend, QuotaCounterBackendError> {
        self.backend.distributed_store()
    }

    pub fn enforcement_backend(
        &self,
    ) -> (
        SharedDistributedQuotaCounterBackend,
        Option<QuotaCounterBackendError>,
    ) {
        let mut initialization_error = None;
        let primary = match self.distributed_store() {
            Ok(backend) => backend,
            Err(error) => {
                initialization_error = Some(error.clone());
                Arc::new(UnavailableDistributedQuotaCounterStore::new(error))
                    as SharedDistributedQuotaCounterBackend
            }
        };

        let backend = self
            .local_fallback
            .as_ref()
            .map(|fallback| {
                Arc::new(DegradedQuotaCounterBackend::new(
                    Arc::clone(&primary),
                    self.backend.backend_kind(),
                    fallback.build_store(),
                )) as SharedDistributedQuotaCounterBackend
            })
            .unwrap_or(primary);

        (backend, initialization_error)
    }

    pub fn policy_snapshots(&self) -> Vec<QuotaPolicyIntrospectionSnapshot> {
        self.policies
            .iter()
            .map(|policy| {
                let mut route_allowlist =
                    policy.route_allowlist.iter().cloned().collect::<Vec<_>>();
                route_allowlist.sort();
                QuotaPolicyIntrospectionSnapshot {
                    name: policy.name.clone(),
                    route_allowlist,
                    selector: QuotaSelectorIntrospectionSnapshot {
                        route: policy.selector.route,
                        tenant: policy
                            .selector
                            .tenant
                            .as_ref()
                            .map(QuotaSelectorKeySpec::descriptor),
                        token: policy
                            .selector
                            .token
                            .as_ref()
                            .map(QuotaSelectorKeySpec::descriptor),
                        client: policy
                            .selector
                            .client
                            .as_ref()
                            .map(QuotaSelectorKeySpec::descriptor),
                    },
                    burst: policy
                        .burst
                        .as_ref()
                        .map(QuotaWindowPolicy::introspection_snapshot),
                    sustained: policy
                        .sustained
                        .as_ref()
                        .map(QuotaWindowPolicy::introspection_snapshot),
                }
            })
            .collect()
    }

    pub fn backend_status_snapshot(
        &self,
        initialization_error: Option<&QuotaCounterBackendError>,
    ) -> QuotaBackendStatusSnapshot {
        let backend_mode = self.backend.backend_kind().to_string();
        if !self.enabled {
            return QuotaBackendStatusSnapshot {
                backend_mode,
                availability: QuotaBackendAvailability::Disabled.slug().to_string(),
                degraded: false,
                health_reason: None,
                last_observed_at_unix_ms: None,
                recent_errors: Vec::new(),
            };
        }

        if let Some(error) = initialization_error {
            return QuotaBackendStatusSnapshot {
                backend_mode: backend_mode.clone(),
                availability: QuotaBackendAvailability::Degraded.slug().to_string(),
                degraded: true,
                health_reason: Some(
                    quota_backend_health_reason_from_deny_reason(error.deny_reason())
                        .slug()
                        .to_string(),
                ),
                last_observed_at_unix_ms: None,
                recent_errors: vec![QuotaBackendErrorSnapshot {
                    observed_at_unix_ms: None,
                    policy_name: error.policy_name.clone(),
                    reason: error.deny_reason().slug().to_string(),
                    detail: error.detail.clone(),
                }],
            };
        }

        let state = current_quota_introspection_state();
        if state.backend_mode.as_deref() == Some(backend_mode.as_str()) {
            let availability = state
                .availability
                .unwrap_or_else(|| default_quota_backend_availability(&self.backend));
            return QuotaBackendStatusSnapshot {
                backend_mode,
                availability: availability.slug().to_string(),
                degraded: matches!(availability, QuotaBackendAvailability::Degraded),
                health_reason: state.health_reason.map(|reason| reason.slug().to_string()),
                last_observed_at_unix_ms: state.last_observed_at_unix_ms,
                recent_errors: state.recent_errors,
            };
        }

        let availability = default_quota_backend_availability(&self.backend);
        QuotaBackendStatusSnapshot {
            backend_mode,
            availability: availability.slug().to_string(),
            degraded: matches!(availability, QuotaBackendAvailability::Degraded),
            health_reason: None,
            last_observed_at_unix_ms: None,
            recent_errors: Vec::new(),
        }
    }
}

impl QuotaPolicyRuntime {
    pub(crate) fn composite_key(
        &self,
        context: &QuotaIdentityContext<'_>,
    ) -> Result<QuotaCompositeKey, QuotaIdentityRejection> {
        let labels = self.selector.extract_identities(&self.name, context)?;
        let labels = labels.canonicalize_for_storage();
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

    pub fn from_slug(value: &str) -> Option<Self> {
        match value {
            "burst_quota_exhausted" => Some(Self::BurstQuotaExhausted),
            "sustained_quota_exhausted" => Some(Self::SustainedQuotaExhausted),
            "selector_identity_missing" => Some(Self::SelectorIdentityMissing),
            "selector_identity_invalid" => Some(Self::SelectorIdentityInvalid),
            "backend_timeout" => Some(Self::BackendTimeout),
            "backend_unavailable" => Some(Self::BackendUnavailable),
            "backend_error" => Some(Self::BackendError),
            _ => None,
        }
    }
}

fn local_fallback_backend_mode(primary_backend_kind: &str, reason: QuotaDenyReason) -> String {
    format!(
        "{}{}{}",
        primary_backend_kind,
        LOCAL_FALLBACK_BACKEND_SEPARATOR,
        reason.slug()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaWindowUsage {
    pub limit: u64,
    pub consumed: u64,
    pub remaining: u64,
    pub window: Duration,
    pub reset_after: Option<Duration>,
    pub bucket_started_at_unix_ms: Option<u64>,
    pub reset_at_unix_ms: Option<u64>,
    pub storage_key: Option<String>,
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
pub struct QuotaCounterBackendMetadata {
    pub backend_kind: String,
    pub protocol_version: String,
    pub evaluated_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaCounterEvaluationOutcome {
    pub matched_policy: String,
    pub composite_key: QuotaCompositeKey,
    pub decision: QuotaCounterEvaluationDecision,
    pub counter: QuotaCounterResult,
    pub backend_metadata: QuotaCounterBackendMetadata,
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

fn should_attempt_local_fallback(error: &QuotaCounterBackendError) -> bool {
    matches!(
        error.kind,
        QuotaCounterBackendErrorKind::Timeout | QuotaCounterBackendErrorKind::Unavailable
    )
}

fn combine_primary_and_fallback_error(
    primary: QuotaCounterBackendError,
    fallback: QuotaCounterBackendError,
) -> QuotaCounterBackendError {
    QuotaCounterBackendError {
        policy_name: fallback.policy_name.or(primary.policy_name),
        composite_key: fallback.composite_key.or(primary.composite_key),
        kind: fallback.kind,
        detail: Some(match (primary.detail, fallback.detail) {
            (Some(primary_detail), Some(fallback_detail)) => format!(
                "primary quota backend failed: {primary_detail}; local fallback failed: {fallback_detail}"
            ),
            (Some(primary_detail), None) => {
                format!("primary quota backend failed: {primary_detail}; local fallback failed")
            }
            (None, Some(fallback_detail)) => {
                format!("local fallback failed after primary backend outage: {fallback_detail}")
            }
            (None, None) => "primary quota backend and local fallback both failed".to_string(),
        }),
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

pub(crate) async fn evaluate_admission_quota(
    runtime: &QuotaRuntime,
    backend: &dyn DistributedQuotaCounterBackend,
    context: &QuotaIdentityContext<'_>,
) -> QuotaDecision {
    if !runtime.enabled {
        return QuotaDecision::NotApplied;
    }

    let Some(route) = context.route else {
        return QuotaDecision::NotApplied;
    };

    // Every policy whose allowlist covers this route is a separate contract and
    // must be evaluated. Stopping at the first match would let a broad policy
    // mask a narrower one layered on the same route, so the narrower limit
    // would validate at startup and appear in the control API while silently
    // enforcing nothing.
    let mut outcome = QuotaDecision::NotApplied;
    let mut shadow_denial: Option<QuotaDecision> = None;

    for policy in runtime
        .policies
        .iter()
        .filter(|policy| policy.applies_to_route(route))
    {
        match evaluate_quota_policy(runtime, backend, context, policy).await {
            // A real denial is final: the request violated at least one
            // contract, and no later policy can overturn that. Returning here
            // also avoids charging the caller's remaining budgets for a request
            // that is about to be rejected.
            decision @ (QuotaDecision::Denied(_) | QuotaDecision::FailedClosed(_)) => {
                return decision;
            }
            // Shadow mode must keep evaluating so every policy records an
            // outcome — measuring one policy's blast radius is the whole point,
            // and stopping early would under-count the rest. Keep the first
            // would-deny as the reported cause.
            decision @ QuotaDecision::ShadowDenied(_) => {
                shadow_denial.get_or_insert(decision);
            }
            // Remember a real allowance so the caller sees it (and its counter)
            // rather than NotApplied.
            decision @ (QuotaDecision::Allowed(_) | QuotaDecision::FailedOpen(_)) => {
                if !matches!(outcome, QuotaDecision::Allowed(_)) {
                    outcome = decision;
                }
            }
            QuotaDecision::NotApplied => {}
        }
    }

    shadow_denial.unwrap_or(outcome)
}

async fn evaluate_quota_policy(
    runtime: &QuotaRuntime,
    backend: &dyn DistributedQuotaCounterBackend,
    context: &QuotaIdentityContext<'_>,
    policy: &QuotaPolicyRuntime,
) -> QuotaDecision {
    let composite_key = match policy.composite_key(context) {
        Ok(key) => key,
        Err(rejection) => {
            let decision = quota_rejection_decision(
                runtime.enforcement,
                QuotaDenial {
                    policy_name: rejection.policy_name,
                    reason: rejection.reason,
                    retry_after_seconds: None,
                    counter: None,
                },
            );
            observe_quota_policy_outcome(
                runtime,
                Some(policy),
                context,
                &decision,
                false,
                None,
                None,
            );
            return decision;
        }
    };

    let backend_mode = runtime.backend.backend_kind();
    let (decision, backend_observed, backend_mode, backend_error_detail) = match backend
        .evaluate(policy.counter_request(composite_key))
        .await
    {
        Ok(outcome) => match outcome.decision {
            QuotaCounterEvaluationDecision::Allowed => (
                QuotaDecision::Allowed(QuotaAllowance {
                    policy_name: outcome.matched_policy,
                    counter: Some(outcome.counter),
                }),
                true,
                outcome.backend_metadata.backend_kind,
                None,
            ),
            QuotaCounterEvaluationDecision::Denied(reason) => (
                quota_rejection_decision(
                    runtime.enforcement,
                    QuotaDenial {
                        policy_name: outcome.matched_policy,
                        reason,
                        retry_after_seconds: quota_retry_after_seconds(reason, &outcome.counter),
                        counter: Some(outcome.counter),
                    },
                ),
                true,
                outcome.backend_metadata.backend_kind,
                None,
            ),
        },
        Err(error) => {
            let deny_reason = error.deny_reason();
            let error_detail = error.detail.clone();
            let decision = match runtime.backend_failure_policy {
                QuotaBackendFailurePolicy::FailOpen => {
                    QuotaDecision::FailedOpen(QuotaBackendFailure {
                        policy_name: error.policy_name.or_else(|| Some(policy.name.clone())),
                        reason: deny_reason,
                    })
                }
                QuotaBackendFailurePolicy::FailClosed => {
                    QuotaDecision::FailedClosed(QuotaBackendFailure {
                        policy_name: error.policy_name.or_else(|| Some(policy.name.clone())),
                        reason: deny_reason,
                    })
                }
            };
            (decision, true, backend_mode.to_string(), error_detail)
        }
    };

    observe_quota_policy_outcome(
        runtime,
        Some(policy),
        context,
        &decision,
        backend_observed,
        Some(backend_mode.as_str()),
        backend_error_detail,
    );
    decision
}

fn quota_rejection_decision(
    enforcement: QuotaEnforcementMode,
    denial: QuotaDenial,
) -> QuotaDecision {
    match enforcement {
        QuotaEnforcementMode::Shadow => QuotaDecision::ShadowDenied(denial),
        QuotaEnforcementMode::Enforce => QuotaDecision::Denied(denial),
    }
}

fn quota_retry_after_seconds(reason: QuotaDenyReason, counter: &QuotaCounterResult) -> Option<u32> {
    let reset_after = match reason {
        QuotaDenyReason::BurstQuotaExhausted => {
            counter.burst.as_ref().and_then(|window| window.reset_after)
        }
        QuotaDenyReason::SustainedQuotaExhausted => counter
            .sustained
            .as_ref()
            .and_then(|window| window.reset_after),
        QuotaDenyReason::SelectorIdentityMissing
        | QuotaDenyReason::SelectorIdentityInvalid
        | QuotaDenyReason::BackendTimeout
        | QuotaDenyReason::BackendUnavailable
        | QuotaDenyReason::BackendError => None,
    }?;

    let rounded = reset_after
        .as_secs()
        .saturating_add(u64::from(reset_after.subsec_nanos() > 0));
    Some(rounded.max(1).min(u64::from(u32::MAX)) as u32)
}

fn quota_metrics_sink() -> &'static RwLock<Weak<Metrics>> {
    QUOTA_METRICS_SINK.get_or_init(|| RwLock::new(Weak::new()))
}

fn quota_introspection_state() -> &'static RwLock<QuotaIntrospectionState> {
    QUOTA_INTROSPECTION_STATE.get_or_init(|| RwLock::new(QuotaIntrospectionState::default()))
}

fn current_quota_metrics() -> Option<Arc<Metrics>> {
    quota_metrics_sink()
        .read()
        .ok()
        .and_then(|metrics| metrics.upgrade())
}

fn current_quota_introspection_state() -> QuotaIntrospectionState {
    quota_introspection_state()
        .read()
        .map(|state| state.clone())
        .unwrap_or_default()
}

fn record_quota_backend_observation(
    backend_mode: &str,
    decision: &QuotaDecision,
    detail: Option<String>,
    observed_at_unix_ms: u64,
) {
    let degraded_reason = degraded_backend_health_reason(backend_mode);
    let availability = if degraded_reason.is_some() {
        QuotaBackendAvailability::Degraded
    } else {
        match decision {
            QuotaDecision::Allowed(_)
            | QuotaDecision::Denied(_)
            | QuotaDecision::ShadowDenied(_) => QuotaBackendAvailability::Available,
            QuotaDecision::FailedOpen(_) | QuotaDecision::FailedClosed(_) => {
                QuotaBackendAvailability::Degraded
            }
            QuotaDecision::NotApplied => return,
        }
    };
    let health_reason = quota_backend_health_reason(decision, backend_mode);

    if let Ok(mut state) = quota_introspection_state().write() {
        state.backend_mode = Some(backend_mode.to_string());
        state.availability = Some(availability);
        state.health_reason = health_reason;
        state.last_observed_at_unix_ms = Some(observed_at_unix_ms);

        if let Some(snapshot) =
            quota_backend_error_snapshot(decision, backend_mode, detail, observed_at_unix_ms)
        {
            state.recent_errors.insert(0, snapshot);
            if state.recent_errors.len() > QUOTA_RECENT_BACKEND_ERRORS_LIMIT {
                state
                    .recent_errors
                    .truncate(QUOTA_RECENT_BACKEND_ERRORS_LIMIT);
            }
        }
    }
}

fn quota_backend_error_snapshot(
    decision: &QuotaDecision,
    backend_mode: &str,
    detail: Option<String>,
    observed_at_unix_ms: u64,
) -> Option<QuotaBackendErrorSnapshot> {
    if let Some(reason) = degraded_backend_deny_reason(backend_mode) {
        return Some(QuotaBackendErrorSnapshot {
            observed_at_unix_ms: Some(observed_at_unix_ms),
            policy_name: quota_decision_policy_name(decision).map(ToOwned::to_owned),
            reason: reason.slug().to_string(),
            detail,
        });
    }

    match decision {
        QuotaDecision::FailedOpen(failure) | QuotaDecision::FailedClosed(failure) => {
            Some(QuotaBackendErrorSnapshot {
                observed_at_unix_ms: Some(observed_at_unix_ms),
                policy_name: failure.policy_name.clone(),
                reason: failure.reason.slug().to_string(),
                detail,
            })
        }
        QuotaDecision::NotApplied
        | QuotaDecision::Allowed(_)
        | QuotaDecision::Denied(_)
        | QuotaDecision::ShadowDenied(_) => None,
    }
}

fn default_quota_backend_availability(backend: &QuotaCounterBackend) -> QuotaBackendAvailability {
    match backend {
        QuotaCounterBackend::InMemory { .. } => QuotaBackendAvailability::Available,
        QuotaCounterBackend::Redis { .. } => QuotaBackendAvailability::Unknown,
    }
}

fn observe_quota_policy_outcome(
    runtime: &QuotaRuntime,
    policy: Option<&QuotaPolicyRuntime>,
    context: &QuotaIdentityContext<'_>,
    decision: &QuotaDecision,
    backend_observed: bool,
    backend_mode: Option<&str>,
    backend_error_detail: Option<String>,
) {
    let policy_name = policy
        .map(|value| value.name.as_str())
        .or(match decision {
            QuotaDecision::Allowed(allowance) => Some(allowance.policy_name.as_str()),
            QuotaDecision::Denied(denial) | QuotaDecision::ShadowDenied(denial) => {
                Some(denial.policy_name.as_str())
            }
            QuotaDecision::FailedOpen(failure) | QuotaDecision::FailedClosed(failure) => {
                failure.policy_name.as_deref()
            }
            QuotaDecision::NotApplied => None,
        })
        .unwrap_or("unmatched");
    let selector_dimensions =
        policy
            .map(|value| value.selector.dimensions())
            .unwrap_or(QuotaSelectorDimensions {
                route: false,
                tenant: false,
                token: false,
                client: false,
            });
    let selector_dimensions = selector_dimensions.slug();
    let backend_mode = backend_mode.unwrap_or(runtime.backend.backend_kind());
    let decision_kind = quota_policy_decision_kind(decision);
    let reason = quota_policy_reason(decision);
    let degraded_health_reason = degraded_backend_health_reason(backend_mode);

    if backend_observed {
        record_quota_backend_observation(
            backend_mode,
            decision,
            backend_error_detail,
            unix_now_ms(),
        );
    }

    if let Some(metrics) = current_quota_metrics() {
        metrics.record_quota_policy_outcome(
            policy_name,
            decision_kind,
            reason,
            &selector_dimensions,
            backend_mode,
        );
        if backend_observed
            && let Some(health_reason) = quota_backend_health_reason(decision, backend_mode)
        {
            metrics.record_quota_backend_health(backend_mode, health_reason);
        }
    }

    let route = context.route.unwrap_or("unrouted");
    let reason_slug = reason.slug();
    let degraded_reason = degraded_health_reason.map(QuotaBackendHealthReason::slug);
    let log_line = format!(
        "quota policy outcome: upstream={} policy={} selector_dimensions={} backend_mode={} decision={} reason={} enforcement={} degraded_reason={}",
        route,
        policy_name,
        selector_dimensions,
        backend_mode,
        decision_kind.slug(),
        reason_slug,
        quota_enforcement_slug(runtime.enforcement),
        degraded_reason.unwrap_or("none"),
    );
    if degraded_reason.is_some() {
        warn!("{log_line}");
    } else {
        match decision_kind {
            QuotaPolicyDecision::Denied | QuotaPolicyDecision::FailedClosed => warn!("{log_line}"),
            QuotaPolicyDecision::FailedOpen | QuotaPolicyDecision::ShadowDenied => {
                warn!("{log_line}")
            }
            QuotaPolicyDecision::Allowed | QuotaPolicyDecision::NotApplied => debug!("{log_line}"),
        }
    }
}

fn quota_policy_decision_kind(decision: &QuotaDecision) -> QuotaPolicyDecision {
    match decision {
        QuotaDecision::NotApplied => QuotaPolicyDecision::NotApplied,
        QuotaDecision::Allowed(_) => QuotaPolicyDecision::Allowed,
        QuotaDecision::ShadowDenied(_) => QuotaPolicyDecision::ShadowDenied,
        QuotaDecision::Denied(_) => QuotaPolicyDecision::Denied,
        QuotaDecision::FailedOpen(_) => QuotaPolicyDecision::FailedOpen,
        QuotaDecision::FailedClosed(_) => QuotaPolicyDecision::FailedClosed,
    }
}

fn quota_policy_reason(decision: &QuotaDecision) -> QuotaPolicyReason {
    match decision {
        QuotaDecision::NotApplied => QuotaPolicyReason::NotApplied,
        QuotaDecision::Allowed(_) => QuotaPolicyReason::Allowed,
        QuotaDecision::ShadowDenied(denial) | QuotaDecision::Denied(denial) => {
            quota_policy_reason_from_deny_reason(denial.reason)
        }
        QuotaDecision::FailedOpen(failure) | QuotaDecision::FailedClosed(failure) => {
            quota_policy_reason_from_deny_reason(failure.reason)
        }
    }
}

fn quota_policy_reason_from_deny_reason(reason: QuotaDenyReason) -> QuotaPolicyReason {
    match reason {
        QuotaDenyReason::BurstQuotaExhausted => QuotaPolicyReason::BurstQuotaExhausted,
        QuotaDenyReason::SustainedQuotaExhausted => QuotaPolicyReason::SustainedQuotaExhausted,
        QuotaDenyReason::SelectorIdentityMissing => QuotaPolicyReason::SelectorIdentityMissing,
        QuotaDenyReason::SelectorIdentityInvalid => QuotaPolicyReason::SelectorIdentityInvalid,
        QuotaDenyReason::BackendTimeout => QuotaPolicyReason::BackendTimeout,
        QuotaDenyReason::BackendUnavailable => QuotaPolicyReason::BackendUnavailable,
        QuotaDenyReason::BackendError => QuotaPolicyReason::BackendError,
    }
}

fn quota_backend_health_reason(
    decision: &QuotaDecision,
    backend_mode: &str,
) -> Option<QuotaBackendHealthReason> {
    if let Some(reason) = degraded_backend_health_reason(backend_mode) {
        return Some(reason);
    }

    match decision {
        QuotaDecision::Allowed(_) | QuotaDecision::Denied(_) | QuotaDecision::ShadowDenied(_) => {
            Some(QuotaBackendHealthReason::Available)
        }
        QuotaDecision::FailedOpen(failure) | QuotaDecision::FailedClosed(failure) => {
            Some(match failure.reason {
                QuotaDenyReason::BackendTimeout => QuotaBackendHealthReason::Timeout,
                QuotaDenyReason::BackendUnavailable => QuotaBackendHealthReason::Unavailable,
                QuotaDenyReason::BackendError => QuotaBackendHealthReason::Error,
                QuotaDenyReason::BurstQuotaExhausted
                | QuotaDenyReason::SustainedQuotaExhausted
                | QuotaDenyReason::SelectorIdentityMissing
                | QuotaDenyReason::SelectorIdentityInvalid => return None,
            })
        }
        QuotaDecision::NotApplied => None,
    }
}

fn quota_backend_health_reason_from_deny_reason(
    reason: QuotaDenyReason,
) -> QuotaBackendHealthReason {
    match reason {
        QuotaDenyReason::BackendTimeout => QuotaBackendHealthReason::Timeout,
        QuotaDenyReason::BackendUnavailable => QuotaBackendHealthReason::Unavailable,
        QuotaDenyReason::BackendError => QuotaBackendHealthReason::Error,
        QuotaDenyReason::BurstQuotaExhausted
        | QuotaDenyReason::SustainedQuotaExhausted
        | QuotaDenyReason::SelectorIdentityMissing
        | QuotaDenyReason::SelectorIdentityInvalid => QuotaBackendHealthReason::Error,
    }
}

fn quota_enforcement_slug(enforcement: QuotaEnforcementMode) -> &'static str {
    enforcement.slug()
}

fn degraded_backend_health_reason(backend_mode: &str) -> Option<QuotaBackendHealthReason> {
    degraded_backend_deny_reason(backend_mode).map(quota_backend_health_reason_from_deny_reason)
}

fn degraded_backend_deny_reason(backend_mode: &str) -> Option<QuotaDenyReason> {
    let suffix = backend_mode
        .rsplit_once(LOCAL_FALLBACK_BACKEND_SEPARATOR)
        .map(|(_, suffix)| suffix)?;
    QuotaDenyReason::from_slug(suffix)
}

fn quota_decision_policy_name(decision: &QuotaDecision) -> Option<&str> {
    match decision {
        QuotaDecision::Allowed(allowance) => Some(allowance.policy_name.as_str()),
        QuotaDecision::ShadowDenied(denial) | QuotaDecision::Denied(denial) => {
            Some(denial.policy_name.as_str())
        }
        QuotaDecision::FailedOpen(failure) | QuotaDecision::FailedClosed(failure) => {
            failure.policy_name.as_deref()
        }
        QuotaDecision::NotApplied => None,
    }
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
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
        RuntimeRequestKeySpec::Cookie(name) => {
            extract_cookie_key_value(name, context.header_lookup)
        }
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
        QuotaSelectorKeySpec::LegacyFallback(inner) => {
            match extract_quota_selector_key(inner.as_ref(), context) {
                RequestKeyExtraction::Found(value) => RequestKeyExtraction::Found(value),
                RequestKeyExtraction::Missing => extract_legacy_default_request_key(context),
                RequestKeyExtraction::Invalid => RequestKeyExtraction::Invalid,
            }
        }
    }
}

fn extract_legacy_default_request_key(context: &QuotaIdentityContext<'_>) -> RequestKeyExtraction {
    if let Some(authority) = context
        .authority
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return bounded_request_key_value(authority);
    }

    let path = context.path.trim();
    if !path.is_empty() {
        return bounded_request_key_value(path);
    }

    let method = context.method.trim();
    if !method.is_empty() {
        return bounded_owned_request_key_value(method.to_ascii_uppercase());
    }

    RequestKeyExtraction::Missing
}

fn extract_path_value(path: &str) -> RequestKeyExtraction {
    let path_only = path.split_once('?').map(|(value, _)| value).unwrap_or(path);
    bounded_request_key_value(path_only)
}

fn extract_authority_value(authority: Option<&str>) -> RequestKeyExtraction {
    let Some(authority) = authority.map(str::trim).filter(|value| !value.is_empty()) else {
        return RequestKeyExtraction::Missing;
    };
    bounded_request_key_value(authority)
}

fn extract_method_value(method: &str) -> RequestKeyExtraction {
    let normalized = method.trim();
    if normalized.is_empty() {
        RequestKeyExtraction::Missing
    } else {
        bounded_owned_request_key_value(normalized.to_ascii_uppercase())
    }
}

fn extract_cid_value(cid_key: Option<&str>) -> RequestKeyExtraction {
    let Some(cid_key) = cid_key.map(str::trim).filter(|value| !value.is_empty()) else {
        return RequestKeyExtraction::Missing;
    };
    bounded_request_key_value(cid_key)
}

fn extract_client_ip_value(client_addr: Option<SocketAddr>) -> RequestKeyExtraction {
    let Some(client_addr) = client_addr else {
        return RequestKeyExtraction::Missing;
    };
    bounded_owned_request_key_value(client_addr.ip().to_string())
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
    bounded_request_key_value(token)
}

fn extract_header_value(
    name: &str,
    header_lookup: Option<&QuotaHeaderLookup<'_>>,
) -> RequestKeyExtraction {
    let Some(value) = header_lookup.and_then(|lookup| lookup(name)) else {
        return RequestKeyExtraction::Missing;
    };
    bounded_request_key_value(value.as_str())
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
        return bounded_request_key_value(value);
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
        return bounded_request_key_value(value);
    }

    RequestKeyExtraction::Missing
}

fn bounded_request_key_value(value: &str) -> RequestKeyExtraction {
    let normalized = value.trim();
    if normalized.is_empty() {
        return RequestKeyExtraction::Missing;
    }
    if normalized.len() > MAX_REQUEST_DERIVED_QUOTA_IDENTITY_BYTES {
        return RequestKeyExtraction::Invalid;
    }
    RequestKeyExtraction::Found(normalized.to_string())
}

fn bounded_owned_request_key_value(value: String) -> RequestKeyExtraction {
    if value.is_empty() {
        return RequestKeyExtraction::Missing;
    }
    if value.len() > MAX_REQUEST_DERIVED_QUOTA_IDENTITY_BYTES {
        return RequestKeyExtraction::Invalid;
    }
    RequestKeyExtraction::Found(value)
}

fn canonical_quota_identity_value(dimension: QuotaIdentityDimension, value: &str) -> String {
    let normalized = value.trim();
    if matches!(dimension, QuotaIdentityDimension::Route) {
        return normalized.to_string();
    }

    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let digest = hasher.finalize();
    format!("sha256:{digest:x}")
}

fn canonical_stored_quota_identity(
    dimension: QuotaIdentityDimension,
    value: Option<&str>,
) -> Option<String> {
    let normalized = value.map(str::trim).filter(|value| !value.is_empty())?;
    if matches!(dimension, QuotaIdentityDimension::Route) {
        return Some(normalized.to_string());
    }
    if is_canonical_hashed_quota_identity(normalized) {
        return Some(normalized.to_string());
    }
    Some(canonical_quota_identity_value(dimension, normalized))
}

fn is_canonical_hashed_quota_identity(value: &str) -> bool {
    value.len() == "sha256:".len() + 64
        && value.starts_with("sha256:")
        && value["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

fn normalize_route_identity(route: Option<&str>) -> Option<String> {
    route
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn compose_quota_key(policy_name: &str, labels: &QuotaIdentityLabels) -> String {
    let labels = labels.canonicalize_for_storage();
    let mut key = String::with_capacity(estimated_quota_key_capacity(policy_name));
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

fn estimated_quota_key_capacity(policy_name: &str) -> usize {
    policy_name
        .len()
        .saturating_add(
            MAX_REQUEST_DERIVED_QUOTA_IDENTITY_BYTES
                .saturating_mul(MAX_REQUEST_DERIVED_QUOTA_IDENTITY_COMPONENTS),
        )
        .saturating_add(64)
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

    use impulse_config::{
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

    use super::*;

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

    fn leak_string(value: String) -> &'static str {
        Box::leak(value.into_boxed_str())
    }

    fn quota_policy_with_selectors(
        tenant: Option<QuotaSelectorKeySpec>,
        token: Option<QuotaSelectorKeySpec>,
        client: Option<QuotaSelectorKeySpec>,
    ) -> QuotaPolicyRuntime {
        QuotaPolicyRuntime {
            name: "tenant-quota".to_string(),
            route_allowlist: HashSet::new(),
            selector: QuotaSelectorMatcher {
                route: true,
                tenant,
                token,
                client,
            },
            burst: Some(QuotaWindowPolicy {
                requests: 10,
                window: Duration::from_secs(1),
            }),
            sustained: None,
        }
    }

    fn assert_selector_identity_invalid(
        policy: &QuotaPolicyRuntime,
        context: &QuotaIdentityContext<'_>,
        dimension: QuotaIdentityDimension,
    ) {
        let err = policy
            .composite_key(context)
            .expect_err("selector identity should be rejected");
        assert_eq!(err.dimension, dimension);
        assert_eq!(err.reason, QuotaDenyReason::SelectorIdentityInvalid);
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
                key_prefix: "impulse:quota".to_string(),
                connect_timeout: Duration::from_millis(250),
                command_timeout: Duration::from_millis(100),
                max_inflight: 64,
            },
            local_fallback: None,
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
        let resilience = Resilience {
            quota: RawQuotaPolicyConfig {
                enabled: true,
                enforcement: RawQuotaEnforcementMode::Enforce,
                backend_failure_policy: RawQuotaBackendFailurePolicy::FailClosed,
                backend: RawQuotaCounterBackend::InMemory {
                    key_prefix: "impulse:quota".to_string(),
                },
                local_fallback: None,
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
            },
            ..Resilience::default()
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
            runtime.policies[0]
                .burst
                .as_ref()
                .map(|window| window.requests),
            Some(25)
        );
        assert!(
            runtime
                .in_memory_store()
                .expect("in-memory quota backend should build")
                .is_some()
        );
        assert!(
            runtime
                .redis_store()
                .expect("in-memory quota backend should not fail redis lookup")
                .is_none()
        );
        let _backend = runtime
            .distributed_store()
            .expect("quota runtime should build generic backend");
    }

    fn sample_counter_request(
        policy_name: &str,
        composite_key: &str,
    ) -> QuotaCounterEvaluationRequest {
        QuotaCounterEvaluationRequest {
            policy_name: policy_name.to_string(),
            composite_key: QuotaCompositeKey {
                policy_name: policy_name.to_string(),
                key: composite_key.to_string(),
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
            },
            burst: Some(QuotaWindowPolicy {
                requests: 2,
                window: Duration::from_secs(1),
            }),
            sustained: Some(QuotaWindowPolicy {
                requests: 5,
                window: Duration::from_secs(60),
            }),
            cost: 1,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn degraded_quota_backend_uses_local_fallback_for_outage_failures() {
        let primary = Arc::new(UnavailableDistributedQuotaCounterStore::new(
            QuotaCounterBackendError {
                policy_name: None,
                composite_key: None,
                kind: QuotaCounterBackendErrorKind::Timeout,
                detail: Some("redis timed out".to_string()),
            },
        )) as SharedDistributedQuotaCounterBackend;
        let fallback = Arc::new(InMemoryDistributedQuotaCounterStore::bounded(
            "impulse:quota:fallback",
            16,
        ));
        let backend = DegradedQuotaCounterBackend::new(primary, "redis", fallback);

        let outcome = backend
            .evaluate(sample_counter_request(
                "tenant-quota",
                "policy=12:tenant-quota|route=3:api|tenant=4:acme|",
            ))
            .await
            .expect("timeout failures should degrade into local fallback evaluation");

        assert_eq!(outcome.decision, QuotaCounterEvaluationDecision::Allowed);
        assert_eq!(
            outcome.backend_metadata.backend_kind,
            "redis_local_fallback_backend_timeout"
        );
        assert_eq!(
            outcome.backend_metadata.protocol_version,
            format!(
                "{}+{}",
                LOCAL_FALLBACK_PROTOCOL_VERSION,
                memory::IN_MEMORY_QUOTA_PROTOCOL_VERSION
            )
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn degraded_quota_backend_does_not_fallback_for_non_outage_errors() {
        let primary = Arc::new(UnavailableDistributedQuotaCounterStore::new(
            QuotaCounterBackendError {
                policy_name: None,
                composite_key: None,
                kind: QuotaCounterBackendErrorKind::Error,
                detail: Some("redis script protocol mismatch".to_string()),
            },
        )) as SharedDistributedQuotaCounterBackend;
        let fallback = Arc::new(InMemoryDistributedQuotaCounterStore::bounded(
            "impulse:quota:fallback",
            16,
        ));
        let backend = DegradedQuotaCounterBackend::new(primary, "redis", fallback);

        let err = backend
            .evaluate(sample_counter_request(
                "tenant-quota",
                "policy=12:tenant-quota|route=3:api|tenant=4:acme|",
            ))
            .await
            .expect_err("non-outage backend errors must not fall back locally");

        assert_eq!(err.kind, QuotaCounterBackendErrorKind::Error);
        assert_eq!(
            err.detail.as_deref(),
            Some("redis script protocol mismatch")
        );
    }

    fn runtime_policy_set_with(
        enforcement: RuntimeQuotaEnforcementMode,
        policies: Vec<RuntimeQuotaPolicy>,
    ) -> QuotaRuntime {
        QuotaRuntime::from_runtime_policy_set(&RuntimeQuotaPolicySet {
            enabled: true,
            enforcement,
            backend_failure_policy: RuntimeQuotaBackendFailurePolicy::FailClosed,
            backend: RuntimeQuotaCounterBackend::InMemory {
                key_prefix: "impulse:quota:test".to_string(),
            },
            local_fallback: None,
            policies,
        })
    }

    fn route_only_policy(name: &str, requests: u64) -> RuntimeQuotaPolicy {
        RuntimeQuotaPolicy {
            name: name.to_string(),
            route_allowlist: vec!["api".to_string()],
            selector: RuntimeQuotaSelectorMatcher {
                route: true,
                tenant: None,
                token: None,
                client: None,
            },
            burst: Some(RuntimeQuotaWindow {
                requests,
                window: Duration::from_secs(1),
            }),
            sustained: None,
        }
    }

    fn tenant_policy(name: &str, requests: u64) -> RuntimeQuotaPolicy {
        RuntimeQuotaPolicy {
            name: name.to_string(),
            route_allowlist: vec!["api".to_string()],
            selector: RuntimeQuotaSelectorMatcher {
                route: true,
                tenant: Some(RuntimeRequestKeySpec::Header("x-tenant-id".to_string())),
                token: None,
                client: None,
            },
            burst: Some(RuntimeQuotaWindow {
                requests,
                window: Duration::from_secs(1),
            }),
            sustained: None,
        }
    }

    /// A narrow policy layered on the same route as a broad one must still be
    /// enforced. Evaluating only the first route match let the broad policy
    /// mask the narrow one entirely, so a caller could exceed the narrow
    /// contract — or skip it by omitting its selector header — while the
    /// config validated and appeared in the control API.
    #[tokio::test(flavor = "current_thread")]
    async fn every_route_matching_policy_is_evaluated_not_just_the_first() {
        // Broad policy first, narrow tenant policy second: the ordering that
        // previously left the tenant contract dead.
        let runtime = runtime_policy_set_with(
            RuntimeQuotaEnforcementMode::Enforce,
            vec![route_only_policy("broad", 100), tenant_policy("narrow", 2)],
        );
        let backend = InMemoryDistributedQuotaCounterStore::new("impulse:quota:test");

        let context = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1/items",
            Some("api.example.com"),
            None,
            Some("203.0.113.10:443".parse().expect("client addr")),
            HashMap::from([("x-tenant-id".to_string(), "acme".to_string())]),
        );

        // The narrow policy allows 2; the broad one allows 100.
        for attempt in 1..=2 {
            assert!(
                matches!(
                    evaluate_admission_quota(&runtime, &backend, &context).await,
                    QuotaDecision::Allowed(_)
                ),
                "request {attempt} is within both contracts",
            );
        }

        let decision = evaluate_admission_quota(&runtime, &backend, &context).await;
        let QuotaDecision::Denied(denial) = decision else {
            panic!("third request must be denied by the narrow policy, got {decision:?}");
        };
        assert_eq!(denial.policy_name, "narrow");
        assert_eq!(denial.reason, QuotaDenyReason::BurstQuotaExhausted);
    }

    /// A request missing a policy's selector identity must be denied rather
    /// than admitted by an earlier, broader policy that does not need it.
    #[tokio::test(flavor = "current_thread")]
    async fn missing_selector_identity_denies_even_when_a_broader_policy_allows() {
        let runtime = runtime_policy_set_with(
            RuntimeQuotaEnforcementMode::Enforce,
            vec![route_only_policy("broad", 100), tenant_policy("narrow", 10)],
        );
        let backend = InMemoryDistributedQuotaCounterStore::new("impulse:quota:test");

        // No x-tenant-id header at all.
        let context = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1/items",
            Some("api.example.com"),
            None,
            Some("203.0.113.10:443".parse().expect("client addr")),
            HashMap::new(),
        );

        let decision = evaluate_admission_quota(&runtime, &backend, &context).await;
        let QuotaDecision::Denied(denial) = decision else {
            panic!("missing tenant identity must not be admitted, got {decision:?}");
        };
        assert_eq!(denial.policy_name, "narrow");
        assert_eq!(denial.reason, QuotaDenyReason::SelectorIdentityMissing);
    }

    /// Shadow mode must not stop at the first would-deny: every matching policy
    /// still needs to record an outcome, otherwise sizing one policy hides the
    /// blast radius of the others.
    #[tokio::test(flavor = "current_thread")]
    async fn shadow_mode_evaluates_all_policies_and_admits() {
        let runtime = runtime_policy_set_with(
            RuntimeQuotaEnforcementMode::Shadow,
            vec![tenant_policy("narrow", 1), route_only_policy("broad", 100)],
        );
        let backend = InMemoryDistributedQuotaCounterStore::new("impulse:quota:test");

        let context = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1/items",
            Some("api.example.com"),
            None,
            Some("203.0.113.10:443".parse().expect("client addr")),
            HashMap::from([("x-tenant-id".to_string(), "acme".to_string())]),
        );

        assert!(matches!(
            evaluate_admission_quota(&runtime, &backend, &context).await,
            QuotaDecision::Allowed(_)
        ));

        // Second request exhausts the narrow policy, but shadow mode reports a
        // would-deny rather than a denial.
        let decision = evaluate_admission_quota(&runtime, &backend, &context).await;
        let QuotaDecision::ShadowDenied(denial) = decision else {
            panic!("shadow mode must report a would-deny, got {decision:?}");
        };
        assert_eq!(denial.policy_name, "narrow");
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
                (
                    "authorization".to_string(),
                    "Bearer secret-token".to_string(),
                ),
                ("x-tenant-id".to_string(), "acme".to_string()),
            ]),
        );

        let composite = policy
            .composite_key(&context)
            .expect("quota identities should resolve");

        let hashed_tenant = composite
            .labels
            .tenant
            .as_ref()
            .expect("hashed tenant label");
        let hashed_token = composite.labels.token.as_ref().expect("hashed token label");
        let hashed_client = composite
            .labels
            .client
            .as_ref()
            .expect("hashed client label");

        assert_eq!(composite.policy_name, "tenant-quota");
        assert_eq!(composite.labels.route.as_deref(), Some("api"));
        assert!(
            hashed_tenant.starts_with("sha256:"),
            "tenant-derived identities must be hashed before reuse"
        );
        assert!(
            hashed_token.starts_with("sha256:"),
            "token-derived identities must be hashed before reuse"
        );
        assert!(
            hashed_client.starts_with("sha256:"),
            "client-derived identities must be hashed before reuse"
        );
        assert_eq!(
            composite.key,
            format!(
                "policy=12:tenant-quota|route=3:api|tenant={}:{}|token={}:{}|client={}:{}|",
                hashed_tenant.len(),
                hashed_tenant,
                hashed_token.len(),
                hashed_token,
                hashed_client.len(),
                hashed_client,
            )
        );
    }

    #[test]
    fn compose_quota_key_canonicalizes_non_route_labels_before_storage() {
        let labels = QuotaIdentityLabels {
            route: Some("api".to_string()),
            tenant: Some("acme".to_string()),
            token: Some("secret-token".to_string()),
            client: Some("203.0.113.10".to_string()),
        };

        let canonical = labels.canonicalize_for_storage();
        assert_eq!(canonical.route.as_deref(), Some("api"));
        assert!(
            canonical
                .tenant
                .as_deref()
                .is_some_and(|value| value.starts_with("sha256:"))
        );
        assert!(
            canonical
                .token
                .as_deref()
                .is_some_and(|value| value.starts_with("sha256:"))
        );
        assert!(
            canonical
                .client
                .as_deref()
                .is_some_and(|value| value.starts_with("sha256:"))
        );

        let key = compose_quota_key("tenant-quota", &labels);
        assert!(key.contains("route=3:api|"));
        assert!(!key.contains("tenant=4:acme|"));
        assert!(!key.contains("token=12:secret-token|"));
        assert!(!key.contains("client=12:203.0.113.10|"));
        assert!(key.contains(canonical.tenant.as_deref().expect("hashed tenant")));
        assert!(key.contains(canonical.token.as_deref().expect("hashed token")));
        assert!(key.contains(canonical.client.as_deref().expect("hashed client")));
    }

    #[test]
    fn route_identity_remains_readable_during_storage_canonicalization() {
        let labels = QuotaIdentityLabels {
            route: Some("payments".to_string()),
            tenant: Some("acme".to_string()),
            token: None,
            client: None,
        };

        let canonical = labels.canonicalize_for_storage();
        assert_eq!(canonical.route.as_deref(), Some("payments"));
        assert_eq!(
            canonical.tenant.as_deref(),
            Some(canonical_quota_identity_value(QuotaIdentityDimension::Tenant, "acme",).as_str())
        );

        let key = compose_quota_key("tenant-quota", &labels);
        assert!(key.contains("route=8:payments|"));
        assert!(!key.contains("tenant=4:acme|"));
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
            HashMap::from([("authorization".to_string(), "Bearer token-1".to_string())]),
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
    fn oversized_bearer_token_is_rejected() {
        let policy =
            quota_policy_with_selectors(None, Some(QuotaSelectorKeySpec::BearerToken), None);
        let oversized_token = "t".repeat(MAX_REQUEST_DERIVED_QUOTA_IDENTITY_BYTES + 1);
        let context = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1",
            Some("api.example.com"),
            None,
            None,
            HashMap::from([(
                "authorization".to_string(),
                format!("Bearer {oversized_token}"),
            )]),
        );

        assert_eq!(
            extract_runtime_request_key(&RuntimeRequestKeySpec::BearerToken, &context),
            RequestKeyExtraction::Invalid
        );
        assert_selector_identity_invalid(&policy, &context, QuotaIdentityDimension::Token);
    }

    #[test]
    fn oversized_header_cookie_query_and_cid_values_are_rejected() {
        let oversized = "a".repeat(MAX_REQUEST_DERIVED_QUOTA_IDENTITY_BYTES + 1);

        let header_policy = quota_policy_with_selectors(
            Some(QuotaSelectorKeySpec::Header("x-tenant-id".to_string())),
            None,
            None,
        );
        let header_context = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1",
            Some("api.example.com"),
            None,
            None,
            HashMap::from([("x-tenant-id".to_string(), oversized.clone())]),
        );
        assert_eq!(
            extract_runtime_request_key(
                &RuntimeRequestKeySpec::Header("x-tenant-id".to_string()),
                &header_context,
            ),
            RequestKeyExtraction::Invalid
        );
        assert_selector_identity_invalid(
            &header_policy,
            &header_context,
            QuotaIdentityDimension::Tenant,
        );

        let cookie_policy = quota_policy_with_selectors(
            Some(QuotaSelectorKeySpec::Cookie("session".to_string())),
            None,
            None,
        );
        let cookie_context = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1",
            Some("api.example.com"),
            None,
            None,
            HashMap::from([(
                "cookie".to_string(),
                format!("session={oversized}; theme=dark"),
            )]),
        );
        assert_eq!(
            extract_runtime_request_key(
                &RuntimeRequestKeySpec::Cookie("session".to_string()),
                &cookie_context,
            ),
            RequestKeyExtraction::Invalid
        );
        assert_selector_identity_invalid(
            &cookie_policy,
            &cookie_context,
            QuotaIdentityDimension::Tenant,
        );

        let query_policy = quota_policy_with_selectors(
            Some(QuotaSelectorKeySpec::Query("tenant".to_string())),
            None,
            None,
        );
        let query_context = identity_context_with_headers(
            Some("api"),
            "GET",
            leak_string(format!("/v1?tenant={oversized}")),
            Some("api.example.com"),
            None,
            None,
            HashMap::new(),
        );
        assert_eq!(
            extract_runtime_request_key(
                &RuntimeRequestKeySpec::Query("tenant".to_string()),
                &query_context,
            ),
            RequestKeyExtraction::Invalid
        );
        assert_selector_identity_invalid(
            &query_policy,
            &query_context,
            QuotaIdentityDimension::Tenant,
        );

        let cid_policy = quota_policy_with_selectors(Some(QuotaSelectorKeySpec::Cid), None, None);
        let cid_context = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1",
            Some("api.example.com"),
            Some(leak_string(oversized)),
            None,
            HashMap::new(),
        );
        assert_eq!(
            extract_runtime_request_key(&RuntimeRequestKeySpec::Cid, &cid_context),
            RequestKeyExtraction::Invalid
        );
        assert_selector_identity_invalid(&cid_policy, &cid_context, QuotaIdentityDimension::Tenant);
    }

    #[test]
    fn legacy_fallback_does_not_mask_invalid_selector_values() {
        let policy = QuotaPolicyRuntime {
            name: "tenant-quota".to_string(),
            route_allowlist: HashSet::new(),
            selector: QuotaSelectorMatcher {
                route: true,
                tenant: Some(
                    QuotaSelectorKeySpec::Header("x-tenant-id".to_string())
                        .with_legacy_default_fallback(),
                ),
                token: None,
                client: None,
            },
            burst: Some(QuotaWindowPolicy {
                requests: 10,
                window: Duration::from_secs(1),
            }),
            sustained: None,
        };

        let oversized_header = "a".repeat(MAX_REQUEST_DERIVED_QUOTA_IDENTITY_BYTES + 1);
        let context = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1",
            Some("api.example.com"),
            None,
            None,
            HashMap::from([("x-tenant-id".to_string(), oversized_header)]),
        );

        let err = policy
            .composite_key(&context)
            .expect_err("oversized selector values must stay invalid");
        assert_eq!(err.dimension, QuotaIdentityDimension::Tenant);
        assert_eq!(err.reason, QuotaDenyReason::SelectorIdentityInvalid);
    }

    #[test]
    fn distinct_oversized_selector_values_do_not_collide_via_truncation() {
        let policy = quota_policy_with_selectors(
            Some(QuotaSelectorKeySpec::Header("x-tenant-id".to_string())),
            None,
            None,
        );
        let prefix = "x".repeat(MAX_REQUEST_DERIVED_QUOTA_IDENTITY_BYTES);
        let first = format!("{prefix}a");
        let second = format!("{prefix}b");

        let first_context = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1",
            Some("api.example.com"),
            None,
            None,
            HashMap::from([("x-tenant-id".to_string(), first)]),
        );
        let second_context = identity_context_with_headers(
            Some("api"),
            "GET",
            "/v1",
            Some("api.example.com"),
            None,
            None,
            HashMap::from([("x-tenant-id".to_string(), second)]),
        );

        assert_eq!(
            extract_runtime_request_key(
                &RuntimeRequestKeySpec::Header("x-tenant-id".to_string()),
                &first_context,
            ),
            RequestKeyExtraction::Invalid
        );
        assert_eq!(
            extract_runtime_request_key(
                &RuntimeRequestKeySpec::Header("x-tenant-id".to_string()),
                &second_context,
            ),
            RequestKeyExtraction::Invalid
        );
        assert_selector_identity_invalid(&policy, &first_context, QuotaIdentityDimension::Tenant);
        assert_selector_identity_invalid(&policy, &second_context, QuotaIdentityDimension::Tenant);
    }

    #[test]
    fn quota_policy_route_allowlist_scopes_composite_selector_matching() {
        let policy = QuotaPolicyRuntime {
            name: "tenant-quota".to_string(),
            route_allowlist: HashSet::from(["payments".to_string()]),
            selector: QuotaSelectorMatcher {
                route: true,
                tenant: Some(QuotaSelectorKeySpec::Header("x-tenant-id".to_string())),
                token: None,
                client: Some(QuotaSelectorKeySpec::ClientIp),
            },
            burst: Some(QuotaWindowPolicy {
                requests: 100,
                window: Duration::from_secs(1),
            }),
            sustained: None,
        };

        assert!(policy.applies_to_route("payments"));
        assert!(!policy.applies_to_route("api"));

        let context = identity_context_with_headers(
            Some("payments"),
            "POST",
            "/v1/payments",
            Some("api.example.com"),
            None,
            Some("203.0.113.10:443".parse().expect("client addr")),
            HashMap::from([("x-tenant-id".to_string(), "acme".to_string())]),
        );
        let composite = policy
            .composite_key(&context)
            .expect("matching route should still build composite keys");

        assert_eq!(composite.labels.route.as_deref(), Some("payments"));
        assert!(
            composite
                .labels
                .tenant
                .as_deref()
                .is_some_and(|value| value.starts_with("sha256:"))
        );
        assert!(
            composite
                .labels
                .client
                .as_deref()
                .is_some_and(|value| value.starts_with("sha256:"))
        );
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
            decision: QuotaCounterEvaluationDecision::Denied(QuotaDenyReason::BurstQuotaExhausted),
            counter: QuotaCounterResult {
                burst: Some(QuotaWindowUsage {
                    limit: 50,
                    consumed: 50,
                    remaining: 0,
                    window: Duration::from_secs(1),
                    reset_after: Some(Duration::from_millis(750)),
                    bucket_started_at_unix_ms: Some(1_700_000_000_000),
                    reset_at_unix_ms: Some(1_700_000_001_000),
                    storage_key: Some(
                        "impulse:quota:qv1:12:tenant-quota:burst:1000:1700000000000:abc"
                            .to_string(),
                    ),
                }),
                sustained: Some(QuotaWindowUsage {
                    limit: 500,
                    consumed: 320,
                    remaining: 180,
                    window: Duration::from_secs(60),
                    reset_after: Some(Duration::from_secs(12)),
                    bucket_started_at_unix_ms: Some(1_699_999_980_000),
                    reset_at_unix_ms: Some(1_700_000_040_000),
                    storage_key: Some(
                        "impulse:quota:qv1:12:tenant-quota:sustained:60000:1699999980000:def"
                            .to_string(),
                    ),
                }),
            },
            backend_metadata: QuotaCounterBackendMetadata {
                backend_kind: "redis".to_string(),
                protocol_version: REDIS_QUOTA_PROTOCOL_VERSION.to_string(),
                evaluated_at_unix_ms: Some(1_700_000_000_250),
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
            outcome
                .counter
                .sustained
                .as_ref()
                .map(|window| window.remaining),
            Some(180)
        );
    }
}
