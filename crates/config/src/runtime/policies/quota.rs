use std::{collections::HashSet, time::Duration};

use super::{
    RuntimeRequestKeySpec, config_invalid, normalize_optional_string,
};
use crate::{
    config::{
        DistributedQuotaPolicy, DistributedQuotaSelector, DistributedQuotaSelectorSource,
        DistributedQuotaWindow, QuotaCounterBackend, QuotaEnforcementMode, QuotaPolicyConfig,
        Resilience,
    },
    runtime::RuntimeConfigError,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RuntimeQuotaEnforcementMode {
    Shadow,
    #[default]
    Enforce,
}

impl From<QuotaEnforcementMode> for RuntimeQuotaEnforcementMode {
    fn from(value: QuotaEnforcementMode) -> Self {
        match value {
            QuotaEnforcementMode::Shadow => Self::Shadow,
            QuotaEnforcementMode::Enforce => Self::Enforce,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RuntimeQuotaBackendFailurePolicy {
    FailOpen,
    #[default]
    FailClosed,
}

impl From<crate::config::QuotaBackendFailurePolicy> for RuntimeQuotaBackendFailurePolicy {
    fn from(value: crate::config::QuotaBackendFailurePolicy) -> Self {
        match value {
            crate::config::QuotaBackendFailurePolicy::FailOpen => Self::FailOpen,
            crate::config::QuotaBackendFailurePolicy::FailClosed => Self::FailClosed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeQuotaCounterBackend {
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

impl RuntimeQuotaCounterBackend {
    fn normalize(config: &QuotaCounterBackend) -> Result<Self, RuntimeConfigError> {
        match config {
            QuotaCounterBackend::InMemory { key_prefix } => {
                let key_prefix = key_prefix.trim();
                if key_prefix.is_empty() {
                    return Err(config_invalid(
                        "resilience.quota.backend.key_prefix must be non-empty for kind=in_memory",
                    ));
                }
                Ok(Self::InMemory {
                    key_prefix: key_prefix.to_string(),
                })
            }
            QuotaCounterBackend::Redis {
                url,
                key_prefix,
                connect_timeout_ms,
                command_timeout_ms,
                max_inflight,
            } => {
                let url = url.trim();
                let key_prefix = key_prefix.trim();
                if url.is_empty() {
                    return Err(config_invalid(
                        "resilience.quota.backend.url must be non-empty for kind=redis",
                    ));
                }
                if key_prefix.is_empty() {
                    return Err(config_invalid(
                        "resilience.quota.backend.key_prefix must be non-empty for kind=redis",
                    ));
                }
                if *connect_timeout_ms == 0 {
                    return Err(config_invalid(
                        "resilience.quota.backend.connect_timeout_ms must be greater than 0 for kind=redis",
                    ));
                }
                if *command_timeout_ms == 0 {
                    return Err(config_invalid(
                        "resilience.quota.backend.command_timeout_ms must be greater than 0 for kind=redis",
                    ));
                }
                if *max_inflight == 0 {
                    return Err(config_invalid(
                        "resilience.quota.backend.max_inflight must be greater than 0 for kind=redis",
                    ));
                }

                Ok(Self::Redis {
                    url: url.to_string(),
                    key_prefix: key_prefix.to_string(),
                    connect_timeout: Duration::from_millis(*connect_timeout_ms),
                    command_timeout: Duration::from_millis(*command_timeout_ms),
                    max_inflight: *max_inflight,
                })
            }
        }
    }
}

impl Default for RuntimeQuotaCounterBackend {
    fn default() -> Self {
        Self::InMemory {
            key_prefix: "spooky:quota".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeQuotaSelectorDimension {
    Tenant,
    Token,
    Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeQuotaSelectorMatcher {
    pub route: bool,
    pub tenant: Option<RuntimeRequestKeySpec>,
    pub token: Option<RuntimeRequestKeySpec>,
    pub client: Option<RuntimeRequestKeySpec>,
}

impl RuntimeQuotaSelectorMatcher {
    fn normalize(
        selector: &DistributedQuotaSelector,
        policy_name: &str,
    ) -> Result<Self, RuntimeConfigError> {
        let tenant = normalize_selector_source(selector.tenant.as_ref(), policy_name, "tenant")?;
        let token = normalize_selector_source(selector.token.as_ref(), policy_name, "token")?;
        let client = normalize_selector_source(selector.client.as_ref(), policy_name, "client")?;

        if !selector.route && tenant.is_none() && token.is_none() && client.is_none() {
            return Err(config_invalid(format!(
                "resilience.quota.policies['{policy_name}'].selector must include at least one dimension",
            )));
        }

        let mut seen_specs = HashSet::new();
        for (dimension, spec) in [
            (RuntimeQuotaSelectorDimension::Tenant, tenant.as_ref()),
            (RuntimeQuotaSelectorDimension::Token, token.as_ref()),
            (RuntimeQuotaSelectorDimension::Client, client.as_ref()),
        ] {
            let Some(spec) = spec else {
                continue;
            };
            if !seen_specs.insert(spec.clone()) {
                return Err(config_invalid(format!(
                    "resilience.quota.policies['{policy_name}'].selector reuses the same request key across multiple identity dimensions",
                )));
            }
            validate_dimension_key_kind(policy_name, dimension, spec)?;
        }

        Ok(Self {
            route: selector.route,
            tenant,
            token,
            client,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeQuotaWindow {
    pub requests: u64,
    pub window: Duration,
}

impl RuntimeQuotaWindow {
    fn normalize(
        window: &DistributedQuotaWindow,
        field_path: &str,
    ) -> Result<Self, RuntimeConfigError> {
        if window.requests == 0 {
            return Err(config_invalid(format!(
                "{field_path}.requests must be greater than 0",
            )));
        }
        if window.window_secs == 0 {
            return Err(config_invalid(format!(
                "{field_path}.window_secs must be greater than 0",
            )));
        }

        Ok(Self {
            requests: window.requests,
            window: Duration::from_secs(window.window_secs),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeQuotaPolicy {
    pub name: String,
    pub route_allowlist: Vec<String>,
    pub selector: RuntimeQuotaSelectorMatcher,
    pub burst: Option<RuntimeQuotaWindow>,
    pub sustained: Option<RuntimeQuotaWindow>,
}

impl RuntimeQuotaPolicy {
    fn normalize(policy: &DistributedQuotaPolicy) -> Result<Self, RuntimeConfigError> {
        let policy_name = policy.name.trim();
        if policy_name.is_empty() {
            return Err(config_invalid(
                "resilience.quota.policies[].name must be non-empty",
            ));
        }

        let route_allowlist = policy
            .route_allowlist
            .iter()
            .map(|route| route.trim())
            .filter(|route| !route.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if route_allowlist.len() != policy.route_allowlist.len() {
            return Err(config_invalid(format!(
                "resilience.quota.policies['{policy_name}'].route_allowlist must not contain empty values",
            )));
        }

        let selector = RuntimeQuotaSelectorMatcher::normalize(&policy.selector, policy_name)?;
        let burst = policy
            .burst
            .as_ref()
            .map(|window| {
                RuntimeQuotaWindow::normalize(
                    window,
                    &format!("resilience.quota.policies['{policy_name}'].burst"),
                )
            })
            .transpose()?;
        let sustained = policy
            .sustained
            .as_ref()
            .map(|window| {
                RuntimeQuotaWindow::normalize(
                    window,
                    &format!("resilience.quota.policies['{policy_name}'].sustained"),
                )
            })
            .transpose()?;

        if burst.is_none() && sustained.is_none() {
            return Err(config_invalid(format!(
                "resilience.quota.policies['{policy_name}'] must define at least one of burst or sustained",
            )));
        }

        if let (Some(burst), Some(sustained)) = (&burst, &sustained)
            && burst.window >= sustained.window
        {
            return Err(config_invalid(format!(
                "resilience.quota.policies['{policy_name}'].burst.window_secs must be less than sustained.window_secs",
            )));
        }

        Ok(Self {
            name: policy_name.to_string(),
            route_allowlist,
            selector,
            burst,
            sustained,
        })
    }

    fn matcher_fingerprint(&self) -> RuntimeQuotaPolicyFingerprint {
        let mut route_allowlist = self.route_allowlist.clone();
        route_allowlist.sort();

        RuntimeQuotaPolicyFingerprint {
            route_allowlist,
            route: self.selector.route,
            tenant: self.selector.tenant.clone(),
            token: self.selector.token.clone(),
            client: self.selector.client.clone(),
            burst: self
                .burst
                .as_ref()
                .map(|window| (window.requests, window.window.as_secs())),
            sustained: self
                .sustained
                .as_ref()
                .map(|window| (window.requests, window.window.as_secs())),
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RuntimeQuotaPolicyFingerprint {
    route_allowlist: Vec<String>,
    route: bool,
    tenant: Option<RuntimeRequestKeySpec>,
    token: Option<RuntimeRequestKeySpec>,
    client: Option<RuntimeRequestKeySpec>,
    burst: Option<(u64, u64)>,
    sustained: Option<(u64, u64)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeQuotaPolicySet {
    pub enabled: bool,
    pub enforcement: RuntimeQuotaEnforcementMode,
    pub backend_failure_policy: RuntimeQuotaBackendFailurePolicy,
    pub backend: RuntimeQuotaCounterBackend,
    pub policies: Vec<RuntimeQuotaPolicy>,
}

impl RuntimeQuotaPolicySet {
    pub(crate) fn normalize(resilience: &Resilience) -> Result<Self, RuntimeConfigError> {
        Self::normalize_config(&resilience.quota)
    }

    fn normalize_config(config: &QuotaPolicyConfig) -> Result<Self, RuntimeConfigError> {
        let backend = RuntimeQuotaCounterBackend::normalize(&config.backend)?;
        let mut seen_names = HashSet::new();
        let mut seen_matchers = HashSet::new();
        let mut policies = Vec::with_capacity(config.policies.len());

        for policy in &config.policies {
            let normalized = RuntimeQuotaPolicy::normalize(policy)?;
            if !seen_names.insert(normalized.name.clone()) {
                return Err(config_invalid(format!(
                    "resilience.quota.policies contains duplicate policy name '{}'",
                    normalized.name
                )));
            }

            let fingerprint = normalized.matcher_fingerprint();
            if !seen_matchers.insert(fingerprint) {
                return Err(config_invalid(format!(
                    "resilience.quota.policies contains duplicate selector/window contract '{}'",
                    normalized.name
                )));
            }

            policies.push(normalized);
        }

        if config.enabled && policies.is_empty() {
            return Err(config_invalid(
                "resilience.quota.policies must not be empty when quota is enabled",
            ));
        }

        Ok(Self {
            enabled: config.enabled,
            enforcement: config.enforcement.into(),
            backend_failure_policy: config.backend_failure_policy.into(),
            backend,
            policies,
        })
    }
}

fn normalize_selector_source(
    source: Option<&DistributedQuotaSelectorSource>,
    policy_name: &str,
    field_name: &str,
) -> Result<Option<RuntimeRequestKeySpec>, RuntimeConfigError> {
    let Some(source) = source else {
        return Ok(None);
    };

    let Some(key) = normalize_optional_string(Some(source.key.as_str())) else {
        return Err(config_invalid(format!(
            "resilience.quota.policies['{policy_name}'].selector.{field_name}.key must be non-empty",
        )));
    };

    RuntimeRequestKeySpec::normalize(&key).map(Some).map_err(|_| {
        config_invalid(format!(
            "resilience.quota.policies['{policy_name}'].selector.{field_name}.key must be a supported request key spec",
        ))
    })
}

fn validate_dimension_key_kind(
    policy_name: &str,
    dimension: RuntimeQuotaSelectorDimension,
    spec: &RuntimeRequestKeySpec,
) -> Result<(), RuntimeConfigError> {
    let supported = match dimension {
        RuntimeQuotaSelectorDimension::Tenant => matches!(
            spec,
            RuntimeRequestKeySpec::Header(_)
                | RuntimeRequestKeySpec::Cookie(_)
                | RuntimeRequestKeySpec::Query(_)
                | RuntimeRequestKeySpec::BearerToken
        ),
        RuntimeQuotaSelectorDimension::Token => matches!(
            spec,
            RuntimeRequestKeySpec::Header(_)
                | RuntimeRequestKeySpec::Cookie(_)
                | RuntimeRequestKeySpec::Query(_)
                | RuntimeRequestKeySpec::BearerToken
        ),
        RuntimeQuotaSelectorDimension::Client => matches!(
            spec,
            RuntimeRequestKeySpec::Header(_)
                | RuntimeRequestKeySpec::Cookie(_)
                | RuntimeRequestKeySpec::Query(_)
                | RuntimeRequestKeySpec::PeerIp
                | RuntimeRequestKeySpec::ClientIp
        ),
    };

    if supported {
        return Ok(());
    }

    Err(config_invalid(format!(
        "resilience.quota.policies['{policy_name}'].selector.{} key '{}' is not valid for that identity dimension",
        match dimension {
            RuntimeQuotaSelectorDimension::Tenant => "tenant",
            RuntimeQuotaSelectorDimension::Token => "token",
            RuntimeQuotaSelectorDimension::Client => "client",
        },
        runtime_request_key_spec_label(spec),
    )))
}

fn runtime_request_key_spec_label(spec: &RuntimeRequestKeySpec) -> String {
    match spec {
        RuntimeRequestKeySpec::Path => "path".to_string(),
        RuntimeRequestKeySpec::Authority => "authority".to_string(),
        RuntimeRequestKeySpec::Method => "method".to_string(),
        RuntimeRequestKeySpec::Cid => "cid".to_string(),
        RuntimeRequestKeySpec::StickyCid => "sticky-cid".to_string(),
        RuntimeRequestKeySpec::PeerIp => "peer_ip".to_string(),
        RuntimeRequestKeySpec::ClientIp => "client_ip".to_string(),
        RuntimeRequestKeySpec::BearerToken => "bearer_token".to_string(),
        RuntimeRequestKeySpec::Header(name) => format!("header:{name}"),
        RuntimeRequestKeySpec::Cookie(name) => format!("cookie:{name}"),
        RuntimeRequestKeySpec::Query(name) => format!("query:{name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        QuotaBackendFailurePolicy, QuotaCounterBackend, QuotaEnforcementMode, QuotaPolicyConfig,
        Resilience,
    };

    fn assert_config_invalid(err: RuntimeConfigError, expected: impl AsRef<str>) {
        let expected = expected.as_ref();
        assert_eq!(err.category(), "config_invalid");
        assert_eq!(err.to_string(), format!("config_invalid: {expected}"));
    }

    fn valid_quota_policy() -> DistributedQuotaPolicy {
        DistributedQuotaPolicy {
            name: "tenant-contract".to_string(),
            route_allowlist: vec!["payments".to_string()],
            selector: DistributedQuotaSelector {
                route: true,
                tenant: Some(DistributedQuotaSelectorSource {
                    key: "header:x-tenant-id".to_string(),
                }),
                token: None,
                client: Some(DistributedQuotaSelectorSource {
                    key: "client_ip".to_string(),
                }),
            },
            burst: Some(DistributedQuotaWindow {
                requests: 100,
                window_secs: 1,
            }),
            sustained: Some(DistributedQuotaWindow {
                requests: 5000,
                window_secs: 60,
            }),
        }
    }

    #[test]
    fn quota_policy_normalization_shapes_runtime_selector_backend_and_windows() {
        let mut resilience = Resilience::default();
        resilience.quota = QuotaPolicyConfig {
            enabled: true,
            enforcement: QuotaEnforcementMode::Shadow,
            backend_failure_policy: QuotaBackendFailurePolicy::FailOpen,
            backend: QuotaCounterBackend::Redis {
                url: " redis://127.0.0.1:6379/0 ".to_string(),
                key_prefix: " spooky:quota ".to_string(),
                connect_timeout_ms: 250,
                command_timeout_ms: 100,
                max_inflight: 128,
            },
            policies: vec![valid_quota_policy()],
        };

        let runtime = RuntimeQuotaPolicySet::normalize(&resilience).expect("quota policy");

        assert!(runtime.enabled);
        assert_eq!(runtime.enforcement, RuntimeQuotaEnforcementMode::Shadow);
        assert_eq!(
            runtime.backend_failure_policy,
            RuntimeQuotaBackendFailurePolicy::FailOpen
        );
        match runtime.backend {
            RuntimeQuotaCounterBackend::Redis {
                ref url,
                ref key_prefix,
                connect_timeout,
                command_timeout,
                max_inflight,
            } => {
                assert_eq!(url, "redis://127.0.0.1:6379/0");
                assert_eq!(key_prefix, "spooky:quota");
                assert_eq!(connect_timeout, Duration::from_millis(250));
                assert_eq!(command_timeout, Duration::from_millis(100));
                assert_eq!(max_inflight, 128);
            }
            RuntimeQuotaCounterBackend::InMemory { .. } => panic!("expected redis backend"),
        }

        let policy = runtime.policies.first().expect("normalized policy");
        assert_eq!(policy.name, "tenant-contract");
        assert_eq!(policy.route_allowlist, vec!["payments".to_string()]);
        assert!(policy.selector.route);
        assert_eq!(
            policy.selector.tenant,
            Some(RuntimeRequestKeySpec::Header("x-tenant-id".to_string()))
        );
        assert_eq!(policy.selector.client, Some(RuntimeRequestKeySpec::ClientIp));
        assert_eq!(
            policy.burst.as_ref().map(|window| window.window),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            policy.sustained.as_ref().map(|window| window.window),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn quota_policy_normalization_rejects_duplicate_names_and_matchers() {
        let mut resilience = Resilience::default();
        let first = valid_quota_policy();
        let second = valid_quota_policy();
        resilience.quota = QuotaPolicyConfig {
            enabled: true,
            policies: vec![first.clone(), second],
            ..QuotaPolicyConfig::default()
        };

        let err = RuntimeQuotaPolicySet::normalize(&resilience).expect_err("duplicate names");
        assert_config_invalid(
            err,
            "resilience.quota.policies contains duplicate policy name 'tenant-contract'",
        );

        let mut third = valid_quota_policy();
        third.name = "other-name".to_string();
        resilience.quota = QuotaPolicyConfig {
            enabled: true,
            policies: vec![first, third],
            ..QuotaPolicyConfig::default()
        };

        let err = RuntimeQuotaPolicySet::normalize(&resilience).expect_err("duplicate matcher");
        assert_config_invalid(
            err,
            "resilience.quota.policies contains duplicate selector/window contract 'other-name'",
        );
    }

    #[test]
    fn quota_policy_normalization_rejects_invalid_identity_keys_and_window_order() {
        let mut invalid_tenant = valid_quota_policy();
        invalid_tenant.selector.tenant = Some(DistributedQuotaSelectorSource {
            key: "client_ip".to_string(),
        });
        let err = RuntimeQuotaPolicy::normalize(&invalid_tenant)
            .expect_err("tenant client_ip must fail");
        assert_config_invalid(
            err,
            "resilience.quota.policies['tenant-contract'].selector.tenant key 'client_ip' is not valid for that identity dimension",
        );

        let mut duplicate_key = valid_quota_policy();
        duplicate_key.selector.token = Some(DistributedQuotaSelectorSource {
            key: "header:x-tenant-id".to_string(),
        });
        let err = RuntimeQuotaPolicy::normalize(&duplicate_key)
            .expect_err("reused selector key must fail");
        assert_config_invalid(
            err,
            "resilience.quota.policies['tenant-contract'].selector reuses the same request key across multiple identity dimensions",
        );

        let mut bad_window_order = valid_quota_policy();
        bad_window_order.burst = Some(DistributedQuotaWindow {
            requests: 100,
            window_secs: 60,
        });
        bad_window_order.sustained = Some(DistributedQuotaWindow {
            requests: 5000,
            window_secs: 60,
        });
        let err = RuntimeQuotaPolicy::normalize(&bad_window_order)
            .expect_err("burst >= sustained window must fail");
        assert_config_invalid(
            err,
            "resilience.quota.policies['tenant-contract'].burst.window_secs must be less than sustained.window_secs",
        );
    }

    #[test]
    fn quota_policy_normalization_rejects_conflicting_backend_settings() {
        let mut resilience = Resilience::default();
        resilience.quota = QuotaPolicyConfig {
            enabled: true,
            backend: QuotaCounterBackend::Redis {
                url: "redis://127.0.0.1:6379/0".to_string(),
                key_prefix: "spooky:quota".to_string(),
                connect_timeout_ms: 250,
                command_timeout_ms: 0,
                max_inflight: 128,
            },
            policies: vec![valid_quota_policy()],
            ..QuotaPolicyConfig::default()
        };

        let err = RuntimeQuotaPolicySet::normalize(&resilience)
            .expect_err("zero command timeout must fail");
        assert_config_invalid(
            err,
            "resilience.quota.backend.command_timeout_ms must be greater than 0 for kind=redis",
        );
    }
}
