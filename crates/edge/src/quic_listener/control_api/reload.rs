use std::sync::Arc;

use bytes::Bytes;
use hyper::body::Body;
use http_body_util::{BodyExt, Full};
use serde::{Deserialize, de::DeserializeOwned};

use super::{
    admin_auth::ControlApiRoute,
    admin_identity::{AdminIdentity, ControlApiRequestContext},
    audit::{AdminAuditAction, AdminAuditEventType, AdminAuditGeneration, AdminAuditResult},
    *,
};
use crate::runtime::{
    activation::{
        ActivationRequest, ActivationResult, RejectedChangeKind, ReloadConfigInput, ReloadPlan,
        RollbackRequest, RollbackResult, RuntimeActivationService, RuntimeRejectionReason,
    },
    bundle::{ActiveRuntimeGeneration, RuntimeBundleHandle},
    policy::{ReloadCompatibilityAuthority, TransitionRejection},
};

#[derive(Default, Deserialize)]
struct ControlApiRuntimePlanRequest {
    config_path: Option<String>,
    requested_by: Option<String>,
    reason: Option<String>,
    expected_generation: Option<u64>,
}

#[derive(Deserialize)]
struct ControlApiRuntimeRollbackPayload {
    target_generation: u64,
    requested_by: Option<String>,
    reason: Option<String>,
    expected_active_generation: Option<u64>,
}

enum ControlApiActivationError {
    Response(Box<Response<Full<Bytes>>>),
    Activation(Box<ActivationResult>),
}

const MAX_CONTROL_API_JSON_BODY_BYTES: usize = 64 * 1024;

impl QUICListener {
    pub(super) fn apply_live_log_level_reload(
        current_level: &str,
        next_level: &str,
    ) -> Result<bool, impulse_utils::logger::LogLevelError> {
        if current_level == next_level {
            return Ok(false);
        }

        impulse_utils::logger::set_log_level(next_level)?;
        Ok(true)
    }

    pub(super) fn reload_listener_certs(
        listener_runtime_configs: &HashMap<String, ListenerRuntimeConfig>,
        listener_tls_store: &ListenerTlsReloadStore,
        metrics: &Metrics,
    ) -> Response<Full<Bytes>> {
        let mut staged = Vec::with_capacity(listener_runtime_configs.len());
        let mut failures = Vec::new();
        for (listener_label, listener_config) in listener_runtime_configs {
            let reloaded_state = match Self::build_listener_tls_reload_state(listener_config) {
                Ok(mut state) => {
                    state.last_reload_status = "cert_reload_applied".to_string();
                    state
                }
                Err(err) => {
                    failures.push(json!({
                        "listener": listener_label,
                        "status": "failed",
                        "reason": "listener_tls_invalid",
                        "error": err.to_string(),
                    }));
                    continue;
                }
            };
            staged.push((listener_label.clone(), reloaded_state));
        }

        if !failures.is_empty() {
            metrics.record_control_plane_cert_reload("failed", "cert_reload_failed");
            metrics.record_secret_reload("listeners", "failed", "cert_reload_failed");
            return Self::json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({
                    "reloaded": false,
                    "reason": "cert_reload_failed",
                    "listeners": failures,
                }),
            );
        }

