use std::collections::HashMap;

use bytes::Bytes;
use http_body_util::Full;
use serde::Serialize;
use spooky_lb::health::HealthFailureReason;

use super::{state::ControlApiState, *};
use crate::runtime::{
    activation::GenerationHistoryEntry,
    backend::state::{
        BackendHealthState, BackendLifecycleInventorySnapshot, BackendMembershipState,
        BackendPoolPlacementSnapshot,
    },
};

/// Map a backend health-failure reason to the canonical control-plane token.
///
/// These are the same tokens as the `spooky_health_failures_total{reason=…}`
/// metric label (obs Phase 4), so control-plane JSON and metrics name the failure
/// the same way.
fn health_failure_reason_label(reason: HealthFailureReason) -> &'static str {
    match reason {
        HealthFailureReason::HttpStatus5xx => "5xx",
        HealthFailureReason::Timeout => "timeout",
        HealthFailureReason::Transport => "transport",
        HealthFailureReason::Tls => "tls",
        HealthFailureReason::CircuitOpen => "circuit_open",
    }
}

#[derive(Serialize)]
struct ControlApiHealthPayload {
    status: &'static str,
    uptime_ms: u64,
    watchdog: ControlApiHealthWatchdogPayload,
}

#[derive(Serialize)]
struct ControlApiHealthWatchdogPayload {
    enabled: bool,
    degraded: bool,
    restart_requested: bool,
}

#[derive(Serialize)]
struct ControlApiReadyPayload {
    ready: bool,
    healthy_backends: usize,
    total_backends: usize,
    restart_requested: bool,
}

#[derive(Serialize)]
struct ControlApiRuntimePayload {
    uptime_ms: u64,
    workers: ControlApiWorkerPayload,
    watchdog: ControlApiRuntimeWatchdogPayload,
    adaptive_admission: ControlApiAdaptiveAdmissionPayload,
    backends: ControlApiBackendInventoryPayload,
    metrics: ControlApiMetricsPayload,
    tls: ControlApiTlsPayload,
    extension_model: ControlApiExtensionModelPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<ControlApiRuntimeGenerationPayload>,
}

#[derive(Serialize)]
struct ControlApiWorkerPayload {
    expected: usize,
}

#[derive(Serialize)]
struct ControlApiRuntimeWatchdogPayload {
    enabled: bool,
    degraded: bool,
    restart_requested: bool,
    restart_reason: String,
    restart_requested_at_ms: u64,
}

#[derive(Serialize)]
struct ControlApiAdaptiveAdmissionPayload {
    enabled: bool,
    current_limit: usize,
    inflight_percent: u8,
}

#[derive(Serialize)]
struct ControlApiBackendInventoryPayload {
    healthy: usize,
    total: usize,
    lifecycle: Vec<ControlApiBackendLifecyclePayload>,
}

#[derive(Serialize)]
struct ControlApiBackendLifecyclePayload {
    backend: String,
    health: &'static str,
    /// Canonical health-failure reason when the backend is unhealthy and a
    /// reason is known. Uses the same tokens as `spooky_health_failures_total`'s
    /// `reason=` label (obs Phase 4), so operators do not translate between the
    /// control-plane JSON and the metric. Omitted when healthy / reason unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    health_reason: Option<&'static str>,
    membership: &'static str,
    authority_host: String,
    authority_port: u16,
    resolved_addrs: Vec<String>,
    resolution_generation: u64,
    last_refresh_success_at_unix_seconds: Option<u64>,
    placements: Vec<ControlApiBackendPlacementPayload>,
}

#[derive(Serialize)]
struct ControlApiBackendPlacementPayload {
    upstream: String,
    backend_index: usize,
    healthy: bool,
    active_requests: usize,
    ewma_latency_ms: Option<f64>,
    membership_epoch: u64,
}

