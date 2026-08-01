use serde::Serialize;
use spooky_config::config::{ControlApi as ControlApiConfig, ControlApiAuditFormat, ControlApiAuditSink};

use super::admin_identity::{AdminAuthnMechanism, AdminIdentity, AdminRole};

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
    pub(in crate::quic_listener) event_type: AdminAuditEventType,
    pub(in crate::quic_listener) time_unix_ms: u64,
    pub(in crate::quic_listener) actor: AdminAuditActor,
    pub(in crate::quic_listener) action: AdminAuditAction,
    pub(in crate::quic_listener) target: AdminAuditTarget,
    pub(in crate::quic_listener) generation: AdminAuditGeneration,
    pub(in crate::quic_listener) result: AdminAuditResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) reason: Option<String>,
    pub(in crate::quic_listener) event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::quic_listener) peer_addr: Option<String>,
    pub(in crate::quic_listener) authn: AdminAuditAuthn,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::quic_listener) enum AdminAuditEventType {
    Auth,
    RuntimeSnapshot,
    RuntimeReload,
    RuntimeActivate,
    RuntimeRollback,
    RuntimeRestart,
    CertReload,
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
            roles: identity.map(|identity| identity.roles.clone()).unwrap_or_default(),
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
            event_type: AdminAuditEventType::RuntimeReload,
            time_unix_ms: 42,
            actor: AdminAuditActor::from_identity(Some(&identity)),
            action: AdminAuditAction::RuntimeReloadAttempt,
            target: AdminAuditTarget {
                route: Some("/admin/runtime/reload".to_string()),
                resource: Some("control_api".to_string()),
                config_path: Some("/etc/spooky/config.yaml".to_string()),
            },
            generation: AdminAuditGeneration {
                active_generation: Some(7),
                candidate_generation: Some(8),
                target_generation: None,
            },
            result: AdminAuditResult::Success,
            reason: Some("operator_requested".to_string()),
            event_id: "evt-1".to_string(),
            peer_addr: Some("127.0.0.1:9999".to_string()),
            authn: AdminAuditAuthn::from_identity(Some(&identity)),
        };

        let value = serde_json::to_value(&event).expect("serialize audit event");
        assert_eq!(value["event_type"], "runtime_reload");
        assert_eq!(value["action"], "runtime_reload.attempt");
        assert_eq!(value["result"], "success");
        assert_eq!(value["actor"]["id"], "alice");
        assert_eq!(value["actor"]["roles"][0], "operator");
        assert_eq!(value["authn"]["mechanisms"][0], "bearer_token");
        assert_eq!(value["generation"]["active_generation"], 7);
        assert_eq!(value["generation"]["candidate_generation"], 8);
    }
}
