use std::{collections::HashSet, time::Duration};

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

#[cfg(test)]
mod tests {
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
}