#[derive(Serialize)]
struct ControlApiMetricsPayload {
    requests_total: u64,
    requests_success: u64,
    requests_failure: u64,
    active_connections: u64,
    backend_timeouts: u64,
    backend_errors: u64,
}

#[derive(Serialize)]
struct ControlApiTlsPayload {
    listeners: HashMap<String, ControlApiTlsListenerPayload>,
}

#[derive(Serialize)]
struct ControlApiTlsListenerPayload {
    default_cert: String,
    default_key: String,
    default_cert_not_after_unix_seconds: i64,
    sni_names: Vec<String>,
    client_auth_enabled: bool,
    require_client_cert: bool,
    generation: u64,
}

#[derive(Serialize)]
struct ControlApiExtensionModelPayload {
    status: &'static str,
    details: &'static str,
}

#[derive(Serialize)]
struct ControlApiRuntimeGenerationPayload {
    generation: u64,
    config_path: String,
}

#[derive(Serialize)]
struct ControlApiRuntimeHistoryPayload {
    active_generation: u64,
    entries: Vec<GenerationHistoryEntry>,
}

#[derive(Serialize)]
struct ControlApiRuntimeHistoryGenerationPayload {
    generation: u64,
    entries: Vec<GenerationHistoryEntry>,
}

impl QUICListener {
    pub(super) fn json_response<T>(status: StatusCode, value: T) -> Response<Full<Bytes>>
    where
        T: Serialize,
    {
        let body = match serde_json::to_vec(&value) {
            Ok(body) => body,
            Err(_) => br#"{"error":"response"}"#.to_vec(),
        };
        match Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body)))
        {
            Ok(resp) => resp,
            Err(_) => Response::new(Full::new(Bytes::from_static(b"{\"error\":\"response\"}"))),
        }
    }

    pub(super) fn render_control_api_health(state: &ControlApiState) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let watchdog = runtime_state.watchdog();
        let payload = ControlApiHealthPayload {
            status: "ok",
            uptime_ms: state.started_at.elapsed().as_millis() as u64,
            watchdog: ControlApiHealthWatchdogPayload {
                enabled: watchdog.enabled(),
                degraded: watchdog.is_degraded(),
                restart_requested: watchdog.restart_requested(),
            },
        };
        Self::json_response(StatusCode::OK, payload)
    }

    pub(super) fn render_control_api_ready(state: &ControlApiState) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let backend_summary = runtime_state.snapshot_backend_health();
        let restart_requested = runtime_state.watchdog().restart_requested();
        let payload = ControlApiReadyPayload {
            ready: !restart_requested
                && (backend_summary.total_backends == 0 || backend_summary.healthy_backends > 0),
            healthy_backends: backend_summary.healthy_backends,
            total_backends: backend_summary.total_backends,
            restart_requested,
        };
        Self::json_response(
            if payload.ready {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
            payload,
        )
    }

    pub(super) fn render_control_api_runtime_snapshot(
        state: &ControlApiState,
    ) -> Response<Full<Bytes>> {
        let payload = ControlApiRuntimePayload::from_state(state);
        Self::json_response(StatusCode::OK, payload)
    }

    pub(super) fn render_control_api_runtime_history(
        state: &ControlApiState,
    ) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let Some(runtime_bundle_handle) = runtime_state.runtime_bundle_handle().cloned() else {
            return Self::control_api_not_found_response();
        };

        Self::json_response(
            StatusCode::OK,
            ControlApiRuntimeHistoryPayload {
                active_generation: runtime_bundle_handle.current_generation(),
                entries: runtime_bundle_handle.generation_change_history(),
            },
        )
    }

    pub(super) fn render_control_api_runtime_history_generation(
        state: &ControlApiState,
        generation: u64,
    ) -> Response<Full<Bytes>> {
        let runtime_state = state.current_service_state();
        let Some(runtime_bundle_handle) = runtime_state.runtime_bundle_handle().cloned() else {
            return Self::control_api_not_found_response();
        };

        let entries = runtime_bundle_handle
            .generation_change_history()
            .into_iter()
            .filter(|entry| entry.generation == generation)
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return Self::json_response(
                StatusCode::NOT_FOUND,
                json!({
                    "error": format!("generation {generation} not found in runtime history"),
                }),
            );
        }

        Self::json_response(
            StatusCode::OK,
            ControlApiRuntimeHistoryGenerationPayload {
                generation,
                entries,
            },
        )
    }
}