        let generations = match listener_tls_store.replace_listeners(&staged) {
            Ok(generations) => generations,
            Err(err) => {
                metrics.record_control_plane_cert_reload("failed", "cert_reload_failed");
                metrics.record_secret_reload("listeners", "failed", "cert_reload_failed");
                return Self::json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({
                        "reloaded": false,
                        "reason": "cert_reload_failed",
                        "error": err.to_string(),
                    }),
                );
            }
        };

        let mut reloaded = Vec::with_capacity(staged.len());
        let mut last_loaded_at_unix_ms = 0u64;
        for (listener_label, reloaded_state) in staged {
            Self::update_listener_tls_expiry_metrics(
                metrics,
                &listener_label,
                &reloaded_state.inventory,
            );
            last_loaded_at_unix_ms = last_loaded_at_unix_ms.max(reloaded_state.loaded_at_unix_ms);
            reloaded.push(json!({
                "listener": listener_label,
                "status": reloaded_state.last_reload_status,
                "loaded_at_unix_ms": reloaded_state.loaded_at_unix_ms,
                "generation": generations.get(&listener_label).copied().unwrap_or(0),
                "default_cert_not_after_unix_seconds": reloaded_state
                    .inventory
                    .default_identity
                    .metadata
                    .not_after_unix_seconds,
            }));
        }
        if last_loaded_at_unix_ms > 0 {
            metrics.set_secret_last_success_unixtime("listeners", last_loaded_at_unix_ms / 1_000);
        }
        metrics.record_control_plane_cert_reload("success", "cert_reload_applied");
        metrics.record_secret_reload("listeners", "success", "cert_reload_applied");

        Self::json_response(
            StatusCode::ACCEPTED,
            json!({
                "reloaded": true,
                "reason": "cert_reload_applied",
                "listener_count": reloaded.len(),
                "listeners": reloaded,
            }),
        )
    }

    pub(super) fn handle_control_api_reload_certs(
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
        identity: Option<AdminIdentity>,
        request_context: Option<ControlApiRequestContext>,
    ) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let active_generation = runtime_state
            .generation
            .as_ref()
            .map(|current| current.generation());
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            AdminAuditEventType::CertReload,
            AdminAuditAction::CertReloadAttempt,
            Self::control_api_audit_target_for_route(ControlApiRoute::ReloadCerts, None),
            AdminAuditGeneration {
                active_generation,
                ..Default::default()
            },
            AdminAuditResult::Success,
            Some("requested".to_string()),
        );
        let live_tls_store = runtime_state.listener_tls_store();
        let live_listener_configs = runtime_state.listener_runtime_configs();
        let live_metrics = runtime_state.metrics();
        let response = Self::reload_listener_certs(
            live_listener_configs.as_ref(),
            live_tls_store.as_ref(),
            live_metrics.as_ref(),
        );
        let succeeded = response.status().is_success();
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            AdminAuditEventType::CertReload,
            if succeeded {
                AdminAuditAction::CertReloadApplied
            } else {
                AdminAuditAction::CertReloadResult
            },
            Self::control_api_audit_target_for_route(ControlApiRoute::ReloadCerts, None),
            AdminAuditGeneration {
                active_generation,
                ..Default::default()
            },
            if succeeded {
                AdminAuditResult::Success
            } else {
                AdminAuditResult::Failed
            },
            Some(if succeeded {
                "cert_reload_applied".to_string()
            } else {
                "cert_reload_failed".to_string()
            }),
        );
        response
    }

    pub(super) async fn handle_control_api_runtime_validate(
        req: Request<Incoming>,
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
    ) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let identity = req.extensions().get::<AdminIdentity>().cloned();
        let request_context = req.extensions().get::<ControlApiRequestContext>().cloned();
        let Some(runtime_bundle_handle) = runtime_state.runtime_bundle_handle().cloned() else {
            return Self::control_api_not_found_response();
        };
        let current = runtime_bundle_handle.current_view();
        let plan_request =
            match Self::control_api_json_body_or_default::<ControlApiRuntimePlanRequest>(req).await
            {
                Ok(payload) => payload,
                Err(response) => {
                    Self::emit_control_api_audit_event(
                        &runtime_state.security,
                        identity.as_ref(),
                        request_context.as_ref(),
                        AdminAuditEventType::RuntimeValidate,
                        AdminAuditAction::RuntimeValidateResult,
                        Self::control_api_audit_target_for_route(
                            ControlApiRoute::RuntimeValidate,
                            None,
                        ),
                        AdminAuditGeneration {
                            active_generation: Some(current.generation()),
                            ..Default::default()
                        },
                        AdminAuditResult::Failed,
                        Some("invalid_request_body".to_string()),
                    );
                    return *response;
                }
            };
        let activation_request = Self::control_api_activation_request(
            &plan_request,
            current.generation(),
            "runtime_validate",
        );
        let reload_input =
            Self::control_api_reload_config_input(&current, plan_request.config_path);
        let config_path = Some(reload_input.source_label().to_string());
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            AdminAuditEventType::RuntimeValidate,
            AdminAuditAction::RuntimeValidateAttempt,
            Self::control_api_audit_target_for_route(
                ControlApiRoute::RuntimeValidate,
                config_path.clone(),
            ),
            AdminAuditGeneration {
                active_generation: Some(current.generation()),
                ..Default::default()
            },
            AdminAuditResult::Success,
            activation_request.reason.clone(),
        );
        Self::record_control_api_plan_attempt(
            &runtime_bundle_handle,
            "validate",
            &activation_request,
            &reload_input,
        );
        let plan = RuntimeActivationService::validate_reload(
            &runtime_bundle_handle,
            activation_request,
            reload_input,
        );
        Self::record_control_api_plan_result(&runtime_bundle_handle, "validate", &plan);
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            AdminAuditEventType::RuntimeValidate,
            AdminAuditAction::RuntimeValidateResult,
            Self::control_api_audit_target_for_route(ControlApiRoute::RuntimeValidate, config_path),
            AdminAuditGeneration {
                active_generation: Some(current.generation()),
                candidate_generation: Some(plan.candidate_generation),
                ..Default::default()
            },
            if plan.primary_rejection_reason().is_some() {
                AdminAuditResult::Failed
            } else {
                AdminAuditResult::Success
            },
            plan.primary_rejection_reason()
                .map(|reason| reason.slug().to_string()),
        );
        Self::json_response(StatusCode::OK, plan)
    }

    pub(super) async fn handle_control_api_runtime_preview(
        req: Request<Incoming>,
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
    ) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let identity = req.extensions().get::<AdminIdentity>().cloned();
        let request_context = req.extensions().get::<ControlApiRequestContext>().cloned();
        let Some(runtime_bundle_handle) = runtime_state.runtime_bundle_handle().cloned() else {
            return Self::control_api_not_found_response();
        };
        let current = runtime_bundle_handle.current_view();
        let plan_request = match Self::control_api_json_body_or_default::<
            ControlApiRuntimePlanRequest,
        >(req)
        .await
        {
            Ok(payload) => payload,
            Err(response) => {
                Self::emit_control_api_audit_event(
                    &runtime_state.security,
                    identity.as_ref(),
                    request_context.as_ref(),
                    AdminAuditEventType::RuntimePreview,
                    AdminAuditAction::RuntimePreviewResult,
                    Self::control_api_audit_target_for_route(ControlApiRoute::RuntimePreview, None),
                    AdminAuditGeneration {
                        active_generation: Some(current.generation()),
                        ..Default::default()
                    },
                    AdminAuditResult::Failed,
                    Some("invalid_request_body".to_string()),
                );
                return *response;
            }
        };
        let activation_request = Self::control_api_activation_request(
            &plan_request,
            current.generation(),
            "runtime_preview",
        );
        let reload_input =
            Self::control_api_reload_config_input(&current, plan_request.config_path);
        let config_path = Some(reload_input.source_label().to_string());
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            AdminAuditEventType::RuntimePreview,
            AdminAuditAction::RuntimePreviewAttempt,
            Self::control_api_audit_target_for_route(
                ControlApiRoute::RuntimePreview,
                config_path.clone(),
            ),
            AdminAuditGeneration {
                active_generation: Some(current.generation()),
                ..Default::default()
            },
            AdminAuditResult::Success,
            activation_request.reason.clone(),
        );
        Self::record_control_api_plan_attempt(
            &runtime_bundle_handle,
            "preview",
            &activation_request,
            &reload_input,
        );
        let plan = RuntimeActivationService::preview_reload(
            &runtime_bundle_handle,
            activation_request,
            reload_input,
        );
        Self::record_control_api_plan_result(&runtime_bundle_handle, "preview", &plan);
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            AdminAuditEventType::RuntimePreview,
            AdminAuditAction::RuntimePreviewResult,
            Self::control_api_audit_target_for_route(ControlApiRoute::RuntimePreview, config_path),
            AdminAuditGeneration {
                active_generation: Some(current.generation()),
                candidate_generation: Some(plan.candidate_generation),
                ..Default::default()
            },
            if plan.primary_rejection_reason().is_some() {
                AdminAuditResult::Failed
            } else {
                AdminAuditResult::Success
            },
            plan.primary_rejection_reason()
                .map(|reason| reason.slug().to_string()),
        );
        Self::json_response(StatusCode::OK, plan)
    }

    pub(super) async fn handle_control_api_runtime_activate(
        req: Request<Incoming>,
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
    ) -> Response<Full<Bytes>> {
        match Self::perform_control_api_runtime_activation(
            req,
            state,
            ControlApiRoute::RuntimeActivate,
            AdminAuditEventType::RuntimeActivate,
            AdminAuditAction::RuntimeActivateAttempt,
            AdminAuditAction::RuntimeActivateResult,
            "runtime_activate",
        )
        .await
        {
            Ok(activation) => Self::json_response(StatusCode::ACCEPTED, activation),
            Err(ControlApiActivationError::Response(response)) => *response,
            Err(ControlApiActivationError::Activation(activation)) => Self::json_response(
                activation_result_status(&activation),
                activation_error_payload(&activation, activation_error(&activation)),
            ),
        }
    }

    pub(super) async fn handle_control_api_runtime_reload(
        req: Request<Incoming>,
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
    ) -> Response<Full<Bytes>> {
        match Self::perform_control_api_runtime_activation(
            req,
            state,
            ControlApiRoute::ReloadRuntime,
            AdminAuditEventType::RuntimeReload,
            AdminAuditAction::RuntimeReloadAttempt,
            AdminAuditAction::RuntimeReloadResult,
            "runtime_reload",
        )
        .await
        {
            Ok(activation) => {
                let Some(generation) = activation.activated_generation else {
                    return Self::json_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({
                            "reloaded": false,
                            "error": "activation succeeded without an activated generation",
                        }),
                    );
                };
                Self::json_response(
                    StatusCode::ACCEPTED,
                    json!({
                        "reloaded": true,
                        "generation": generation,
                        "candidate_generation": activation.history_entry.generation,
                        "status": activation.status,
                    }),
                )
            }
            Err(ControlApiActivationError::Response(response)) => *response,
            Err(ControlApiActivationError::Activation(activation)) => Self::json_response(
                legacy_reload_result_status(&activation),
                legacy_reload_error_payload(&activation, activation_error(&activation)),
            ),
        }
    }

    #[cfg(test)]
    pub(super) async fn handle_control_api_runtime_reload_without_body_for_tests(
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
        identity: Option<AdminIdentity>,
        request_context: Option<ControlApiRequestContext>,
    ) -> Response<Full<Bytes>> {
        match Self::perform_control_api_runtime_activation_from_plan_request(
            ControlApiRuntimePlanRequest::default(),
            state,
            identity,
            request_context,
            ControlApiRoute::ReloadRuntime,
            AdminAuditEventType::RuntimeReload,
            AdminAuditAction::RuntimeReloadAttempt,
            AdminAuditAction::RuntimeReloadResult,
            "runtime_reload",
        )
        .await
        {
            Ok(activation) => {
                let Some(generation) = activation.activated_generation else {
                    return Self::json_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({
                            "reloaded": false,
                            "error": "activation succeeded without an activated generation",
                        }),
                    );
                };
                Self::json_response(
                    StatusCode::ACCEPTED,
                    json!({
                        "reloaded": true,
                        "generation": generation,
                        "candidate_generation": activation.history_entry.generation,
                        "status": activation.status,
                    }),
                )
            }
            Err(ControlApiActivationError::Response(response)) => *response,
            Err(ControlApiActivationError::Activation(activation)) => Self::json_response(
                legacy_reload_result_status(&activation),
                legacy_reload_error_payload(&activation, activation_error(&activation)),
            ),
        }
    }

    async fn perform_control_api_runtime_activation(
        req: Request<Incoming>,
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
        route: ControlApiRoute,
        event_type: AdminAuditEventType,
        attempt_action: AdminAuditAction,
        result_action: AdminAuditAction,
        default_reason: &str,
    ) -> Result<ActivationResult, ControlApiActivationError> {
        let runtime_state = state.current_service_state();
        let identity = req.extensions().get::<AdminIdentity>().cloned();
        let request_context = req.extensions().get::<ControlApiRequestContext>().cloned();
        let plan_request =
            Self::control_api_json_body_or_default::<ControlApiRuntimePlanRequest>(req)
                .await
                .map_err(|response| {
                    Self::emit_control_api_audit_event(
                        &runtime_state.security,
                        identity.as_ref(),
                        request_context.as_ref(),
                        event_type,
                        result_action,
                        Self::control_api_audit_target_for_route(route, None),
                        AdminAuditGeneration {
                            active_generation: runtime_state
                                .generation
                                .as_ref()
                                .map(|current| current.generation()),
                            ..Default::default()
                        },
                        AdminAuditResult::Failed,
                        Some("invalid_request_body".to_string()),
                    );
                    ControlApiActivationError::Response(response)
                })?;
        Self::perform_control_api_runtime_activation_from_plan_request(
            plan_request,
            state,
            identity,
            request_context,
            route,
            event_type,
            attempt_action,
            result_action,
            default_reason,
        )
        .await
    }

    // Mirrors `perform_control_api_runtime_activation`; the extra parameters are the
    // audit descriptor threaded through to the emitter.
    #[allow(clippy::too_many_arguments)]
    async fn perform_control_api_runtime_activation_from_plan_request(
        plan_request: ControlApiRuntimePlanRequest,
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
        identity: Option<AdminIdentity>,
        request_context: Option<ControlApiRequestContext>,
        route: ControlApiRoute,
        event_type: AdminAuditEventType,
        attempt_action: AdminAuditAction,
        result_action: AdminAuditAction,
        default_reason: &str,
    ) -> Result<ActivationResult, ControlApiActivationError> {
        let runtime_state = state.current_service_state();
        let Some(runtime_bundle_handle) = runtime_state.runtime_bundle_handle().cloned() else {
            return Err(ControlApiActivationError::Response(Box::new(
                Self::control_api_not_found_response(),
            )));
        };
        let Some(runtime) = runtime_state.generation.clone() else {
            return Err(ControlApiActivationError::Response(Box::new(
                Self::json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({
                        "reloaded": false,
                        "error": "runtime generation unavailable",
                    }),
                ),
            )));
        };
        let activation_request = Self::control_api_activation_request(
            &plan_request,
            runtime.generation(),
            default_reason,
        );
        let reload_input =
            Self::control_api_reload_config_input(&runtime, plan_request.config_path);
        let config_path = Some(reload_input.source_label().to_string());
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            event_type,
            attempt_action,
            Self::control_api_audit_target_for_route(route, config_path.clone()),
            AdminAuditGeneration {
                active_generation: Some(runtime.generation()),
                ..Default::default()
            },
            AdminAuditResult::Success,
            activation_request.reason.clone(),
        );
        Self::record_control_api_activation_attempt(
            &runtime_bundle_handle,
            &activation_request,
            &reload_input,
        );
        let current_log_level = runtime.startup().log_config.level.clone();
        let activation = RuntimeActivationService::activate_reload(
            &runtime_bundle_handle,
            activation_request,
            reload_input,
        );
        Self::record_control_api_activation_outcome(&runtime_bundle_handle, &activation);
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            event_type,
            result_action,
            Self::control_api_audit_target_for_route(route, config_path.clone()),
            AdminAuditGeneration {
                active_generation: Some(activation.active_generation),
                candidate_generation: Some(activation.history_entry.generation),
                target_generation: activation.activated_generation,
            },
            if activation.succeeded() {
                AdminAuditResult::Success
            } else {
                AdminAuditResult::Failed
            },
            Some(control_api_activation_reason(&activation)),
        );
        if let Some((action, result, reason)) = upstream_mtls_material_audit_event(&activation) {
            Self::emit_control_api_audit_event(
                &runtime_state.security,
                identity.as_ref(),
                request_context.as_ref(),
                AdminAuditEventType::UpstreamMtlsMaterial,
                action,
                Self::control_api_audit_target_for_route(route, config_path),
                AdminAuditGeneration {
                    active_generation: Some(activation.active_generation),
                    candidate_generation: Some(activation.history_entry.generation),
                    target_generation: activation.activated_generation,
                },
                result,
                Some(reason),
            );
        }
        if !activation.succeeded() {
            return Err(ControlApiActivationError::Activation(Box::new(activation)));
        }
        let Some(generation) = activation.activated_generation else {
            return Err(ControlApiActivationError::Response(Box::new(
                Self::json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({
                        "reloaded": false,
                        "error": "activation succeeded without an activated generation",
                    }),
                ),
            )));
        };
        let next_log_level = runtime_bundle_handle
            .current_view()
            .startup()
            .log_config
            .level
            .clone();
        if let Err(err) = Self::apply_live_log_level_reload(&current_log_level, &next_log_level) {
            error!(
                "Runtime reload applied generation={} but failed to update live log.level from '{}' to '{}': {}",
                generation, current_log_level, next_log_level, err
            );
        }
        Ok(activation)
    }

    pub(super) async fn handle_control_api_runtime_rollback(
        req: Request<Incoming>,
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
    ) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let identity = req.extensions().get::<AdminIdentity>().cloned();
        let request_context = req.extensions().get::<ControlApiRequestContext>().cloned();
        let Some(runtime_bundle_handle) = runtime_state.runtime_bundle_handle().cloned() else {
            return Self::control_api_not_found_response();
        };
        let payload =
            match Self::control_api_json_body::<ControlApiRuntimeRollbackPayload>(req).await {
                Ok(payload) => payload,
                Err(response) => {
                    Self::emit_control_api_audit_event(
                        &runtime_state.security,
                        identity.as_ref(),
                        request_context.as_ref(),
                        AdminAuditEventType::RuntimeRollback,
                        AdminAuditAction::RuntimeRollbackResult,
                        Self::control_api_audit_target_for_route(
                            ControlApiRoute::RuntimeRollback,
                            None,
                        ),
                        AdminAuditGeneration::default(),
                        AdminAuditResult::Failed,
                        Some("invalid_request_body".to_string()),
                    );
                    return *response;
                }
            };
        let current_generation = runtime_bundle_handle.current_generation();
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            AdminAuditEventType::RuntimeRollback,
            AdminAuditAction::RuntimeRollbackAttempt,
            Self::control_api_audit_target_for_route(ControlApiRoute::RuntimeRollback, None),
            AdminAuditGeneration {
                active_generation: Some(current_generation),
                target_generation: Some(payload.target_generation),
                ..Default::default()
            },
            AdminAuditResult::Success,
            payload
                .reason
                .clone()
                .or_else(|| Some("runtime_rollback".to_string())),
        );
        let rollback = RuntimeActivationService::rollback_generation(
            &runtime_bundle_handle,
            RollbackRequest {
                target_generation: payload.target_generation,
                requested_by: payload
                    .requested_by
                    .or_else(|| Some("control_api".to_string())),
                trigger_source: Some("control_api".to_string()),
                reason: payload
                    .reason
                    .or_else(|| Some("runtime_rollback".to_string())),
                expected_active_generation: payload.expected_active_generation,
                requested_at_ms: crate::watchdog::time::now_millis(),
            },
        );
        Self::record_control_api_rollback_outcome(&runtime_bundle_handle, &rollback);
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            AdminAuditEventType::RuntimeRollback,
            AdminAuditAction::RuntimeRollbackResult,
            Self::control_api_audit_target_for_route(ControlApiRoute::RuntimeRollback, None),
            AdminAuditGeneration {
                active_generation: Some(rollback.active_generation),
                target_generation: Some(rollback.request.target_generation),
                candidate_generation: rollback.rolled_back_to,
            },
            if rollback.succeeded() {
                AdminAuditResult::Success
            } else {
                AdminAuditResult::Failed
            },
            Some(
                rollback
                    .primary_rejection_reason()
                    .map(|reason| reason.slug().to_string())
                    .unwrap_or_else(|| rollback.outcome_reason().slug().to_string()),
            ),
        );
        if !rollback.succeeded() {
            return Self::json_response(
                rollback_result_status(&rollback),
                rollback_error_payload(&rollback, rollback_error(&rollback)),
            );
        }
        Self::json_response(StatusCode::ACCEPTED, rollback)
    }

    pub(super) fn handle_control_api_restart(
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
        identity: Option<AdminIdentity>,
        request_context: Option<ControlApiRequestContext>,
    ) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let active_generation = runtime_state
            .generation
            .as_ref()
            .map(|current| current.generation());
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            AdminAuditEventType::RuntimeRestart,
            AdminAuditAction::RuntimeRestartAttempt,
            Self::control_api_audit_target_for_route(ControlApiRoute::Restart, None),
            AdminAuditGeneration {
                active_generation,
                ..Default::default()
            },
            AdminAuditResult::Success,
            Some("requested".to_string()),
        );
        let watchdog = runtime_state.watchdog();
        if !watchdog.enabled() {
            Self::emit_control_api_audit_event(
                &runtime_state.security,
                identity.as_ref(),
                request_context.as_ref(),
                AdminAuditEventType::RuntimeRestart,
                AdminAuditAction::RuntimeRestartResult,
                Self::control_api_audit_target_for_route(ControlApiRoute::Restart, None),
                AdminAuditGeneration {
                    active_generation,
                    ..Default::default()
                },
                AdminAuditResult::Failed,
                Some("watchdog_disabled".to_string()),
            );
            return Self::json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({
                    "accepted": false,
                    "error": "watchdog disabled",
                }),
            );
        }

        let accepted = watchdog.request_restart("admin_runtime_api");
        Self::emit_control_api_audit_event(
            &runtime_state.security,
            identity.as_ref(),
            request_context.as_ref(),
            AdminAuditEventType::RuntimeRestart,
            AdminAuditAction::RuntimeRestartResult,
            Self::control_api_audit_target_for_route(ControlApiRoute::Restart, None),
            AdminAuditGeneration {
                active_generation,
                ..Default::default()
            },
            if accepted {
                AdminAuditResult::Success
            } else {
                AdminAuditResult::Failed
            },
            Some(if accepted {
                "restart_requested".to_string()
            } else {
                "restart_pending_or_cooldown_active".to_string()
            }),
        );
        Self::json_response(
            if accepted {
                StatusCode::ACCEPTED
            } else {
                StatusCode::CONFLICT
            },
            json!({
                "accepted": accepted,
                "restart_requested": watchdog.restart_requested(),
                "reason": if accepted { "admin_runtime_api" } else { "restart pending or cooldown active" },
            }),
        )
    }

    /// Evaluate reload compatibility, returning typed rejections.
    ///
    /// The check order and short-circuit behavior are preserved from the pre-Phase-2
    /// call sites: listener/bind, then control API, then metrics each surface the
    /// first rejection they find; only if all three are compatible are the
    /// startup-owned field changes collected together. The per-domain *rules* and
    /// *wording* now come from the central [`ReloadCompatibilityAuthority`].
    pub(crate) fn evaluate_runtime_reload_compatibility(
        current: &ActiveRuntimeGeneration,
        next: &RuntimeBundle,
    ) -> Result<(), Vec<TransitionRejection>> {
        if let Some(rejection) = Self::validate_runtime_reload_compatibility(current.bundle(), next)
        {
            return Err(vec![rejection]);
        }
        if let Some(rejection) =
            Self::validate_control_api_reload_compatibility(current.bundle(), next)
        {
            return Err(vec![rejection]);
        }
        if let Some(rejection) = Self::validate_metrics_reload_compatibility(current.bundle(), next)
        {
            return Err(vec![rejection]);
        }
        Self::validate_startup_owned_reload_compatibility(current.bundle(), next)
    }

    pub(super) fn validate_runtime_reload_compatibility(
        current: &RuntimeBundle,
        next: &RuntimeBundle,
    ) -> Option<TransitionRejection> {
        for label in current
            .shared_state
            .generation_state()
            .listener_runtime_configs
            .keys()
        {
            if !next
                .shared_state
                .generation_state()
                .listener_runtime_configs
                .contains_key(label)
            {
                return Some(TransitionRejection::listener_bind_changed(label));
            }
        }

        let worker_count = next.runtime_config.performance.worker_threads.max(1);
        for (label, listener_config) in next
            .shared_state
            .generation_state()
            .listener_runtime_configs
            .iter()
        {
            if current
                .shared_state
                .generation_state()
                .listener_runtime_configs
                .contains_key(label)
            {
                continue;
            }
            if worker_count > 1 {
                if let Err(err) = Self::bind_reuseport_sockets(listener_config, worker_count) {
                    return Some(TransitionRejection::resource_preflight_failed(
                        "QUIC listener",
                        label,
                        err.to_string(),
                    ));
                }
            } else if let Err(err) = Self::bind_socket(listener_config, false) {
                return Some(TransitionRejection::resource_preflight_failed(
                    "QUIC listener",
                    label,
                    err.to_string(),
                ));
            }

            let bind = format!(
                "{}:{}",
                listener_config.listen.listen.address, listener_config.listen.listen.port
            );
            if let Err(err) = Self::probe_tcp_bind(&bind, "bootstrap TLS listener") {
                return Some(TransitionRejection::resource_preflight_failed(
                    "bootstrap TLS listener",
                    label,
                    err,
                ));
            }
        }
        None
    }

    pub(super) fn validate_control_api_reload_compatibility(
        current: &RuntimeBundle,
        next: &RuntimeBundle,
    ) -> Option<TransitionRejection> {
        let next_control_api = &next.runtime_config.observability.control_api;
        if !next_control_api.enabled {
            return None;
        }

        let Some(listener_config) = next.runtime_config.primary_listener_runtime_config() else {
            return Some(TransitionRejection::raw_resource_message(
                "runtime reload rejected: no effective listeners configured for control API TLS",
            ));
        };
        let primary_listener_label = Self::listener_label(&listener_config);
        if next
            .shared_state
            .shared_services()
            .listener_tls_store
            .bootstrap_server_config(&primary_listener_label)
            .is_none()
        {
            return Some(TransitionRejection::raw_resource_message(format!(
                "runtime reload rejected: control API TLS config missing for listener '{}'",
                primary_listener_label
            )));
        }

        let current_control_api = &current.runtime_config.observability.control_api;
        let bind_changed = !current_control_api.enabled
            || current_control_api.address != next_control_api.address
            || current_control_api.port != next_control_api.port;
        if bind_changed {
            let bind = format!("{}:{}", next_control_api.address, next_control_api.port);
            if let Err(err) = Self::probe_tcp_bind(&bind, "control API endpoint") {
                return Some(TransitionRejection::resource_preflight_failed(
                    "control API endpoint",
                    bind,
                    err,
                ));
            }
        }
        None
    }

    pub(super) fn validate_metrics_reload_compatibility(
        current: &RuntimeBundle,
        next: &RuntimeBundle,
    ) -> Option<TransitionRejection> {
        let next_metrics = &next.runtime_config.observability.metrics;
        if !next_metrics.enabled {
            return None;
        }

        let current_metrics = &current.runtime_config.observability.metrics;
        let bind_changed = !current_metrics.enabled
            || current_metrics.address != next_metrics.address
            || current_metrics.port != next_metrics.port;
        if bind_changed {
            let bind = format!("{}:{}", next_metrics.address, next_metrics.port);
            if let Err(err) = Self::probe_tcp_bind(&bind, "metrics endpoint") {
                return Some(TransitionRejection::resource_preflight_failed(
                    "metrics endpoint",
                    bind,
                    err,
                ));
            }
        }
        None
    }

    /// Collect every restart-required (startup-owned) field change as a typed
    /// rejection. Uses the central [`ReloadCompatibilityAuthority`] so the rule
    /// (restart-required) and wording live in one place; the set of fields checked
    /// here matches the `RESOURCE_DOMAINS` rows marked restart-required.
    pub(super) fn validate_startup_owned_reload_compatibility(
        current: &RuntimeBundle,
        next: &RuntimeBundle,
    ) -> Result<(), Vec<TransitionRejection>> {
        let mut authority = ReloadCompatibilityAuthority::new();

        authority.note_restart_required_change(
            "log.file.enabled",
            &current.startup.log_config.file.enabled,
            &next.startup.log_config.file.enabled,
        );
        authority.note_restart_required_change(
            "log.file.path",
            &current.startup.log_config.file.path,
            &next.startup.log_config.file.path,
        );
        authority.note_restart_required_change(
            "log.format",
            &current.startup.log_config.format,
            &next.startup.log_config.format,
        );

        let current_tracing = &current.runtime_config.observability.tracing;
        let next_tracing = &next.runtime_config.observability.tracing;
        authority.note_restart_required_change(
            "observability.tracing.enabled",
            &current_tracing.enabled,
            &next_tracing.enabled,
        );
        authority.note_restart_required_change(
            "observability.tracing.service_name",
            &current_tracing.service_name,
            &next_tracing.service_name,
        );
        authority.note_restart_required_change(
            "observability.tracing.otlp_endpoint",
            &current_tracing.otlp_endpoint,
            &next_tracing.otlp_endpoint,
        );
        authority.note_restart_required_change(
            "observability.tracing.sample_ratio",
            &current_tracing.sample_ratio,
            &next_tracing.sample_ratio,
        );

        let current_perf = &current.runtime_config.performance;
        let next_perf = &next.runtime_config.performance;
        authority.note_restart_required_change(
            "performance.control_plane_threads",
            &current_perf.control_plane_threads,
            &next_perf.control_plane_threads,
        );

        authority.into_result()
    }
}

