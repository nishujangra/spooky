use std::{fs::OpenOptions, io::Write, sync::atomic::Ordering};

use log::{error, info, warn};
use serde::Serialize;
use impulse_config::config::{
    ControlApi as ControlApiConfig, ControlApiAuditFormat, ControlApiAuditSink,
};
use impulse_utils::logger::CONTROL_API_AUDIT_LOG_TARGET;

use super::{
    admin_auth::ControlApiRoute,
    admin_identity::{AdminAuthnMechanism, AdminIdentity, AdminRole, ControlApiRequestContext},
    security::ControlApiSecurityPolicy,
    *,
};
use crate::REQUEST_ID_COUNTER;

pub(super) const ADMIN_AUDIT_SCHEMA_VERSION: &str = "v1";

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiAdminAuditEmitter {
    pub(in crate::quic_listener) enabled: bool,
    pub(in crate::quic_listener) format: ControlApiAuditFormat,
    pub(in crate::quic_listener) sink: ControlApiAdminAuditTarget,
}

impl ControlApiAdminAuditEmitter {
    pub(in crate::quic_listener) fn from_config(config: &ControlApiConfig) -> Self {
        Self {
            enabled: config.audit.enabled,
            format: config.audit.format,
            sink: match config.audit.sink {
                ControlApiAuditSink::Log => ControlApiAdminAuditTarget::Log,
                ControlApiAuditSink::File => {
                    ControlApiAdminAuditTarget::File(config.audit.file_path.clone())
                }
            },
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) enum ControlApiAdminAuditTarget {
    Log,
    File(Option<String>),
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub(in crate::quic_listener) struct AdminAuditEvent {
    pub(in crate::quic_listener) schema_version: &'static str,
    pub(in crate::quic_listener) event_id: String,
    pub(in crate::quic_listener) event_type: AdminAuditEventType,
    pub(in crate::quic_listener) time_unix_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) span_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) listener: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) backend: Option<String>,
    pub(in crate::quic_listener) actor: AdminAuditActor,
    pub(in crate::quic_listener) action: AdminAuditAction,
    pub(in crate::quic_listener) target: AdminAuditTarget,
    pub(in crate::quic_listener) generation: AdminAuditGeneration,
    pub(in crate::quic_listener) result: AdminAuditResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) failure_class: Option<AdminAuditFailureClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) peer_addr: Option<String>,
    pub(in crate::quic_listener) authn: AdminAuditAuthn,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::quic_listener) enum AdminAuditFailureClass {
    Authentication,
    Authorization,
    SourcePolicy,
    RequestValidation,
    RuntimeConfig,
    RuntimeState,
    ListenerTls,
    Watchdog,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::quic_listener) enum AdminAuditEventType {
    Auth,
    RuntimeSnapshot,
    RuntimeValidate,
    RuntimePreview,
    RuntimeReload,
    RuntimeActivate,
    RuntimeRollback,
    RuntimeRestart,
    CertReload,
    /// Upstream secret-backed TLS material (client cert/key, CA bundle)
    /// changed and was applied through generation activation.
    UpstreamMtlsMaterial,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub(in crate::quic_listener) struct AdminAuditActor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) id: Option<String>,
    pub(in crate::quic_listener) roles: Vec<AdminRole>,
    pub(in crate::quic_listener) authn_mechanisms: Vec<AdminAuthnMechanism>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) mtls_subject: Option<String>,
}