impl ControlApiRuntimePayload {
    fn from_state(service_ctx: &ControlApiState) -> Self {
        let state = service_ctx.current_service_state();
        let runtime = &state.runtime;
        let watchdog = state.watchdog();
        let resilience = state.resilience();
        let metrics = state.metrics();
        let listener_tls_store = state.listener_tls_store();
        let backend_inventory = state.snapshot_backend_inventory();
        let backend_summary = backend_inventory.summary();

        Self {
            uptime_ms: service_ctx.started_at.elapsed().as_millis() as u64,
            workers: ControlApiWorkerPayload {
                expected: runtime.expected_workers(),
            },
            watchdog: ControlApiRuntimeWatchdogPayload {
                enabled: watchdog.enabled(),
                degraded: watchdog.is_degraded(),
                restart_requested: watchdog.restart_requested(),
                restart_reason: watchdog.restart_reason(),
                restart_requested_at_ms: watchdog.restart_requested_at_ms(),
            },
            adaptive_admission: ControlApiAdaptiveAdmissionPayload {
                enabled: resilience.adaptive_admission.enabled(),
                current_limit: resilience.adaptive_admission.current_limit(),
                inflight_percent: resilience.adaptive_admission.inflight_percent(),
            },
            backends: ControlApiBackendInventoryPayload::from_inventory(
                backend_inventory,
                backend_summary.healthy_backends,
                backend_summary.total_backends,
            ),
            metrics: ControlApiMetricsPayload {
                requests_total: metrics.requests_total.load(Ordering::Relaxed),
                requests_success: metrics.requests_success.load(Ordering::Relaxed),
                requests_failure: metrics.requests_failure.load(Ordering::Relaxed),
                active_connections: metrics.active_connections.load(Ordering::Relaxed),
                backend_timeouts: metrics.backend_timeouts.load(Ordering::Relaxed),
                backend_errors: metrics.backend_errors.load(Ordering::Relaxed),
            },
            tls: ControlApiTlsPayload {
                listeners: listener_tls_store
                    .snapshot()
                    .into_iter()
                    .map(|(listener, inventory)| {
                        (
                            listener.clone(),
                            ControlApiTlsListenerPayload {
                                default_cert: inventory.default_identity.identity.cert_path,
                                default_key: inventory.default_identity.identity.key_path,
                                default_cert_not_after_unix_seconds: inventory
                                    .default_identity
                                    .metadata
                                    .not_after_unix_seconds,
                                sni_names: inventory.sni_identities.keys().cloned().collect(),
                                client_auth_enabled: inventory.listener_tls.client_auth.enabled,
                                require_client_cert: inventory
                                    .listener_tls
                                    .client_auth
                                    .require_client_cert,
                                generation: listener_tls_store.generation(&listener).unwrap_or(0),
                            },
                        )
                    })
                    .collect(),
            },
            extension_model: ControlApiExtensionModelPayload {
                status: "non_goal",
                details: "No plugin/middleware ABI is exposed in-process today; extension support remains a deliberate non-goal until a safe isolation model is designed.",
            },
            runtime: state
                .generation
                .map(|active| ControlApiRuntimeGenerationPayload {
                    generation: active.generation(),
                    config_path: active.startup().config_path.clone(),
                }),
        }
    }
}