impl QUICListener {
    async fn control_api_json_body<T>(
        req: Request<Incoming>,
    ) -> Result<T, Box<Response<Full<Bytes>>>>
    where
        T: DeserializeOwned,
    {
        let body = Self::collect_control_api_json_body_bounded(req.into_body()).await?;
        if body.is_empty() {
            return Err(Box::new(Self::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "request body is required" }),
            )));
        }
        serde_json::from_slice(&body).map_err(|err| {
            Box::new(Self::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("invalid request body: {err}") }),
            ))
        })
    }

    async fn control_api_json_body_or_default<T>(
        req: Request<Incoming>,
    ) -> Result<T, Box<Response<Full<Bytes>>>>
    where
        T: DeserializeOwned + Default,
    {
        let body = Self::collect_control_api_json_body_bounded(req.into_body()).await?;
        if body.is_empty() {
            return Ok(T::default());
        }
        serde_json::from_slice(&body).map_err(|err| {
            Box::new(Self::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("invalid request body: {err}") }),
            ))
        })
    }

    async fn collect_control_api_json_body_bounded<B>(
        mut body: B,
    ) -> Result<Vec<u8>, Box<Response<Full<Bytes>>>>
    where
        B: Body<Data = Bytes> + Unpin,
        B::Error: std::fmt::Display,
    {
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = match frame {
                Ok(frame) => frame,
                Err(err) => {
                    return Err(Box::new(Self::json_response(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": format!("invalid request body: {err}") }),
                    )));
                }
            };
            let Ok(chunk) = frame.into_data() else {
                continue;
            };
            let next_len = bytes.len().saturating_add(chunk.len());
            if next_len > MAX_CONTROL_API_JSON_BODY_BYTES {
                return Err(Box::new(Self::json_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    json!({
                        "error": format!(
                            "request body exceeded {} bytes",
                            MAX_CONTROL_API_JSON_BODY_BYTES
                        )
                    }),
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    fn control_api_reload_config_input(
        current: &ActiveRuntimeGeneration,
        config_path: Option<String>,
    ) -> ReloadConfigInput {
        ReloadConfigInput::Path {
            path: config_path.unwrap_or_else(|| current.startup().config_path.clone()),
        }
    }

    fn control_api_activation_request(
        payload: &ControlApiRuntimePlanRequest,
        current_generation: u64,
        default_reason: &str,
    ) -> ActivationRequest {
        ActivationRequest {
            requested_by: payload
                .requested_by
                .clone()
                .or_else(|| Some("control_api".to_string())),
            trigger_source: Some("control_api".to_string()),
            reason: payload
                .reason
                .clone()
                .or_else(|| Some(default_reason.to_string())),
            expected_generation: payload.expected_generation.or(Some(current_generation)),
            requested_at_ms: crate::watchdog::time::now_millis(),
        }
    }

    fn record_control_api_plan_attempt(
        handle: &RuntimeBundleHandle,
        operation: &str,
        request: &ActivationRequest,
        input: &ReloadConfigInput,
    ) {
        let current = handle.current_view();
        let metrics = Arc::clone(&current.shared_services().metrics);
        match operation {
            "validate" => metrics.inc_runtime_validation_attempt(),
            "preview" => metrics.inc_runtime_preview_attempt(),
            _ => {}
        }
        info!(
            "runtime {} requested active_generation={} expected_generation={:?} config_source={} requested_by={:?} trigger_source={:?}",
            operation,
            current.generation(),
            request.expected_generation,
            input.source_label(),
            request.requested_by,
            request.trigger_source,
        );
    }

    fn record_control_api_plan_result(
        handle: &RuntimeBundleHandle,
        operation: &str,
        plan: &ReloadPlan,
    ) {
        if let Some(reason) = plan.primary_rejection_reason() {
            let current = handle.current_view();
            let metrics = Arc::clone(&current.shared_services().metrics);
            metrics.inc_runtime_rejection_reason(reason);
            warn!(
                "runtime {} rejected generation={} reason={} summary={}",
                operation,
                plan.candidate_generation,
                reason.slug(),
                plan.rejection_summary
                    .as_deref()
                    .unwrap_or(plan.summary.as_str())
            );
        } else {
            info!(
                "runtime {} accepted candidate_generation={} summary={}",
                operation, plan.candidate_generation, plan.summary
            );
        }
    }

    fn record_control_api_activation_attempt(
        handle: &RuntimeBundleHandle,
        request: &ActivationRequest,
        input: &ReloadConfigInput,
    ) {
        let current = handle.current_view();
        info!(
            "runtime activation requested active_generation={} expected_generation={:?} config_source={} requested_by={:?} trigger_source={:?}",
            current.generation(),
            request.expected_generation,
            input.source_label(),
            request.requested_by,
            request.trigger_source,
        );
    }

    fn record_control_api_activation_outcome(
        handle: &RuntimeBundleHandle,
        activation: &ActivationResult,
    ) {
        let current = handle.current_view();
        let metrics = Arc::clone(&current.shared_services().metrics);
        let outcome = activation.outcome_reason();
        metrics.record_runtime_activation_outcome(outcome);
        record_control_api_secret_metrics(current.runtime_config(), metrics.as_ref(), activation);
        if let Some(reason) = activation.primary_rejection_reason() {
            metrics.inc_runtime_rejection_reason(reason);
            warn!(
                "runtime activation rejected active_generation={} candidate_generation={} reason={} error={}",
                activation.active_generation,
                activation.history_entry.generation,
                outcome.slug(),
                activation_error(activation)
            );
        } else if let Some(activated_generation) = activation.activated_generation {
            info!(
                "runtime activation succeeded active_generation={} activated_generation={} reason={}",
                activation.active_generation,
                activated_generation,
                outcome.slug()
            );
        }
    }

    fn record_control_api_rollback_outcome(
        handle: &RuntimeBundleHandle,
        rollback: &RollbackResult,
    ) {
        let current = handle.current_view();
        let metrics = Arc::clone(&current.shared_services().metrics);
        let outcome = rollback.outcome_reason();
        metrics.record_runtime_rollback_outcome(outcome);
        if let Some(reason) = rollback.primary_rejection_reason() {
            metrics.inc_runtime_rejection_reason(reason);
            warn!(
                "runtime rollback rejected active_generation={} target_generation={} reason={} error={}",
                rollback.active_generation,
                rollback.request.target_generation,
                outcome.slug(),
                rollback_error(rollback)
            );
        } else if let Some(rolled_back_to) = rollback.rolled_back_to {
            info!(
                "runtime rollback succeeded active_generation={} rolled_back_to={} reason={}",
                rollback.active_generation,
                rolled_back_to,
                outcome.slug()
            );
        }
    }
}

fn activation_result_status(activation: &ActivationResult) -> StatusCode {
    if activation
        .rejected_changes
        .iter()
        .any(|rejection| matches!(rejection.kind, RejectedChangeKind::InvalidConfiguration))
    {
        StatusCode::BAD_REQUEST
    } else if activation.rejected_changes.iter().any(|rejection| {
        matches!(
            rejection.kind,
            RejectedChangeKind::ResourcePreparationFailed
        )
    }) || matches!(
        activation.status,
        crate::runtime::activation::GenerationStatus::Failed
    ) {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::CONFLICT
    }
}

fn activation_error(activation: &ActivationResult) -> &str {
    activation
        .rejected_changes
        .first()
        .map(|rejection| rejection.message.as_str())
        .unwrap_or(activation.history_entry.summary.as_str())
}

fn activation_error_payload(activation: &ActivationResult, error: &str) -> serde_json::Value {
    json!({
        "error": error,
        "reason": control_api_activation_reason(activation),
        "rejection_reason": activation.primary_rejection_reason().map(RuntimeRejectionReason::slug),
        "active_generation": activation.active_generation,
        "candidate_generation": activation.history_entry.generation,
        "status": activation.status,
        "rejected_changes": activation.rejected_changes,
        "history_entry": activation.history_entry,
    })
}

fn legacy_reload_error_payload(activation: &ActivationResult, error: &str) -> serde_json::Value {
    json!({
        "reloaded": false,
        "error": error,
        "reason": control_api_activation_reason(activation),
        "rejection_reason": activation.primary_rejection_reason().map(RuntimeRejectionReason::slug),
        "generation": activation.active_generation,
        "candidate_generation": activation.history_entry.generation,
        "status": activation.status,
    })
}

fn record_control_api_secret_metrics(
    runtime_config: &impulse_config::runtime::RuntimeConfig,
    metrics: &Metrics,
    activation: &ActivationResult,
) {
    let reason = control_api_activation_reason(activation);
    if activation.succeeded() {
        if activation
            .history_entry
            .diff
            .entries
            .iter()
            .any(|entry| entry.secret_material_changed)
        {
            metrics.record_secret_reload("upstreams", "success", &reason);
            if let Some(last_loaded_at_unix_ms) = runtime_config
                .upstreams
                .values()
                .flat_map(|upstream| {
                    let policy = upstream.backend_tls_policy();
                    [
                        policy
                            .client_certificate
                            .as_ref()
                            .map(|metadata| metadata.loaded_at_unix_ms),
                        policy
                            .client_key
                            .as_ref()
                            .map(|metadata| metadata.loaded_at_unix_ms),
                    ]
                })
                .flatten()
                .max()
            {
                metrics
                    .set_secret_last_success_unixtime("upstreams", last_loaded_at_unix_ms / 1_000);
            }
        }
        return;
    }

    if matches!(
        classify_upstream_mtls_material_outcome(activation),
        Some(UpstreamMtlsMaterialOutcome::ResolutionFailed | UpstreamMtlsMaterialOutcome::Invalid)
    ) {
        metrics.record_secret_reload("upstreams", "failed", &reason);
    }
    if let Some((provider, resolve_reason)) =
        classify_secret_resolution_failure(activation_error(activation))
    {
        metrics.record_secret_resolve(provider, "failed", resolve_reason);
    }
}

/// Secret/cert-specific classification of an activation outcome, shared by
/// the operator-facing reason string and the dedicated audit event. Lets
/// operators identify secret rotations and failures without parsing
/// free-text diff summaries.
enum UpstreamMtlsMaterialOutcome {
    Changed,
    ResolutionFailed,
    Invalid,
}

impl UpstreamMtlsMaterialOutcome {
    fn slug(&self) -> &'static str {
        match self {
            Self::Changed => "upstream_mtls_material_changed",
            Self::ResolutionFailed => "secret_resolution_failed",
            Self::Invalid => "upstream_mtls_material_invalid",
        }
    }

    fn audit_action(&self) -> AdminAuditAction {
        match self {
            Self::Changed => AdminAuditAction::UpstreamMtlsMaterialChanged,
            Self::ResolutionFailed => AdminAuditAction::SecretResolutionFailed,
            Self::Invalid => AdminAuditAction::UpstreamMtlsMaterialInvalid,
        }
    }
}

fn classify_upstream_mtls_material_outcome(
    activation: &ActivationResult,
) -> Option<UpstreamMtlsMaterialOutcome> {
    if activation.succeeded() {
        return activation
            .history_entry
            .diff
            .entries
            .iter()
            .any(|entry| entry.secret_material_changed)
            .then_some(UpstreamMtlsMaterialOutcome::Changed);
    }

    let error = activation_error(activation);
    if error.contains("secret_resolution_failed") || error.contains("secret resolution failed") {
        return Some(UpstreamMtlsMaterialOutcome::ResolutionFailed);
    }
    if error.contains("tls_material_invalid")
        || error.contains("client_certificate")
        || error.contains("client_key")
    {
        return Some(UpstreamMtlsMaterialOutcome::Invalid);
    }
    None
}

fn classify_secret_resolution_failure(error: &str) -> Option<(&'static str, &str)> {
    if !error.contains("secret resolution failed") {
        return None;
    }

    let provider = if error.contains("file secret resolution failed") {
        "file"
    } else if error.contains("literal secret resolution failed") {
        "literal"
    } else {
        "unknown"
    };
    let reason = error.rsplit_once(':').map(|(_, reason)| reason.trim())?;
    if reason.is_empty() {
        None
    } else {
        Some((provider, reason))
    }
}

/// Emits a dedicated, precisely-typed audit event when this activation
/// touched secret-backed upstream TLS material (client cert/key, CA bundle),
/// alongside the generic runtime-activation audit event.
fn upstream_mtls_material_audit_event(
    activation: &ActivationResult,
) -> Option<(AdminAuditAction, AdminAuditResult, String)> {
    let outcome = classify_upstream_mtls_material_outcome(activation)?;
    let result = if activation.succeeded() {
        AdminAuditResult::Success
    } else {
        AdminAuditResult::Failed
    };
    Some((outcome.audit_action(), result, outcome.slug().to_string()))
}

fn control_api_activation_reason(activation: &ActivationResult) -> String {
    if let Some(outcome) = classify_upstream_mtls_material_outcome(activation) {
        return outcome.slug().to_string();
    }

    activation
        .primary_rejection_reason()
        .map(|reason| reason.slug().to_string())
        .unwrap_or_else(|| activation.outcome_reason().slug().to_string())
}

fn legacy_reload_result_status(activation: &ActivationResult) -> StatusCode {
    if activation
        .rejected_changes
        .iter()
        .any(|rejection| matches!(rejection.kind, RejectedChangeKind::InvalidConfiguration))
    {
        StatusCode::BAD_REQUEST
    } else if activation.rejected_changes.iter().any(|rejection| {
        matches!(
            rejection.kind,
            RejectedChangeKind::ResourcePreparationFailed
                | RejectedChangeKind::RuntimeStateUnavailable
        )
    }) || matches!(
        activation.status,
        crate::runtime::activation::GenerationStatus::Failed
    ) {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::CONFLICT
    }
}

fn rollback_result_status(rollback: &RollbackResult) -> StatusCode {
    if rollback
        .rejected_changes
        .iter()
        .any(|rejection| matches!(rejection.kind, RejectedChangeKind::InvalidConfiguration))
    {
        StatusCode::BAD_REQUEST
    } else if rollback.rejected_changes.iter().any(|rejection| {
        matches!(
            rejection.kind,
            RejectedChangeKind::ResourcePreparationFailed
        )
    }) || matches!(
        rollback.status,
        crate::runtime::activation::GenerationStatus::Failed
    ) {
        StatusCode::INTERNAL_SERVER_ERROR
    } else if rollback.rejected_changes.iter().any(|rejection| {
        rejection.kind == RejectedChangeKind::RuntimeStateUnavailable
            && rejection.reason == RuntimeRejectionReason::UnknownGeneration
    }) {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::CONFLICT
    }
}

fn rollback_error(rollback: &RollbackResult) -> &str {
    rollback
        .rejected_changes
        .first()
        .map(|rejection| rejection.message.as_str())
        .unwrap_or(rollback.history_entry.summary.as_str())
}

fn rollback_error_payload(rollback: &RollbackResult, error: &str) -> serde_json::Value {
    json!({
        "error": error,
        "rejection_reason": rollback.primary_rejection_reason().map(RuntimeRejectionReason::slug),
        "active_generation": rollback.active_generation,
        "target_generation": rollback.request.target_generation,
        "rolled_back_to": rollback.rolled_back_to,
        "status": rollback.status,
        "rejected_changes": rollback.rejected_changes,
        "history_entry": rollback.history_entry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::activation::{
        ActivationRequest, GenerationHistoryEntry, GenerationOperation, GenerationStatus,
        RejectedChange, RejectedChangeKind, ReloadDiff, RollbackRequest,
    };

    fn rejected_change(reason: RuntimeRejectionReason, kind: RejectedChangeKind) -> RejectedChange {
        RejectedChange {
            reason,
            kind,
            field_path: None,
            current_value: None,
            requested_value: None,
            operator_action: "retry".to_string(),
            active_generation_changed: false,
            message: "rejected".to_string(),
        }
    }

    fn history_entry(
        operation: GenerationOperation,
        status: GenerationStatus,
    ) -> GenerationHistoryEntry {
        GenerationHistoryEntry {
            generation: 2,
            operation,
            status,
            config_source: "config.yaml".to_string(),
            config_version: Some(1),
            requested_by: Some("test".to_string()),
            trigger_source: Some("unit_test".to_string()),
            requested_at_ms: 1,
            completed_at_ms: Some(2),
            summary: "summary".to_string(),
            diff: ReloadDiff::default(),
            rejected_changes: Vec::new(),
        }
    }

    #[test]
    fn activation_status_maps_stale_generation_conflicts_to_conflict() {
        let activation = ActivationResult {
            request: ActivationRequest {
                requested_by: Some("test".to_string()),
                trigger_source: Some("unit_test".to_string()),
                reason: Some("runtime_activate".to_string()),
                expected_generation: Some(1),
                requested_at_ms: 1,
            },
            active_generation: 3,
            activated_generation: None,
            status: GenerationStatus::Rejected,
            rejected_changes: vec![rejected_change(
                RuntimeRejectionReason::UnknownGeneration,
                RejectedChangeKind::IllegalTransition,
            )],
            history_entry: history_entry(GenerationOperation::Activate, GenerationStatus::Rejected),
        };

        assert_eq!(activation_result_status(&activation), StatusCode::CONFLICT);
        assert_eq!(
            legacy_reload_result_status(&activation),
            StatusCode::CONFLICT
        );
    }

    #[test]
    fn rollback_status_maps_missing_target_to_not_found() {
        let rollback = RollbackResult {
            request: RollbackRequest {
                target_generation: 9,
                requested_by: Some("test".to_string()),
                trigger_source: Some("unit_test".to_string()),
                reason: Some("runtime_rollback".to_string()),
                expected_active_generation: Some(3),
                requested_at_ms: 1,
            },
            active_generation: 3,
            rolled_back_to: None,
            status: GenerationStatus::Rejected,
            rejected_changes: vec![rejected_change(
                RuntimeRejectionReason::UnknownGeneration,
                RejectedChangeKind::RuntimeStateUnavailable,
            )],
            history_entry: history_entry(GenerationOperation::Rollback, GenerationStatus::Rejected),
        };

        assert_eq!(rollback_result_status(&rollback), StatusCode::NOT_FOUND);
    }

    #[test]
    fn rollback_status_keeps_other_operator_conflicts_as_conflict() {
        let rollback = RollbackResult {
            request: RollbackRequest {
                target_generation: 3,
                requested_by: Some("test".to_string()),
                trigger_source: Some("unit_test".to_string()),
                reason: Some("runtime_rollback".to_string()),
                expected_active_generation: Some(3),
                requested_at_ms: 1,
            },
            active_generation: 3,
            rolled_back_to: None,
            status: GenerationStatus::Rejected,
            rejected_changes: vec![rejected_change(
                RuntimeRejectionReason::RollbackNotAllowed,
                RejectedChangeKind::RuntimeStateUnavailable,
            )],
            history_entry: history_entry(GenerationOperation::Rollback, GenerationStatus::Rejected),
        };

        assert_eq!(rollback_result_status(&rollback), StatusCode::CONFLICT);
    }
}
