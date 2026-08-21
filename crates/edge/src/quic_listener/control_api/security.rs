use std::{net::IpAddr, sync::Arc};

use impulse_config::config::{
    ControlApi as ControlApiConfig, ControlApiBearerToken, ControlApiClientAuthMode,
    ControlApiIdentitySource, ControlApiRole,
};

use super::audit::ControlApiAdminAuditEmitter;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiSecurityPolicy {
    pub(in crate::quic_listener) client_auth: ControlApiClientAuthPolicy,
    pub(in crate::quic_listener) bearer_tokens: Arc<Vec<ControlApiBearerTokenEntry>>,
    pub(in crate::quic_listener) identity_source: Option<ControlApiIdentitySourcePolicy>,
    pub(in crate::quic_listener) authorization: ControlApiAuthorizationPolicy,
    pub(in crate::quic_listener) ip_allowlist: ControlApiIpAllowlistPolicy,
    pub(in crate::quic_listener) audit: ControlApiAdminAuditEmitter,
}

impl ControlApiSecurityPolicy {
    pub(in crate::quic_listener) fn from_config(config: &ControlApiConfig) -> Self {
        Self {
            client_auth: ControlApiClientAuthPolicy::from_config(config),
            bearer_tokens: Arc::new(runtime_bearer_tokens(config)),
            identity_source: config
                .auth
                .identity_source
                .as_ref()
                .map(ControlApiIdentitySourcePolicy::from_config),
            authorization: ControlApiAuthorizationPolicy::from_config(config),
            ip_allowlist: ControlApiIpAllowlistPolicy::from_config(config),
            audit: ControlApiAdminAuditEmitter::from_config(config),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiClientAuthPolicy {
    pub(in crate::quic_listener) mode: ControlApiClientAuthMode,
    pub(in crate::quic_listener) verifier: ControlApiClientVerifierState,
}

impl ControlApiClientAuthPolicy {
    fn from_config(config: &ControlApiConfig) -> Self {
        let client_auth = &config.tls.client_auth;
        Self {
            mode: client_auth.mode,
            verifier: if matches!(client_auth.mode, ControlApiClientAuthMode::Disabled) {
                ControlApiClientVerifierState::Disabled
            } else {
                ControlApiClientVerifierState::Configured(ControlApiClientCaMaterial {
                    ca_file: client_auth.ca_file.clone(),
                    ca_dir: client_auth.ca_dir.clone(),
                })
            },
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) enum ControlApiClientVerifierState {
    Disabled,
    Configured(ControlApiClientCaMaterial),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiClientCaMaterial {
    pub(in crate::quic_listener) ca_file: Option<String>,
    pub(in crate::quic_listener) ca_dir: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiBearerTokenEntry {
    pub(in crate::quic_listener) token: String,
    pub(in crate::quic_listener) role: ControlApiRole,
    pub(in crate::quic_listener) actor_id: Option<String>,
    pub(in crate::quic_listener) source: ControlApiBearerTokenSource,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(in crate::quic_listener) enum ControlApiBearerTokenSource {
    LegacyAuthToken,
    StaticTokenList,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiAuthorizationPolicy {
    pub(in crate::quic_listener) protect_health: bool,
    pub(in crate::quic_listener) protect_ready: bool,
    pub(in crate::quic_listener) runtime_read_role: ControlApiRole,
    pub(in crate::quic_listener) runtime_mutate_role: ControlApiRole,
    pub(in crate::quic_listener) restart_role: ControlApiRole,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiIdentitySourcePolicy {
    pub(in crate::quic_listener) kind: String,
    pub(in crate::quic_listener) role_attribute: Option<String>,
}

impl ControlApiIdentitySourcePolicy {
    fn from_config(config: &ControlApiIdentitySource) -> Self {
        Self {
            kind: config.kind.clone(),
            role_attribute: config.role_attribute.clone(),
        }
    }
}

impl ControlApiAuthorizationPolicy {
    fn from_config(config: &ControlApiConfig) -> Self {
        let authz = &config.authorization;
        Self {
            protect_health: authz.protect_health,
            protect_ready: authz.protect_ready,
            runtime_read_role: authz.runtime_read_role,
            runtime_mutate_role: authz.runtime_mutate_role,
            restart_role: authz.restart_role,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiIpAllowlistPolicy {
    pub(in crate::quic_listener) trust_proxy_headers: bool,
    pub(in crate::quic_listener) matcher: Option<ControlApiIpAllowlistMatcher>,
}

impl ControlApiIpAllowlistPolicy {
    fn from_config(config: &ControlApiConfig) -> Self {
        let cidrs = config
            .ip_allowlist
            .cidrs
            .iter()
            .filter_map(|cidr| ControlApiIpNetwork::parse(cidr))
            .collect::<Vec<_>>();
        Self {
            trust_proxy_headers: config.ip_allowlist.trust_proxy_headers,
            matcher: (!cidrs.is_empty()).then_some(ControlApiIpAllowlistMatcher { cidrs }),
        }
    }

    pub(in crate::quic_listener) fn allows(&self, ip: IpAddr) -> bool {
        self.matcher
            .as_ref()
            .is_none_or(|matcher| matcher.contains(ip))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) enum ControlApiSourcePolicyDecision {
    Allow,
    Deny { reason: &'static str },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiSourcePolicyContext {
    pub(in crate::quic_listener) source_ip: IpAddr,
    pub(in crate::quic_listener) trust_proxy_headers: bool,
}

impl ControlApiSecurityPolicy {
    pub(in crate::quic_listener) fn has_source_policy(&self) -> bool {
        self.ip_allowlist.matcher.is_some()
    }

    pub(in crate::quic_listener) fn evaluate_source_policy(
        &self,
        context: &ControlApiSourcePolicyContext,
    ) -> ControlApiSourcePolicyDecision {
        if !self.ip_allowlist.allows(context.source_ip) {
            return ControlApiSourcePolicyDecision::Deny {
                reason: "source_ip_not_allowed",
            };
        }

        // Phase 1 hook point for future sidecar / policy-engine integration.
        self.evaluate_external_source_policy_hooks(context)
    }

    fn evaluate_external_source_policy_hooks(
        &self,
        _context: &ControlApiSourcePolicyContext,
    ) -> ControlApiSourcePolicyDecision {
        ControlApiSourcePolicyDecision::Allow
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiIpAllowlistMatcher {
    cidrs: Vec<ControlApiIpNetwork>,
}

impl ControlApiIpAllowlistMatcher {
    #[allow(dead_code)]
    pub(in crate::quic_listener) fn contains(&self, ip: IpAddr) -> bool {
        self.cidrs.iter().any(|cidr| cidr.contains(ip))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum ControlApiIpNetwork {
    V4 { network: u32, prefix_len: u8 },
    V6 { network: u128, prefix_len: u8 },
}

impl ControlApiIpNetwork {
    fn parse(cidr: &str) -> Option<Self> {
        let (addr, prefix) = cidr.trim().split_once('/')?;
        let prefix = prefix.trim().parse::<u8>().ok()?;
        match addr.trim().parse::<IpAddr>().ok()? {
            IpAddr::V4(ip) if prefix <= 32 => {
                let raw = u32::from(ip);
                let mask = if prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - prefix)
                };
                Some(Self::V4 {
                    network: raw & mask,
                    prefix_len: prefix,
                })
            }
            IpAddr::V6(ip) if prefix <= 128 => {
                let raw = u128::from(ip);
                let mask = if prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - prefix)
                };
                Some(Self::V6 {
                    network: raw & mask,
                    prefix_len: prefix,
                })
            }
            _ => None,
        }
    }

    #[allow(dead_code)]
    fn contains(&self, ip: IpAddr) -> bool {
        match (self, ip) {
            (
                Self::V4 {
                    network,
                    prefix_len,
                },
                IpAddr::V4(ip),
            ) => {
                let raw = u32::from(ip);
                let mask = if *prefix_len == 0 {
                    0
                } else {
                    u32::MAX << (32 - *prefix_len)
                };
                (raw & mask) == *network
            }
            (
                Self::V6 {
                    network,
                    prefix_len,
                },
                IpAddr::V6(ip),
            ) => {
                let raw = u128::from(ip);
                let mask = if *prefix_len == 0 {
                    0
                } else {
                    u128::MAX << (128 - *prefix_len)
                };
                (raw & mask) == *network
            }
            _ => false,
        }
    }
}

fn runtime_bearer_tokens(config: &ControlApiConfig) -> Vec<ControlApiBearerTokenEntry> {
    let mut tokens = Vec::new();
    if let Some(token) = config.auth_token.as_ref() {
        tokens.push(ControlApiBearerTokenEntry {
            token: token.clone(),
            role: ControlApiRole::Admin,
            actor_id: Some("legacy_auth_token".to_string()),
            source: ControlApiBearerTokenSource::LegacyAuthToken,
        });
    }
    tokens.extend(
        config
            .auth
            .bearer_tokens
            .iter()
            .map(runtime_bearer_token_from_config),
    );
    tokens
}

fn runtime_bearer_token_from_config(token: &ControlApiBearerToken) -> ControlApiBearerTokenEntry {
    ControlApiBearerTokenEntry {
        token: token.token.clone(),
        role: token.role,
        actor_id: token.actor_id.clone(),
        source: ControlApiBearerTokenSource::StaticTokenList,
    }
}
