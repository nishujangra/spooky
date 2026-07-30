use bytes::Bytes;
use http_body_util::Full;

use super::*;
use crate::runtime::{
    activation::{
        ActivationRequest, RejectedChangeKind, ReloadConfigInput, ReloadPlan,
        StagedRuntimeReloadPlan, plan_runtime_reload,
    },
    bundle::{ActiveRuntimeGeneration, RuntimeBundleHandle},
    policy::{ReloadCompatibilityAuthority, TransitionRejection},
};

impl QUICListener {
    pub(super) fn apply_live_log_level_reload(
        current_level: &str,
        next_level: &str,
    ) -> Result<bool, spooky_utils::logger::LogLevelError> {
        if current_level == next_level {
            return Ok(false);
        }

        spooky_utils::logger::set_log_level(next_level)?;
        Ok(true)
    }

    pub(super) fn reload_listener_certs(
        listener_runtime_configs: &HashMap<String, ListenerRuntimeConfig>,
        listener_tls_store: &ListenerTlsReloadStore,
        metrics: &Metrics,
    ) -> Response<Full<Bytes>> {
        let mut staged = Vec::with_capacity(listener_runtime_configs.len());
        for (listener_label, listener_config) in listener_runtime_configs {
            let reloaded_state = match Self::build_listener_tls_reload_state(listener_config) {
                Ok(state) => state,
                Err(err) => {
                    return Self::json_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({
                            "reloaded": false,
                            "listener": listener_label,
                            "error": err.to_string(),
                        }),
                    );
                }
            };
            staged.push((listener_label.clone(), reloaded_state));
        }

        let generations = match listener_tls_store.replace_listeners(&staged) {
            Ok(generations) => generations,
            Err(err) => {
                return Self::json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({
                        "reloaded": false,
                        "error": err.to_string(),
                    }),
                );
            }
        };

        let mut reloaded = Vec::with_capacity(staged.len());
        for (listener_label, reloaded_state) in staged {
            Self::update_listener_tls_expiry_metrics(
                metrics,
                &listener_label,
                &reloaded_state.inventory,
            );
            reloaded.push(json!({
                "listener": listener_label,
                "generation": generations.get(&listener_label).copied().unwrap_or(0),
            }));
        }

        Self::json_response(
            StatusCode::ACCEPTED,
            json!({
                "reloaded": true,
                "listeners": reloaded,
            }),
        )
    }

    pub(super) fn handle_control_api_reload_certs(
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
    ) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let live_tls_store = runtime_state.listener_tls_store();
        let live_listener_configs = runtime_state.listener_runtime_configs();
        let live_metrics = runtime_state.metrics();
        Self::reload_listener_certs(
            live_listener_configs.as_ref(),
            live_tls_store.as_ref(),
            live_metrics.as_ref(),
        )
    }

    pub(super) fn handle_control_api_runtime_reload(
        req: &Request<Incoming>,
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
    ) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let Some(runtime_bundle_handle) = runtime_state.runtime_bundle_handle().cloned() else {
            return Self::control_api_not_found_response();
        };
        let Some(runtime) = runtime_state.generation.clone() else {
            return Self::json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({
                    "reloaded": false,
                    "error": "runtime generation unavailable",
                }),
            );
        };

        let plan = plan_runtime_reload(
            &runtime,
            ActivationRequest {
                requested_by: Some("control_api".to_string()),
                reason: Some("runtime_reload".to_string()),
                expected_generation: Some(runtime.generation()),
                requested_at_ms: crate::watchdog::time::now_millis(),
            },
            ReloadConfigInput::Path {
                path: runtime.startup().config_path.clone(),
            },
        );
        if !plan.can_activate() {
            let error = plan
                .plan
                .rejection_summary
                .as_deref()
                .unwrap_or("reload candidate rejected");
            // Phase 8: API response and logs communicate the same core reason.
            warn!(
                "runtime reload rejected at generation {}; active runtime unchanged: {}",
                runtime.generation(),
                error
            );
            return Self::json_response(
                plan_rejection_status(&plan.plan),
                json!({
                    "reloaded": false,
                    "error": error,
                    "generation": plan.plan.current_generation,
                    "candidate_generation": plan.plan.candidate_generation,
                    "compatibility": plan.plan.compatibility,
                }),
            );
        }
        let current_log_level = plan.current_log_level.clone();
        let next_log_level = plan.next_log_level.clone().unwrap_or_default();
        let generation = match Self::apply_runtime_reload_plan(&runtime_bundle_handle, plan) {
            Ok(generation) => generation,
            Err(err) => {
                return Self::json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({
                        "reloaded": false,
                        "error": err.to_string(),
                    }),
                );
            }
        };
        if let Err(err) = Self::apply_live_log_level_reload(&current_log_level, &next_log_level) {
            error!(
                "Runtime reload applied generation={} but failed to update live log.level from '{}' to '{}': {}",
                generation, current_log_level, next_log_level, err
            );
        }
        Self::json_response(
            StatusCode::ACCEPTED,
            json!({
                "reloaded": true,
                "generation": generation,
                "candidate_generation": generation,
                "path": req.uri().path(),
            }),
        )
    }

    pub(super) fn handle_control_api_restart(
        state: &crate::quic_listener::runtime_state::ControlApiServiceCtx,
    ) -> Response<Full<Bytes>> {
        let watchdog = state.current_service_state().watchdog();
        if !watchdog.enabled() {
            return Self::json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({
                    "accepted": false,
                    "error": "watchdog disabled",
                }),
            );
        }

        let accepted = watchdog.request_restart("admin_runtime_api");
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

    pub(super) fn apply_runtime_reload_plan(
        runtime_bundle_handle: &RuntimeBundleHandle,
        plan: StagedRuntimeReloadPlan,
    ) -> Result<u64, ProxyError> {
        let next_runtime = plan.next_runtime.ok_or_else(|| {
            ProxyError::Transport("staged reload plan missing next runtime".to_string())
        })?;
        QUICListener::spawn_generation_background_tasks_for_runtime(
            &next_runtime.runtime_config,
            next_runtime.shared_state.as_ref(),
        );
        runtime_bundle_handle.replace(next_runtime)
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

fn plan_rejection_status(plan: &ReloadPlan) -> StatusCode {
    if plan
        .rejected_changes
        .iter()
        .any(|rejection| matches!(rejection.kind, RejectedChangeKind::InvalidConfiguration))
    {
        StatusCode::BAD_REQUEST
    } else if plan.rejected_changes.iter().any(|rejection| {
        matches!(
            rejection.kind,
            RejectedChangeKind::ResourcePreparationFailed
        )
    }) {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::CONFLICT
    }
}