impl AdminAuditActor {
    pub(in crate::quic_listener) fn from_identity(identity: Option<&AdminIdentity>) -> Self {
        Self {
            id: identity.and_then(|identity| identity.actor_id.clone()),
            roles: identity
                .map(|identity| identity.roles.clone())
                .unwrap_or_default(),
            authn_mechanisms: identity
                .map(|identity| identity.authn_mechanisms.clone())
                .unwrap_or_default(),
            mtls_subject: identity.and_then(|identity| identity.mtls_subject.clone()),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
pub(in crate::quic_listener) enum AdminAuditAction {
    #[serde(rename = "auth")]
    Auth,
    #[serde(rename = "runtime_snapshot.read")]
    RuntimeSnapshotRead,
    #[serde(rename = "runtime_validate.attempt")]
    RuntimeValidateAttempt,
    #[serde(rename = "runtime_validate.result")]
    RuntimeValidateResult,
    #[serde(rename = "runtime_preview.attempt")]
    RuntimePreviewAttempt,
    #[serde(rename = "runtime_preview.result")]
    RuntimePreviewResult,
    #[serde(rename = "runtime_reload.attempt")]
    RuntimeReloadAttempt,
    #[serde(rename = "runtime_reload.result")]
    RuntimeReloadResult,
    #[serde(rename = "runtime_activate.attempt")]
    RuntimeActivateAttempt,
    #[serde(rename = "runtime_activate.result")]
    RuntimeActivateResult,
    #[serde(rename = "runtime_rollback.attempt")]
    RuntimeRollbackAttempt,
    #[serde(rename = "runtime_rollback.result")]
    RuntimeRollbackResult,
    #[serde(rename = "runtime_restart.attempt")]
    RuntimeRestartAttempt,
    #[serde(rename = "runtime_restart.result")]
    RuntimeRestartResult,
    #[serde(rename = "cert_reload.attempt")]
    CertReloadAttempt,
    #[serde(rename = "cert_reload.result")]
    CertReloadResult,
    #[serde(rename = "cert_reload_applied")]
    CertReloadApplied,
    #[serde(rename = "secret_resolution_failed")]
    SecretResolutionFailed,
    #[serde(rename = "upstream_mtls_material_changed")]
    UpstreamMtlsMaterialChanged,
    #[serde(rename = "upstream_mtls_material_invalid")]
    UpstreamMtlsMaterialInvalid,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub(in crate::quic_listener) struct AdminAuditTarget {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) config_path: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Default)]
pub(in crate::quic_listener) struct AdminAuditGeneration {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) active_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) candidate_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) target_generation: Option<u64>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::quic_listener) enum AdminAuditResult {
    Success,
    Denied,
    Failed,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub(in crate::quic_listener) struct AdminAuditAuthn {
    pub(in crate::quic_listener) mechanisms: Vec<AdminAuthnMechanism>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) mtls_subject: Option<String>,
}

impl AdminAuditAuthn {
    pub(in crate::quic_listener) fn from_identity(identity: Option<&AdminIdentity>) -> Self {
        Self {
            mechanisms: identity
                .map(|identity| identity.authn_mechanisms.clone())
                .unwrap_or_default(),
            mtls_subject: identity.and_then(|identity| identity.mtls_subject.clone()),
        }
    }
}