impl ControlApiBackendInventoryPayload {
    fn from_inventory(
        inventory: BackendLifecycleInventorySnapshot,
        healthy: usize,
        total: usize,
    ) -> Self {
        Self {
            healthy,
            total,
            lifecycle: inventory
                .backends
                .into_iter()
                .map(|backend| ControlApiBackendLifecyclePayload {
                    backend: backend.identity.backend_addr,
                    health: match backend.health {
                        BackendHealthState::Unknown => "unknown",
                        BackendHealthState::Healthy => "healthy",
                        BackendHealthState::Unhealthy { .. } => "unhealthy",
                    },
                    health_reason: match backend.health {
                        BackendHealthState::Unhealthy {
                            reason: Some(reason),
                        } => Some(health_failure_reason_label(reason)),
                        _ => None,
                    },
                    membership: match backend.membership {
                        BackendMembershipState::Active => "active",
                        BackendMembershipState::Suppressed => "suppressed",
                        BackendMembershipState::Removed => "removed",
                    },
                    authority_host: backend.resolution.authority_host,
                    authority_port: backend.resolution.authority_port,
                    resolved_addrs: backend
                        .resolution
                        .resolved_addrs
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    resolution_generation: backend.resolution.refresh_generation,
                    last_refresh_success_at_unix_seconds: backend
                        .resolution
                        .last_refresh_success_at
                        .and_then(|time| {
                            time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok()
                        })
                        .map(|duration| duration.as_secs()),
                    placements: backend
                        .placements
                        .into_iter()
                        .map(ControlApiBackendPlacementPayload::from_snapshot)
                        .collect(),
                })
                .collect(),
        }
    }
}

