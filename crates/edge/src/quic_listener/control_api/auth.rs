use std::{collections::BTreeSet, net::SocketAddr};

use ::http::{Method, header};
use bytes::Bytes;
use http_body_util::Full;
use serde::Serialize;
use subtle::ConstantTimeEq;
use x509_parser::{extensions::GeneralName, parse_x509_certificate, prelude::X509Certificate};

use super::{
    security::{ControlApiIdentitySourcePolicy, ControlApiSecurityPolicy},
    state::{ControlApiPaths, ControlApiState},
    *,
};

type ControlApiGateError = Box<Response<Full<Bytes>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ControlApiRoute {
    Health,
    Ready,
    Runtime,
    RuntimeValidate,
    RuntimePreview,
    RuntimeActivate,
    RuntimeRollback,
    RuntimeHistory,
    RuntimeHistoryGeneration(u64),
    ReloadCerts,
    ReloadRuntime,
    Restart,
}

impl ControlApiRoute {
    fn minimum_role(self, security: &ControlApiSecurityPolicy) -> Option<AdminRole> {
        match self {
            Self::Health => security
                .authorization
                .protect_health
                .then_some(AdminRole::from(security.authorization.runtime_read_role)),
            Self::Ready => security
                .authorization
                .protect_ready
                .then_some(AdminRole::from(security.authorization.runtime_read_role)),
            Self::Runtime | Self::RuntimeHistory | Self::RuntimeHistoryGeneration(_) => {
                Some(AdminRole::from(security.authorization.runtime_read_role))
            }
            Self::RuntimeValidate
            | Self::RuntimePreview
            | Self::RuntimeActivate
            | Self::RuntimeRollback
            | Self::ReloadCerts
            | Self::ReloadRuntime => Some(AdminRole::from(
                security.authorization.runtime_mutate_role,
            )),
            Self::Restart => Some(AdminRole::from(security.authorization.restart_role)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AdminRole {
    Viewer,
    Operator,
    Admin,
}

impl From<spooky_config::config::ControlApiRole> for AdminRole {
    fn from(value: spooky_config::config::ControlApiRole) -> Self {
        match value {
            spooky_config::config::ControlApiRole::Viewer => Self::Viewer,
            spooky_config::config::ControlApiRole::Operator => Self::Operator,
            spooky_config::config::ControlApiRole::Admin => Self::Admin,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AdminAuthnMechanism {
    BearerToken,
    MutualTls,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct AdminIdentity {
    pub(super) actor_id: Option<String>,
    pub(super) authn_mechanisms: Vec<AdminAuthnMechanism>,
    pub(super) roles: Vec<AdminRole>,
    pub(super) peer_addr: Option<SocketAddr>,
    pub(super) mtls_subject: Option<String>,
    pub(super) mtls_san: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ControlApiRequestContext {
    pub(super) peer_addr: SocketAddr,
    pub(super) mtls_identity: Option<AdminMtlsIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AdminMtlsIdentity {
    pub(super) subject: Option<String>,
    pub(super) common_name: Option<String>,
    pub(super) san_dns: Vec<String>,
    pub(super) san_uri: Vec<String>,
    pub(super) roles: Vec<AdminRole>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum AuthorizationDecision {
    Allow {
        route: ControlApiRoute,
        identity: Option<AdminIdentity>,
    },
    Deny {
        status: StatusCode,
        error: &'static str,
        reason: &'static str,
        required_role: Option<AdminRole>,
        identity: Option<AdminIdentity>,
        route: ControlApiRoute,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AuthenticationOutcome {
    Authenticated(AdminIdentity),
    Missing,
    Invalid(&'static str),
}

#[derive(Clone, Debug)]
struct AdminTokenMatch {
    actor_id: Option<String>,
    role: AdminRole,
}

impl QUICListener {
    pub(super) fn control_api_request_route_for<B>(
        req: &::http::Request<B>,
        paths: &ControlApiPaths,
    ) -> Option<ControlApiRoute> {
        let path = req.uri().path();
        let runtime_validate_path = paths.runtime_validate_path();
        let runtime_preview_path = paths.runtime_preview_path();
        let runtime_activate_path = paths.runtime_activate_path();
        let runtime_rollback_path = paths.runtime_rollback_path();
        let runtime_history_path = paths.runtime_history_path();
        let runtime_history_entry_prefix = paths.runtime_history_entry_prefix();

        match *req.method() {
            Method::GET if path == paths.health_path.as_str() => Some(ControlApiRoute::Health),
            Method::GET if path == paths.ready_path.as_str() => Some(ControlApiRoute::Ready),
            Method::GET if path == paths.runtime_path.as_str() => Some(ControlApiRoute::Runtime),
            Method::GET if path == runtime_history_path.as_str() => {
                Some(ControlApiRoute::RuntimeHistory)
            }
            Method::GET if path.starts_with(runtime_history_entry_prefix.as_str()) => path
                .strip_prefix(runtime_history_entry_prefix.as_str())
                .and_then(|raw_generation| raw_generation.parse::<u64>().ok())
                .map(ControlApiRoute::RuntimeHistoryGeneration),
            Method::POST if path == paths.reload_certs_path.as_str() => {
                Some(ControlApiRoute::ReloadCerts)
            }
            Method::POST if path == runtime_validate_path.as_str() => {
                Some(ControlApiRoute::RuntimeValidate)
            }
            Method::POST if path == runtime_preview_path.as_str() => {
                Some(ControlApiRoute::RuntimePreview)
            }
            Method::POST if path == runtime_activate_path.as_str() => {
                Some(ControlApiRoute::RuntimeActivate)
            }
            Method::POST if path == runtime_rollback_path.as_str() => {
                Some(ControlApiRoute::RuntimeRollback)
            }
            Method::POST if path == paths.reload_path.as_str() => {
                Some(ControlApiRoute::ReloadRuntime)
            }
            Method::POST if path == paths.restart_path.as_str() => Some(ControlApiRoute::Restart),
            _ => None,
        }
    }

    pub(super) fn bearer_token_from_authorization_header(raw: &str) -> Option<&str> {
        let raw = raw.trim();
        let split = raw.find(char::is_whitespace)?;
        let (scheme, rest) = raw.split_at(split);
        if !scheme.eq_ignore_ascii_case("bearer") {
            return None;
        }
        let token = rest.trim_start();
        if token.is_empty() {
            return None;
        }
        Some(token)
    }

    pub(super) fn build_control_api_request_context(
        peer_addr: SocketAddr,
        peer_certificates: Option<&[CertificateDer<'static>]>,
        identity_source: Option<&ControlApiIdentitySourcePolicy>,
    ) -> ControlApiRequestContext {
        let mtls_identity = peer_certificates
            .and_then(|certs| certs.first())
            .and_then(|cert| Self::parse_admin_mtls_identity(cert.as_ref(), identity_source));
        ControlApiRequestContext {
            peer_addr,
            mtls_identity,
        }
    }

    fn parse_admin_mtls_identity(
        cert_der: &[u8],
        identity_source: Option<&ControlApiIdentitySourcePolicy>,
    ) -> Option<AdminMtlsIdentity> {
        let (_, cert) = parse_x509_certificate(cert_der).ok()?;
        let subject = Some(cert.subject().to_string());
        let mut common_name = None;
        for cn in cert.subject().iter_common_name() {
            if let Ok(value) = cn.as_str() {
                common_name = Some(value.to_string());
                break;
            }
        }

        let mut san_dns = Vec::new();
        let mut san_uri = Vec::new();
        if let Ok(Some(san)) = cert.subject_alternative_name() {
            for name in &san.value.general_names {
                match name {
                    GeneralName::DNSName(dns) => san_dns.push(dns.to_string()),
                    GeneralName::URI(uri) => san_uri.push(uri.to_string()),
                    _ => {}
                }
            }
        }

        let roles = identity_source
            .and_then(|policy| policy.role_attribute.as_deref())
            .map(|attribute| Self::extract_roles_from_subject_attribute(&cert, attribute))
            .unwrap_or_default();

        Some(AdminMtlsIdentity {
            subject,
            common_name,
            san_dns,
            san_uri,
            roles,
        })
    }

    fn extract_roles_from_subject_attribute(
        cert: &X509Certificate<'_>,
        attribute: &str,
    ) -> Vec<AdminRole> {
        let mut roles = Vec::new();
        for attr in cert.subject().iter_attributes() {
            let short_name = attr.attr_type().to_id_string();
            let matches = short_name.eq_ignore_ascii_case(attribute)
                || Self::matches_subject_attribute_alias(&short_name, attribute);
            if !matches {
                continue;
            }
            if let Ok(value) = attr.as_str()
                && let Some(role) = Self::parse_admin_role(value)
            {
                roles.push(role);
            }
        }
        roles
    }

    fn matches_subject_attribute_alias(actual: &str, requested: &str) -> bool {
        match requested.to_ascii_lowercase().as_str() {
            "cn" => actual == "2.5.4.3",
            "o" => actual == "2.5.4.10",
            "ou" => actual == "2.5.4.11",
            _ => false,
        }
    }

    fn parse_admin_role(value: &str) -> Option<AdminRole> {
        match value.trim().to_ascii_lowercase().as_str() {
            "viewer" => Some(AdminRole::Viewer),
            "operator" => Some(AdminRole::Operator),
            "admin" => Some(AdminRole::Admin),
            _ => None,
        }
    }

    fn authenticate_control_api_request<B>(
        req: &::http::Request<B>,
        security: &ControlApiSecurityPolicy,
    ) -> AuthenticationOutcome {
        let request_ctx = req.extensions().get::<ControlApiRequestContext>().cloned();
        let token_match = match req.headers().get(header::AUTHORIZATION) {
            Some(value) => {
                let Ok(raw) = value.to_str() else {
                    return AuthenticationOutcome::Invalid("invalid_authorization_header");
                };
                let Some(provided) = Self::bearer_token_from_authorization_header(raw) else {
                    return AuthenticationOutcome::Invalid("invalid_bearer_token");
                };
                match Self::control_api_token_match(provided, security) {
                    Some(token) => Some(token),
                    None => return AuthenticationOutcome::Invalid("invalid_bearer_token"),
                }
            }
            None => None,
        };

        let mtls_identity = request_ctx
            .as_ref()
            .and_then(|ctx| ctx.mtls_identity.as_ref())
            .cloned();

        if token_match.is_none() && mtls_identity.is_none() {
            return AuthenticationOutcome::Missing;
        }

        let mut roles = BTreeSet::new();
        let mut mechanisms = Vec::new();
        let mut actor_id = None;
        if let Some(token) = token_match {
            mechanisms.push(AdminAuthnMechanism::BearerToken);
            roles.insert(token.role);
            actor_id = token.actor_id.or(actor_id);
        }
        if let Some(mtls) = mtls_identity.as_ref() {
            mechanisms.push(AdminAuthnMechanism::MutualTls);
            for role in &mtls.roles {
                roles.insert(*role);
            }
            actor_id = actor_id.or_else(|| {
                Self::actor_id_from_mtls_identity(mtls, security.identity_source.as_ref())
            });
        }

        AuthenticationOutcome::Authenticated(AdminIdentity {
            actor_id,
            authn_mechanisms: mechanisms,
            roles: roles.into_iter().collect(),
            peer_addr: request_ctx.as_ref().map(|ctx| ctx.peer_addr),
            mtls_subject: mtls_identity.as_ref().and_then(|identity| identity.subject.clone()),
            mtls_san: mtls_identity
                .as_ref()
                .map(|identity| {
                    identity
                        .san_dns
                        .iter()
                        .cloned()
                        .chain(identity.san_uri.iter().cloned())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    fn actor_id_from_mtls_identity(
        mtls: &AdminMtlsIdentity,
        identity_source: Option<&ControlApiIdentitySourcePolicy>,
    ) -> Option<String> {
        match identity_source
            .map(|policy| policy.kind.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("mtls_subject_cn") => mtls.common_name.clone(),
            Some("mtls_san_dns") => mtls.san_dns.first().cloned(),
            Some("mtls_san_uri") => mtls.san_uri.first().cloned(),
            Some("mtls_subject") => mtls.subject.clone(),
            _ => mtls.common_name.clone().or_else(|| mtls.subject.clone()),
        }
    }

    fn control_api_token_match(
        provided: &str,
        security: &ControlApiSecurityPolicy,
    ) -> Option<AdminTokenMatch> {
        let mut matched = None;
        let mut matched_any = 0u8;
        for entry in security.bearer_tokens.iter() {
            let is_match = bool::from(provided.as_bytes().ct_eq(entry.token.as_bytes())) as u8;
            if is_match == 1 {
                matched = Some(AdminTokenMatch {
                    actor_id: entry.actor_id.clone(),
                    role: AdminRole::from(entry.role),
                });
            }
            matched_any |= is_match;
        }
        (matched_any == 1).then_some(matched).flatten()
    }

    #[cfg(test)]
    pub(super) fn control_api_is_authorized_for<B>(
        req: &::http::Request<B>,
        security: &ControlApiSecurityPolicy,
    ) -> bool {
        matches!(
            Self::authenticate_control_api_request(req, security),
            AuthenticationOutcome::Authenticated(_)
        )
    }

    fn authorize_control_api_request_for<B>(
        req: &mut ::http::Request<B>,
        state: &ControlApiState,
        route: ControlApiRoute,
    ) -> Result<(), ControlApiGateError> {
        let service_state = state.current_service_state();
        let Some(required_role) = route.minimum_role(&service_state.security) else {
            return Ok(());
        };

        let decision = match Self::authenticate_control_api_request(req, &service_state.security) {
            AuthenticationOutcome::Missing => AuthorizationDecision::Deny {
                status: StatusCode::UNAUTHORIZED,
                error: "unauthorized",
                reason: "missing_authentication",
                required_role: Some(required_role),
                identity: None,
                route,
            },
            AuthenticationOutcome::Invalid(reason) => AuthorizationDecision::Deny {
                status: StatusCode::UNAUTHORIZED,
                error: "unauthorized",
                reason,
                required_role: Some(required_role),
                identity: None,
                route,
            },
            AuthenticationOutcome::Authenticated(identity) => {
                let authorized = identity.roles.iter().any(|role| *role >= required_role);
                if authorized {
                    AuthorizationDecision::Allow {
                        route,
                        identity: Some(identity),
                    }
                } else {
                    AuthorizationDecision::Deny {
                        status: StatusCode::FORBIDDEN,
                        error: "forbidden",
                        reason: "insufficient_role",
                        required_role: Some(required_role),
                        identity: Some(identity),
                        route,
                    }
                }
            }
        };

        match decision {
            AuthorizationDecision::Allow { identity, .. } => {
                if let Some(identity) = identity {
                    req.extensions_mut().insert(identity);
                }
                Ok(())
            }
            AuthorizationDecision::Deny {
                status,
                error,
                reason,
                required_role,
                route,
                ..
            } => Err(Box::new(Self::control_api_auth_error_response(
                route,
                status,
                error,
                reason,
                required_role,
            ))),
        }
    }

    fn control_api_auth_error_response(
        route: ControlApiRoute,
        status: StatusCode,
        error: &'static str,
        reason: &'static str,
        required_role: Option<AdminRole>,
    ) -> Response<Full<Bytes>> {
        let required_role = required_role.map(|role| match role {
            AdminRole::Viewer => "viewer",
            AdminRole::Operator => "operator",
            AdminRole::Admin => "admin",
        });
        let response = match route {
            ControlApiRoute::Runtime
            | ControlApiRoute::RuntimeValidate
            | ControlApiRoute::RuntimePreview
            | ControlApiRoute::RuntimeActivate
            | ControlApiRoute::RuntimeRollback
            | ControlApiRoute::RuntimeHistory
            | ControlApiRoute::RuntimeHistoryGeneration(_)
            | ControlApiRoute::Health
            | ControlApiRoute::Ready => json!({
                "error": error,
                "reason": reason,
                "required_role": required_role,
            }),
            ControlApiRoute::ReloadCerts | ControlApiRoute::ReloadRuntime => json!({
                "reloaded": false,
                "error": error,
                "reason": reason,
                "required_role": required_role,
            }),
            ControlApiRoute::Restart => json!({
                "accepted": false,
                "error": error,
                "reason": reason,
                "required_role": required_role,
            }),
        };
        Self::json_response(status, response)
    }

    pub(super) fn gate_control_api_request_for<B>(
        req: &mut ::http::Request<B>,
        state: &ControlApiState,
    ) -> Result<ControlApiRoute, ControlApiGateError> {
        let service_state = state.current_service_state();
        let Some(route) = Self::control_api_request_route_for(req, &service_state.paths) else {
            return Err(Box::new(Self::control_api_not_found_response()));
        };
        Self::authorize_control_api_request_for(req, state, route)?;
        Ok(route)
    }

    pub(super) fn gate_control_api_request(
        req: &mut Request<Incoming>,
        state: &ControlApiState,
    ) -> Result<ControlApiRoute, ControlApiGateError> {
        Self::gate_control_api_request_for(req, state)
    }

    pub(super) fn control_api_not_found_response() -> Response<Full<Bytes>> {
        match Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from_static(b"not found\n")))
        {
            Ok(resp) => resp,
            Err(_) => Response::new(Full::new(Bytes::from_static(b"not found\n"))),
        }
    }
}