impl QUICListener {
    // The audit event is a flat record; passing its fields individually keeps the
    // ~24 emission sites readable rather than forcing a builder at each one.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_control_api_audit_event(
        security: &ControlApiSecurityPolicy,
        identity: Option<&AdminIdentity>,
        request_context: Option<&ControlApiRequestContext>,
        event_type: AdminAuditEventType,
        action: AdminAuditAction,
        target: AdminAuditTarget,
        generation: AdminAuditGeneration,
        result: AdminAuditResult,
        reason: Option<String>,
    ) {
        let emitter = &security.audit;
        if !emitter.enabled {
            return;
        }
        let reason = reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);

        let event = AdminAuditEvent {
            schema_version: ADMIN_AUDIT_SCHEMA_VERSION,
            event_id: format!(
                "control-api-audit-{}",
                REQUEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
            event_type,
            time_unix_ms: crate::watchdog::time::now_millis(),
            request_id: Self::control_api_audit_request_id(request_context),
            trace_id: Self::control_api_audit_trace_id(request_context),
            span_id: Self::control_api_audit_span_id(request_context),
            listener: Self::control_api_audit_listener(request_context),
            upstream: None,
            backend: None,
            actor: AdminAuditActor::from_identity(identity),
            action,
            target,
            generation,
            result,
            failure_class: Self::control_api_audit_failure_class(
                event_type,
                result,
                reason.as_deref(),
            ),
            reason,
            peer_addr: Self::control_api_audit_peer_addr(identity, request_context),
            authn: AdminAuditAuthn::from_identity(identity),
        };

        emitter.emit(&event);
    }

    pub(super) fn emit_control_api_auth_audit_event(
        security: &ControlApiSecurityPolicy,
        identity: Option<&AdminIdentity>,
        request_context: Option<&ControlApiRequestContext>,
        route: ControlApiRoute,
        active_generation: Option<u64>,
        result: AdminAuditResult,
        reason: impl Into<String>,
    ) {
        Self::emit_control_api_audit_event(
            security,
            identity,
            request_context,
            AdminAuditEventType::Auth,
            AdminAuditAction::Auth,
            Self::control_api_audit_target_for_route(route, None),
            AdminAuditGeneration {
                active_generation,
                ..Default::default()
            },
            result,
            Some(reason.into()),
        );
    }

    pub(super) fn emit_control_api_route_denial_audit_event(
        security: &ControlApiSecurityPolicy,
        identity: Option<&AdminIdentity>,
        request_context: Option<&ControlApiRequestContext>,
        route: ControlApiRoute,
        active_generation: Option<u64>,
        reason: impl Into<String>,
    ) {
        let Some((event_type, action)) = Self::control_api_denied_action_for_route(route) else {
            return;
        };

        Self::emit_control_api_audit_event(
            security,
            identity,
            request_context,
            event_type,
            action,
            Self::control_api_audit_target_for_route(route, None),
            AdminAuditGeneration {
                active_generation,
                ..Default::default()
            },
            AdminAuditResult::Denied,
            Some(reason.into()),
        );
    }

    pub(super) fn control_api_audit_target_for_route(
        route: ControlApiRoute,
        config_path: Option<String>,
    ) -> AdminAuditTarget {
        let resource = match route {
            ControlApiRoute::Health | ControlApiRoute::Ready => {
                Some("control_api_status".to_string())
            }
            ControlApiRoute::Runtime
            | ControlApiRoute::RuntimeHistory
            | ControlApiRoute::RuntimeHistoryGeneration(_) => Some("runtime_state".to_string()),
            ControlApiRoute::RuntimeValidate
            | ControlApiRoute::RuntimePreview
            | ControlApiRoute::RuntimeActivate
            | ControlApiRoute::RuntimeRollback
            | ControlApiRoute::ReloadRuntime => Some("runtime_generation".to_string()),
            ControlApiRoute::ReloadCerts => Some("listener_tls".to_string()),
            ControlApiRoute::Restart => Some("watchdog".to_string()),
        };

        AdminAuditTarget {
            route: Some(Self::control_api_route_name(route).to_string()),
            resource,
            config_path,
        }
    }

    pub(super) fn control_api_route_name(route: ControlApiRoute) -> &'static str {
        match route {
            ControlApiRoute::Health => "/health",
            ControlApiRoute::Ready => "/ready",
            ControlApiRoute::Runtime => "/admin/runtime",
            ControlApiRoute::RuntimeValidate => "/admin/runtime/validate",
            ControlApiRoute::RuntimePreview => "/admin/runtime/preview",
            ControlApiRoute::RuntimeActivate => "/admin/runtime/activate",
            ControlApiRoute::RuntimeRollback => "/admin/runtime/rollback",
            ControlApiRoute::RuntimeHistory => "/admin/runtime/history",
            ControlApiRoute::RuntimeHistoryGeneration(_) => "/admin/runtime/history/{generation}",
            ControlApiRoute::ReloadCerts => "/admin/reload-certs",
            ControlApiRoute::ReloadRuntime => "/admin/reload",
            ControlApiRoute::Restart => "/admin/restart",
        }
    }

    fn control_api_denied_action_for_route(
        route: ControlApiRoute,
    ) -> Option<(AdminAuditEventType, AdminAuditAction)> {
        match route {
            ControlApiRoute::Runtime
            | ControlApiRoute::RuntimeHistory
            | ControlApiRoute::RuntimeHistoryGeneration(_) => Some((
                AdminAuditEventType::RuntimeSnapshot,
                AdminAuditAction::RuntimeSnapshotRead,
            )),
            ControlApiRoute::RuntimeValidate => Some((
                AdminAuditEventType::RuntimeValidate,
                AdminAuditAction::RuntimeValidateAttempt,
            )),
            ControlApiRoute::RuntimePreview => Some((
                AdminAuditEventType::RuntimePreview,
                AdminAuditAction::RuntimePreviewAttempt,
            )),
            ControlApiRoute::RuntimeActivate => Some((
                AdminAuditEventType::RuntimeActivate,
                AdminAuditAction::RuntimeActivateAttempt,
            )),
            ControlApiRoute::RuntimeRollback => Some((
                AdminAuditEventType::RuntimeRollback,
                AdminAuditAction::RuntimeRollbackAttempt,
            )),
            ControlApiRoute::ReloadCerts => Some((
                AdminAuditEventType::CertReload,
                AdminAuditAction::CertReloadAttempt,
            )),
            ControlApiRoute::ReloadRuntime => Some((
                AdminAuditEventType::RuntimeReload,
                AdminAuditAction::RuntimeReloadAttempt,
            )),
            ControlApiRoute::Restart => Some((
                AdminAuditEventType::RuntimeRestart,
                AdminAuditAction::RuntimeRestartAttempt,
            )),
            ControlApiRoute::Health | ControlApiRoute::Ready => None,
        }
    }

    fn control_api_audit_peer_addr(
        identity: Option<&AdminIdentity>,
        request_context: Option<&ControlApiRequestContext>,
    ) -> Option<String> {
        identity
            .and_then(|identity| identity.peer_addr)
            .or_else(|| request_context.map(|context| context.peer_addr))
            .map(|addr| addr.to_string())
    }

    fn control_api_audit_request_id(
        request_context: Option<&ControlApiRequestContext>,
    ) -> Option<String> {
        request_context.and_then(|context| context.request_id.clone())
    }

    fn control_api_audit_trace_id(
        request_context: Option<&ControlApiRequestContext>,
    ) -> Option<String> {
        request_context.and_then(|context| context.trace_id.clone())
    }

    fn control_api_audit_span_id(
        request_context: Option<&ControlApiRequestContext>,
    ) -> Option<String> {
        request_context.and_then(|context| context.span_id.clone())
    }

    fn control_api_audit_listener(
        request_context: Option<&ControlApiRequestContext>,
    ) -> Option<String> {
        request_context.and_then(|context| context.listener.clone())
    }

    fn control_api_audit_failure_class(
        event_type: AdminAuditEventType,
        result: AdminAuditResult,
        reason: Option<&str>,
    ) -> Option<AdminAuditFailureClass> {
        if matches!(result, AdminAuditResult::Success) {
            return None;
        }

        match reason {
            Some("missing_authentication")
            | Some("invalid_authorization_header")
            | Some("invalid_bearer_token") => {
                return Some(AdminAuditFailureClass::Authentication);
            }
            Some("insufficient_role") => {
                return Some(AdminAuditFailureClass::Authorization);
            }
            Some("missing_peer_context") | Some("source_ip_not_allowed") => {
                return Some(AdminAuditFailureClass::SourcePolicy);
            }
            Some("invalid_request_body") => {
                return Some(AdminAuditFailureClass::RequestValidation);
            }
            _ => {}
        }

        match event_type {
            AdminAuditEventType::Auth => Some(AdminAuditFailureClass::Authentication),
            AdminAuditEventType::RuntimeSnapshot => Some(AdminAuditFailureClass::RuntimeState),
            AdminAuditEventType::RuntimeValidate
            | AdminAuditEventType::RuntimePreview
            | AdminAuditEventType::RuntimeReload
            | AdminAuditEventType::RuntimeActivate
            | AdminAuditEventType::RuntimeRollback
            | AdminAuditEventType::UpstreamMtlsMaterial => {
                Some(AdminAuditFailureClass::RuntimeConfig)
            }
            AdminAuditEventType::RuntimeRestart => Some(AdminAuditFailureClass::Watchdog),
            AdminAuditEventType::CertReload => Some(AdminAuditFailureClass::ListenerTls),
        }
    }
}