impl ControlApiBackendPlacementPayload {
    fn from_snapshot(snapshot: BackendPoolPlacementSnapshot) -> Self {
        Self {
            upstream: snapshot.upstream_name,
            backend_index: snapshot.backend_index,
            healthy: snapshot.healthy,
            active_requests: snapshot.active_requests,
            ewma_latency_ms: snapshot.ewma_latency_ms,
            membership_epoch: snapshot.membership_epoch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_health_reason_token(reason: HealthFailureReason, expected: &'static str) {
        let control_plane_reason = health_failure_reason_label(reason);
        let structured_log_token = format!("health_reason={control_plane_reason}");

        assert_eq!(
            control_plane_reason, expected,
            "control-plane health reason token should stay canonical"
        );
        assert_eq!(
            structured_log_token,
            format!("health_reason={expected}"),
            "structured log token should reuse the canonical health reason value"
        );
    }

    mod observability_contracts {
        use super::*;

        #[test]
        fn control_plane_health_reason_tokens_match_metric_reason_labels() {
            // obs Phase 4: control-plane `health_reason` must use the same tokens as
            // the `spooky_health_failures_total{reason=…}` metric label so operators
            // don't translate between surfaces.
            assert_health_reason_token(HealthFailureReason::HttpStatus5xx, "5xx");
            assert_health_reason_token(HealthFailureReason::Timeout, "timeout");
            assert_health_reason_token(HealthFailureReason::Transport, "transport");
            assert_health_reason_token(HealthFailureReason::Tls, "tls");
            assert_health_reason_token(HealthFailureReason::CircuitOpen, "circuit_open");
        }

        #[test]
        fn unhealthy_backend_reason_tokens_stay_aligned_across_surfaces() {
            for (reason, expected) in [
                (HealthFailureReason::Timeout, "timeout"),
                (HealthFailureReason::Transport, "transport"),
                (HealthFailureReason::Tls, "tls"),
            ] {
                assert_health_reason_token(reason, expected);
            }
        }
    }

    mod runtime_snapshot_rendering {
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};

        use super::*;
        use crate::runtime::backend::{
            resolution::RuntimeBackendAddressKind,
            state::{
                BackendIdentity, BackendPoolPlacementSnapshot, BackendResolutionState,
                CanonicalBackendLifecycleSnapshot,
            },
        };

        #[test]
        fn backend_payload_serialization_omits_or_includes_optional_health_reason_by_contract() {
            // The field appears only when unhealthy with a known reason, and is
            // omitted otherwise (skip_serializing_if).
            let with_reason = ControlApiBackendLifecyclePayload {
                backend: "b".to_string(),
                health: "unhealthy",
                health_reason: Some("timeout"),
                membership: "active",
                authority_host: "h".to_string(),
                authority_port: 443,
                resolved_addrs: vec![],
                resolution_generation: 0,
                last_refresh_success_at_unix_seconds: None,
                placements: vec![],
            };
            let json = serde_json::to_string(&with_reason).expect("serialize");
            assert!(json.contains("\"health_reason\":\"timeout\""));

            let without = ControlApiBackendLifecyclePayload {
                health_reason: None,
                ..with_reason
            };
            let json = serde_json::to_string(&without).expect("serialize");
            assert!(!json.contains("health_reason"));
        }

        #[test]
        fn backend_inventory_payload_preserves_runtime_snapshot_field_meanings() {
            let inventory = BackendLifecycleInventorySnapshot {
                backends: vec![
                    CanonicalBackendLifecycleSnapshot {
                        identity: BackendIdentity::new("backend-a"),
                        resolution: BackendResolutionState {
                            authority_host: "backend-a.internal".into(),
                            authority_port: 8443,
                            address_kind: RuntimeBackendAddressKind::Hostname,
                            resolved_addrs: vec![SocketAddr::new(
                                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10)),
                                8443,
                            )],
                            last_refresh_success_at: None,
                            refresh_generation: 4,
                        },
                        health: BackendHealthState::Unhealthy {
                            reason: Some(HealthFailureReason::Timeout),
                        },
                        membership: BackendMembershipState::Suppressed,
                        placements: Vec::new(),
                    },
                    CanonicalBackendLifecycleSnapshot {
                        identity: BackendIdentity::new("backend-b"),
                        resolution: BackendResolutionState {
                            authority_host: "backend-b.internal".into(),
                            authority_port: 9443,
                            address_kind: RuntimeBackendAddressKind::IpLiteral,
                            resolved_addrs: vec![SocketAddr::new(
                                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 11)),
                                9443,
                            )],
                            last_refresh_success_at: None,
                            refresh_generation: 2,
                        },
                        health: BackendHealthState::Healthy,
                        membership: BackendMembershipState::Active,
                        placements: vec![BackendPoolPlacementSnapshot {
                            upstream_name: "api".into(),
                            backend_index: 0,
                            healthy: true,
                            active_requests: 1,
                            ewma_latency_ms: Some(12.5),
                            membership_epoch: 9,
                        }],
                    },
                ],
            };

            let payload = ControlApiBackendInventoryPayload::from_inventory(inventory, 1, 1);
            let json = serde_json::to_value(payload).expect("serialize backend inventory");
            let lifecycle = json["lifecycle"].as_array().expect("lifecycle array");

            assert_eq!(lifecycle[0]["backend"], "backend-a");
            assert_eq!(lifecycle[0]["health"], "unhealthy");
            assert_eq!(lifecycle[0]["health_reason"], "timeout");
            assert_eq!(lifecycle[0]["membership"], "suppressed");
            assert_eq!(lifecycle[0]["authority_host"], "backend-a.internal");
            assert_eq!(lifecycle[0]["authority_port"], 8443);
            assert_eq!(lifecycle[0]["resolution_generation"], 4);

            assert_eq!(lifecycle[1]["backend"], "backend-b");
            assert_eq!(lifecycle[1]["health"], "healthy");
            assert!(
                lifecycle[1].get("health_reason").is_none(),
                "healthy payloads must omit optional health_reason"
            );
            assert_eq!(lifecycle[1]["membership"], "active");
            assert_eq!(
                lifecycle[1]["placements"]
                    .as_array()
                    .expect("placements")
                    .len(),
                1
            );
        }
    }
}