impl ControlApiAdminAuditEmitter {
    fn emit(&self, event: &AdminAuditEvent) {
        let serialized = match self.format {
            ControlApiAuditFormat::Json => match serde_json::to_string(event) {
                Ok(serialized) => serialized,
                Err(err) => {
                    error!("failed to serialize control API admin audit event: {}", err);
                    return;
                }
            },
        };

        match &self.sink {
            ControlApiAdminAuditTarget::Log => {
                info!(target: CONTROL_API_AUDIT_LOG_TARGET, "{}", serialized)
            }
            ControlApiAdminAuditTarget::File(Some(path)) => {
                match OpenOptions::new().create(true).append(true).open(path) {
                    Ok(mut file) => {
                        if let Err(err) = writeln!(file, "{}", serialized) {
                            error!(
                                "failed to write control API admin audit event to {}: {}",
                                path, err
                            );
                        }
                    }
                    Err(err) => error!(
                        "failed to open control API admin audit sink {}: {}",
                        path, err
                    ),
                }
            }
            ControlApiAdminAuditTarget::File(None) => {
                warn!(
                    "control API admin audit sink configured as file without file_path; falling back to log"
                );
                info!(target: CONTROL_API_AUDIT_LOG_TARGET, "{}", serialized);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quic_listener::control_api::admin_identity::{
        AdminAuthnMechanism, AdminIdentity, AdminRole,
    };

    #[test]
    fn admin_audit_event_serialization_stays_stable() {
        let identity = AdminIdentity {
            actor_id: Some("alice".to_string()),
            authn_mechanisms: vec![AdminAuthnMechanism::BearerToken],
            roles: vec![AdminRole::Operator],
            peer_addr: Some("127.0.0.1:9999".parse().expect("socket addr")),
            mtls_subject: Some("CN=alice".to_string()),
            mtls_san: vec!["spiffe://alice".to_string()],
        };
        let event = AdminAuditEvent {
            schema_version: ADMIN_AUDIT_SCHEMA_VERSION,
            event_id: "evt-1".to_string(),
            event_type: AdminAuditEventType::RuntimeReload,
            time_unix_ms: 42,
            request_id: Some("req-7".to_string()),
            trace_id: Some("4bf92f3577b34da6a3ce929d0e0e4736".to_string()),
            span_id: Some("00f067aa0ba902b7".to_string()),
            listener: Some("edge-primary".to_string()),
            upstream: None,
            backend: None,
            actor: AdminAuditActor::from_identity(Some(&identity)),
            action: AdminAuditAction::RuntimeReloadAttempt,
            target: AdminAuditTarget {
                route: Some("/admin/runtime/reload".to_string()),
                resource: Some("control_api".to_string()),
                config_path: Some("/etc/impulse/config.yaml".to_string()),
            },
            generation: AdminAuditGeneration {
                active_generation: Some(7),
                candidate_generation: Some(8),
                target_generation: None,
            },
            result: AdminAuditResult::Success,
            reason: Some("operator_requested".to_string()),
            failure_class: None,
            peer_addr: Some("127.0.0.1:9999".to_string()),
            authn: AdminAuditAuthn::from_identity(Some(&identity)),
        };

        let value = serde_json::to_value(&event).expect("serialize audit event");
        assert_eq!(
            value,
            serde_json::json!({
                "schema_version": "v1",
                "event_id": "evt-1",
                "event_type": "runtime_reload",
                "time_unix_ms": 42,
                "request_id": "req-7",
                "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
                "span_id": "00f067aa0ba902b7",
                "listener": "edge-primary",
                "actor": {
                    "id": "alice",
                    "roles": ["operator"],
                    "authn_mechanisms": ["bearer_token"],
                    "mtls_subject": "CN=alice"
                },
                "action": "runtime_reload.attempt",
                "target": {
                    "route": "/admin/runtime/reload",
                    "resource": "control_api",
                    "config_path": "/etc/impulse/config.yaml"
                },
                "generation": {
                    "active_generation": 7,
                    "candidate_generation": 8
                },
                "result": "success",
                "reason": "operator_requested",
                "peer_addr": "127.0.0.1:9999",
                "authn": {
                    "mechanisms": ["bearer_token"],
                    "mtls_subject": "CN=alice"
                }
            })
        );
    }

    #[test]
    fn admin_audit_failure_class_serializes_low_cardinality_category() {
        assert_eq!(
            QUICListener::control_api_audit_failure_class(
                AdminAuditEventType::RuntimeValidate,
                AdminAuditResult::Failed,
                Some("invalid_request_body"),
            ),
            Some(AdminAuditFailureClass::RequestValidation)
        );
        assert_eq!(
            QUICListener::control_api_audit_failure_class(
                AdminAuditEventType::Auth,
                AdminAuditResult::Denied,
                Some("invalid_bearer_token"),
            ),
            Some(AdminAuditFailureClass::Authentication)
        );
        assert_eq!(
            QUICListener::control_api_audit_failure_class(
                AdminAuditEventType::RuntimeRestart,
                AdminAuditResult::Failed,
                Some("watchdog_disabled"),
            ),
            Some(AdminAuditFailureClass::Watchdog)
        );
    }
}
