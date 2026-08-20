use std::collections::HashMap;

use bytes::Bytes;
use http_body_util::Full;
use serde::Serialize;
use spooky_config::config::SecretProvider;
use spooky_config::runtime::RuntimeJwtAuth;
use spooky_lb::health::HealthFailureReason;

use super::{state::ControlApiState, *};
use crate::{
    quic_listener::control_api::audit::ADMIN_AUDIT_SCHEMA_VERSION,
    resilience::quota::{
        QuotaBackendErrorSnapshot, QuotaBackendStatusSnapshot, QuotaPolicyIntrospectionSnapshot,
        QuotaSelectorIntrospectionSnapshot, QuotaWindowIntrospectionSnapshot,
    },
    runtime::{
        activation::{
            GenerationChangeEvent, GenerationEventKind, GenerationHistoryEntry,
            GenerationOperation, GenerationStatus,
        },
        backend::state::{
            BackendHealthState, BackendLifecycleInventorySnapshot, BackendMembershipState,
            BackendPoolPlacementSnapshot,
        },
        bundle::RuntimeGenerationRecord,
    },
};

const OBSERVABILITY_CONTRACT_VERSION: &str = "v1";
const RECENT_ADMIN_ACTION_LIMIT: usize = 5;

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
    quota: ControlApiQuotaPayload,
    observability: ControlApiObservabilityPayload,
    auth: ControlApiAuthPayload,
    jwks: ControlApiJwksPayload,
    backends: ControlApiBackendInventoryPayload,
    metrics: ControlApiMetricsPayload,
    tls: ControlApiTlsPayload,
    secrets: ControlApiSecretsPayload,
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
struct ControlApiQuotaPayload {
    enabled: bool,
    enforcement: &'static str,
    backend_failure_policy: &'static str,
    active_backend: String,
    backend_status: ControlApiQuotaBackendStatusPayload,
    policies: Vec<ControlApiQuotaPolicyPayload>,
}

#[derive(Serialize)]
struct ControlApiQuotaBackendStatusPayload {
    availability: String,
    degraded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    health_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_observed_at_unix_ms: Option<u64>,
    recent_errors: Vec<ControlApiQuotaBackendErrorPayload>,
}

#[derive(Serialize)]
struct ControlApiQuotaBackendErrorPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_at_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_name: Option<String>,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Serialize)]
struct ControlApiQuotaPolicyPayload {
    name: String,
    route_allowlist: Vec<String>,
    selector: ControlApiQuotaSelectorPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    burst: Option<ControlApiQuotaWindowPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sustained: Option<ControlApiQuotaWindowPayload>,
}

#[derive(Serialize)]
struct ControlApiQuotaSelectorPayload {
    route: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client: Option<String>,
}

#[derive(Serialize)]
struct ControlApiQuotaWindowPayload {
    requests: u64,
    window_secs: u64,
}

#[derive(Serialize)]
struct ControlApiObservabilityPayload {
    contract_version: &'static str,
    audit_schema_version: &'static str,
    current_generation: Option<u64>,
    documentation: ControlApiObservabilityDocumentationPayload,
    dashboard_packages: Vec<ControlApiObservabilityDashboardPayload>,
    backend_health_summary: ControlApiBackendHealthSummaryPayload,
    quota_backend_health_summary: ControlApiQuotaBackendHealthSummaryPayload,
    recent_admin_actions: Vec<ControlApiRecentAdminActionPayload>,
}

#[derive(Serialize)]
struct ControlApiObservabilityDocumentationPayload {
    observability_contract: &'static str,
    control_plane_operations: &'static str,
    metrics_and_alerts_operations: &'static str,
    distributed_quota_operations: &'static str,
}

#[derive(Serialize)]
struct ControlApiObservabilityDashboardPayload {
    dashboard_id: &'static str,
    definition_path: &'static str,
    focus: &'static str,
}

#[derive(Serialize)]
struct ControlApiBackendHealthSummaryPayload {
    availability: &'static str,
    placed_total: usize,
    healthy: usize,
    unhealthy: usize,
    unknown: usize,
    active_membership: usize,
    suppressed_membership: usize,
    removed_membership: usize,
}

#[derive(Serialize)]
struct ControlApiQuotaBackendHealthSummaryPayload {
    enabled: bool,
    active_backend: String,
    availability: String,
    degraded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    health_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_observed_at_unix_ms: Option<u64>,
    recent_error_count: usize,
}

#[derive(Serialize)]
struct ControlApiRecentAdminActionPayload {
    kind: GenerationEventKind,
    operation: GenerationOperation,
    generation: u64,
    status: GenerationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger_source: Option<String>,
    requested_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at_ms: Option<u64>,
    event_emitted_at_ms: u64,
    summary: String,
}

#[derive(Serialize)]
struct ControlApiBackendInventoryPayload {
    healthy: usize,
    total: usize,
    lifecycle: Vec<ControlApiBackendLifecyclePayload>,
}

#[derive(Serialize)]
struct ControlApiAuthPayload {
    providers: Vec<ControlApiAuthProviderPayload>,
    jwt_validation_failures: Vec<ControlApiReasonCountPayload>,
    jwt_algorithm_rejections: Vec<ControlApiReasonCountPayload>,
    unknown_kid_events: Vec<ControlApiUnknownKidEventPayload>,
}

#[derive(Serialize)]
struct ControlApiAuthProviderPayload {
    upstream: String,
    api_key_configured: bool,
    external_auth_configured: bool,
    required_scopes: Vec<String>,
    required_roles: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    jwt: Option<ControlApiJwtProviderPayload>,
}

#[derive(Serialize)]
struct ControlApiJwtProviderPayload {
    provider_mode: &'static str,
    allowed_algorithms: Vec<String>,
    require_kid: bool,
    issuers: Vec<String>,
    audiences: Vec<String>,
    static_key_count: usize,
    jwks_configured: bool,
    jwks_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    jwks_cache_state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    serving_from_stale_cache: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usable_key_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_refresh_success_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_refresh_attempt_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_failure_reason: Option<String>,
}

#[derive(Serialize)]
struct ControlApiReasonCountPayload {
    reason: String,
    count: u64,
}

#[derive(Serialize)]
struct ControlApiUnknownKidEventPayload {
    jwks_source_id: String,
    count: u64,
}

#[derive(Serialize)]
struct ControlApiJwksPayload {
    sources: Vec<ControlApiJwksSourcePayload>,
}

#[derive(Serialize)]
struct ControlApiJwksSourcePayload {
    jwks_source_id: String,
    jwks_endpoint: String,
    allowed_algorithms: Vec<String>,
    startup_behavior: &'static str,
    cache_state: &'static str,
    active_key_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    age_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_refresh_attempt_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_refresh_success_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_failure_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
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
    upstreams: HashMap<String, ControlApiTlsUpstreamPayload>,
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
    last_loaded_at_unix_ms: u64,
    last_reload_status: String,
}

#[derive(Serialize)]
struct ControlApiTlsUpstreamPayload {
    verify_certificates: bool,
    strict_sni: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    ca_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ca_file_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ca_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ca_dir_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_certificate: Option<ControlApiSecretMaterialPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_key: Option<ControlApiSecretMaterialPayload>,
}

#[derive(Serialize)]
struct ControlApiSecretsPayload {
    providers: Vec<ControlApiSecretProviderPayload>,
    material: Vec<ControlApiSecretMaterialPayload>,
}

#[derive(Serialize)]
struct ControlApiSecretProviderPayload {
    provider: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_dir: Option<String>,
    default_provider: bool,
}

#[derive(Serialize, Clone)]
struct ControlApiSecretMaterialPayload {
    scope: String,
    source_kind: &'static str,
    reference: String,
    fingerprint: String,
    last_loaded_at_unix_ms: u64,
    last_reload_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expiry_not_after_unix_seconds: Option<i64>,
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
    observability: ControlApiObservabilityPayload,
    retained_generations: Vec<ControlApiRetainedGenerationPayload>,
    entries: Vec<GenerationHistoryEntry>,
}

#[derive(Serialize)]
struct ControlApiRuntimeHistoryGenerationPayload {
    generation: u64,
    observability: ControlApiObservabilityPayload,
    retained_generation: ControlApiRetainedGenerationPayload,
    entries: Vec<GenerationHistoryEntry>,
}

#[derive(Serialize)]
struct ControlApiRetainedGenerationPayload {
    generation: u64,
    status: &'static str,
    rollback_candidate: bool,
    has_bundle: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
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
        let backend_inventory = runtime_state.snapshot_backend_inventory();
        let backend_summary = backend_inventory.summary();
        let quota_backend_status = runtime_state.resilience().quota.backend_status_snapshot(
            runtime_state
                .resilience()
                .quota_backend_initialization_error
                .as_ref(),
        );

        Self::json_response(
            StatusCode::OK,
            ControlApiRuntimeHistoryPayload {
                active_generation: runtime_bundle_handle.current_generation(),
                observability: ControlApiObservabilityPayload::from_runtime_state(
                    &runtime_state,
                    &backend_inventory,
                    &backend_summary,
                    &quota_backend_status,
                ),
                retained_generations: runtime_bundle_handle
                    .generation_history()
                    .into_iter()
                    .map(ControlApiRetainedGenerationPayload::from_record)
                    .collect(),
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
        let backend_inventory = runtime_state.snapshot_backend_inventory();
        let backend_summary = backend_inventory.summary();
        let quota_backend_status = runtime_state.resilience().quota.backend_status_snapshot(
            runtime_state
                .resilience()
                .quota_backend_initialization_error
                .as_ref(),
        );

        let entries = runtime_bundle_handle
            .generation_change_history()
            .into_iter()
            .filter(|entry| entry.generation == generation)
            .collect::<Vec<_>>();
        let Some(record) = runtime_bundle_handle.generation_record(generation) else {
            return Self::json_response(
                StatusCode::NOT_FOUND,
                json!({
                    "error": format!("generation {generation} not found in runtime history"),
                }),
            );
        };

        Self::json_response(
            StatusCode::OK,
            ControlApiRuntimeHistoryGenerationPayload {
                generation,
                observability: ControlApiObservabilityPayload::from_runtime_state(
                    &runtime_state,
                    &backend_inventory,
                    &backend_summary,
                    &quota_backend_status,
                ),
                retained_generation: ControlApiRetainedGenerationPayload::from_record(record),
                entries,
            },
        )
    }
}

impl ControlApiRetainedGenerationPayload {
    fn from_record(record: RuntimeGenerationRecord) -> Self {
        Self {
            generation: record.generation(),
            status: record.status().as_str(),
            rollback_candidate: record.status().is_rollback_candidate(),
            has_bundle: record.has_bundle(),
            note: record.note().map(ToOwned::to_owned),
        }
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
        let listener_tls_states = listener_tls_store.snapshot_states();
        let backend_inventory = state.snapshot_backend_inventory();
        let backend_summary = backend_inventory.summary();
        let quota_backend_status = resilience
            .quota
            .backend_status_snapshot(resilience.quota_backend_initialization_error.as_ref());
        let jwks_sources = crate::quic_listener::admission::snapshot_runtime_jwks_sources(
            runtime.runtime_config(),
        );
        let jwks_by_source_id = jwks_sources
            .iter()
            .map(|snapshot| (snapshot.source_id.as_str(), snapshot))
            .collect::<HashMap<_, _>>();

        let mut auth_providers = runtime
            .runtime_config()
            .upstreams
            .iter()
            .map(|(name, upstream)| ControlApiAuthProviderPayload {
                upstream: name.clone(),
                api_key_configured: upstream.policy.upstream_auth.api_key.is_some(),
                external_auth_configured: upstream.policy.upstream_auth.external_auth.is_some(),
                required_scopes: upstream.policy.upstream_auth.required_scopes.clone(),
                required_roles: upstream.policy.upstream_auth.required_roles.clone(),
                jwt: upstream
                    .policy
                    .upstream_auth
                    .jwt
                    .as_ref()
                    .map(|jwt| jwt_provider_payload(jwt, &jwks_by_source_id)),
            })
            .collect::<Vec<_>>();
        auth_providers.sort_by(|left, right| left.upstream.cmp(&right.upstream));

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
            quota: ControlApiQuotaPayload::from_snapshot(
                resilience.quota.as_ref(),
                quota_backend_status.clone(),
            ),
            observability: ControlApiObservabilityPayload::from_runtime_state(
                &state,
                &backend_inventory,
                &backend_summary,
                &quota_backend_status,
            ),
            auth: ControlApiAuthPayload {
                providers: auth_providers,
                jwt_validation_failures: metrics
                    .snapshot_jwt_validation_failures()
                    .into_iter()
                    .map(|(reason, count)| ControlApiReasonCountPayload { reason, count })
                    .collect(),
                jwt_algorithm_rejections: metrics
                    .snapshot_jwt_algorithm_rejections()
                    .into_iter()
                    .map(|(reason, count)| ControlApiReasonCountPayload { reason, count })
                    .collect(),
                unknown_kid_events: metrics
                    .snapshot_jwks_unknown_kid_events()
                    .into_iter()
                    .map(|(jwks_source_id, count)| ControlApiUnknownKidEventPayload {
                        jwks_source_id,
                        count,
                    })
                    .collect(),
            },
            jwks: ControlApiJwksPayload {
                sources: jwks_sources
                    .into_iter()
                    .map(|snapshot| ControlApiJwksSourcePayload {
                        jwks_source_id: snapshot.source_id,
                        jwks_endpoint: snapshot.endpoint,
                        allowed_algorithms: snapshot.allowed_algorithms,
                        startup_behavior: snapshot.startup_behavior,
                        cache_state: snapshot.state,
                        active_key_count: snapshot.active_key_count,
                        age_seconds: snapshot.age_seconds,
                        last_refresh_attempt_unix_seconds: snapshot
                            .last_refresh_attempt_unix_seconds,
                        last_refresh_success_unix_seconds: snapshot
                            .last_refresh_success_unix_seconds,
                        last_failure_reason: snapshot.last_failure_reason,
                        last_error: snapshot.last_error,
                    })
                    .collect(),
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
                listeners: listener_tls_states
                    .into_iter()
                    .map(|(listener, inventory)| {
                        (
                            listener.clone(),
                            ControlApiTlsListenerPayload {
                                default_cert: sanitize_path(
                                    &inventory.inventory.default_identity.identity.cert_path,
                                ),
                                default_key: sanitize_path(
                                    &inventory.inventory.default_identity.identity.key_path,
                                ),
                                default_cert_not_after_unix_seconds: inventory
                                    .inventory
                                    .default_identity
                                    .metadata
                                    .not_after_unix_seconds,
                                sni_names: inventory
                                    .inventory
                                    .sni_identities
                                    .keys()
                                    .cloned()
                                    .collect(),
                                client_auth_enabled: inventory
                                    .inventory
                                    .listener_tls
                                    .client_auth
                                    .enabled,
                                require_client_cert: inventory
                                    .inventory
                                    .listener_tls
                                    .client_auth
                                    .require_client_cert,
                                generation: inventory.generation,
                                last_loaded_at_unix_ms: inventory.loaded_at_unix_ms,
                                last_reload_status: inventory.last_reload_status,
                            },
                        )
                    })
                    .collect(),
                upstreams: runtime
                    .runtime_config()
                    .upstreams
                    .iter()
                    .map(|(name, upstream)| {
                        let tls_policy = upstream.backend_tls_policy();
                        let effective_tls = &upstream.effective_tls;
                        (
                            name.clone(),
                            ControlApiTlsUpstreamPayload {
                                verify_certificates: tls_policy.verify_certificates,
                                strict_sni: tls_policy.strict_sni,
                                ca_file: tls_policy.ca_file.as_deref().map(sanitize_path),
                                ca_file_fingerprint: tls_policy
                                    .ca_file_fingerprint_sha256
                                    .as_deref()
                                    .map(fingerprint_surrogate),
                                ca_dir: tls_policy.ca_dir.as_deref().map(sanitize_path),
                                ca_dir_fingerprint: tls_policy
                                    .ca_dir_fingerprint_sha256
                                    .as_deref()
                                    .map(fingerprint_surrogate),
                                client_certificate: tls_policy.client_certificate.as_ref().map(
                                    |metadata| ControlApiSecretMaterialPayload {
                                        scope: format!("upstream.{name}.tls.client_certificate"),
                                        source_kind: metadata.source_kind.as_str(),
                                        reference: sanitize_optional_secret_reference(
                                            effective_tls
                                                .client_certificate_ref
                                                .as_ref()
                                                .map(|secret_ref| secret_ref.reference.as_str()),
                                            effective_tls.client_certificate.as_deref(),
                                        ),
                                        fingerprint: fingerprint_surrogate(
                                            &metadata.fingerprint_sha256,
                                        ),
                                        last_loaded_at_unix_ms: metadata.loaded_at_unix_ms,
                                        last_reload_status: "loaded".to_string(),
                                        expiry_not_after_unix_seconds: tls_policy
                                            .client_certificate_not_after_unix_seconds,
                                    },
                                ),
                                client_key: tls_policy.client_key.as_ref().map(|metadata| {
                                    ControlApiSecretMaterialPayload {
                                        scope: format!("upstream.{name}.tls.client_key"),
                                        source_kind: metadata.source_kind.as_str(),
                                        reference: sanitize_optional_secret_reference(
                                            effective_tls
                                                .client_key_ref
                                                .as_ref()
                                                .map(|secret_ref| secret_ref.reference.as_str()),
                                            effective_tls.client_key.as_deref(),
                                        ),
                                        fingerprint: fingerprint_surrogate(
                                            &metadata.fingerprint_sha256,
                                        ),
                                        last_loaded_at_unix_ms: metadata.loaded_at_unix_ms,
                                        last_reload_status: "loaded".to_string(),
                                        expiry_not_after_unix_seconds: None,
                                    }
                                }),
                            },
                        )
                    })
                    .collect(),
            },
            secrets: ControlApiSecretsPayload {
                providers: runtime
                    .runtime_config()
                    .secrets
                    .providers
                    .iter()
                    .map(|(provider, config)| ControlApiSecretProviderPayload {
                        provider: provider.clone(),
                        kind: match config {
                            SecretProvider::File { .. } => "file",
                        },
                        base_dir: match config {
                            SecretProvider::File { base_dir } => {
                                base_dir.as_deref().map(sanitize_path)
                            }
                        },
                        default_provider: runtime
                            .runtime_config()
                            .secrets
                            .default_provider
                            .as_deref()
                            == Some(provider.as_str()),
                    })
                    .collect(),
                material: runtime
                    .runtime_config()
                    .upstreams
                    .iter()
                    .flat_map(|(name, upstream)| {
                        let tls_policy = upstream.backend_tls_policy();
                        let effective_tls = &upstream.effective_tls;
                        let mut material = Vec::new();
                        if let Some(metadata) = tls_policy.client_certificate.as_ref() {
                            material.push(ControlApiSecretMaterialPayload {
                                scope: format!("upstream.{name}.tls.client_certificate"),
                                source_kind: metadata.source_kind.as_str(),
                                reference: sanitize_optional_secret_reference(
                                    effective_tls
                                        .client_certificate_ref
                                        .as_ref()
                                        .map(|secret_ref| secret_ref.reference.as_str()),
                                    effective_tls.client_certificate.as_deref(),
                                ),
                                fingerprint: fingerprint_surrogate(&metadata.fingerprint_sha256),
                                last_loaded_at_unix_ms: metadata.loaded_at_unix_ms,
                                last_reload_status: "loaded".to_string(),
                                expiry_not_after_unix_seconds: tls_policy
                                    .client_certificate_not_after_unix_seconds,
                            });
                        }
                        if let Some(metadata) = tls_policy.client_key.as_ref() {
                            material.push(ControlApiSecretMaterialPayload {
                                scope: format!("upstream.{name}.tls.client_key"),
                                source_kind: metadata.source_kind.as_str(),
                                reference: sanitize_optional_secret_reference(
                                    effective_tls
                                        .client_key_ref
                                        .as_ref()
                                        .map(|secret_ref| secret_ref.reference.as_str()),
                                    effective_tls.client_key.as_deref(),
                                ),
                                fingerprint: fingerprint_surrogate(&metadata.fingerprint_sha256),
                                last_loaded_at_unix_ms: metadata.loaded_at_unix_ms,
                                last_reload_status: "loaded".to_string(),
                                expiry_not_after_unix_seconds: None,
                            });
                        }
                        material
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

fn sanitize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }
    std::path::Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!(".../{name}"))
        .unwrap_or_else(|| "<path>".to_string())
}

fn sanitize_optional_secret_reference(
    secret_ref: Option<&str>,
    legacy_path: Option<&str>,
) -> String {
    if let Some(secret_ref) = secret_ref {
        if let Some(path) = secret_ref.strip_prefix("file://") {
            return format!("file://{}", sanitize_path(path));
        }
        if let Some((_scheme, _)) = secret_ref.split_once(':') {
            return "<secret_ref>".to_string();
        }
    }
    legacy_path
        .map(sanitize_path)
        .unwrap_or_else(|| "<unavailable>".to_string())
}

fn fingerprint_surrogate(fingerprint: &str) -> String {
    let trimmed = fingerprint.trim();
    let prefix_len = trimmed.len().min(12);
    format!("sha256:{}", &trimmed[..prefix_len])
}

impl ControlApiQuotaPayload {
    fn from_snapshot(
        runtime: &crate::resilience::quota::QuotaRuntime,
        backend_status: QuotaBackendStatusSnapshot,
    ) -> Self {
        Self {
            enabled: runtime.enabled,
            enforcement: runtime.enforcement.slug(),
            backend_failure_policy: runtime.backend_failure_policy.slug(),
            active_backend: backend_status.backend_mode.clone(),
            backend_status: ControlApiQuotaBackendStatusPayload::from_snapshot(backend_status),
            policies: runtime
                .policy_snapshots()
                .into_iter()
                .map(ControlApiQuotaPolicyPayload::from_snapshot)
                .collect(),
        }
    }
}

impl ControlApiObservabilityPayload {
    fn from_runtime_state(
        state: &super::context::ControlApiServiceState,
        backend_inventory: &BackendLifecycleInventorySnapshot,
        backend_summary: &crate::runtime::backend::state::BackendLifecycleInventorySummary,
        quota_backend_status: &QuotaBackendStatusSnapshot,
    ) -> Self {
        Self {
            contract_version: OBSERVABILITY_CONTRACT_VERSION,
            audit_schema_version: ADMIN_AUDIT_SCHEMA_VERSION,
            current_generation: state
                .generation
                .as_ref()
                .map(|generation| generation.generation()),
            documentation: ControlApiObservabilityDocumentationPayload {
                observability_contract: "docs/architecture/observability-contract.md",
                control_plane_operations: "docs/operations/control-plane.md",
                metrics_and_alerts_operations: "docs/operations/metrics-and-alerts.md",
                distributed_quota_operations: "docs/operations/distributed-quota.md",
            },
            dashboard_packages: vec![
                ControlApiObservabilityDashboardPayload {
                    dashboard_id: "edge_traffic",
                    definition_path: "deploy/observability/grafana/edge-traffic.json",
                    focus: "edge traffic, status mix, and latency",
                },
                ControlApiObservabilityDashboardPayload {
                    dashboard_id: "admission_overload",
                    definition_path: "deploy/observability/grafana/admission-overload.json",
                    focus: "admission, overload, quota, and auth outcomes",
                },
                ControlApiObservabilityDashboardPayload {
                    dashboard_id: "backend_health",
                    definition_path: "deploy/observability/grafana/backend-health.json",
                    focus: "backend health, dns refresh, and client rotations",
                },
                ControlApiObservabilityDashboardPayload {
                    dashboard_id: "retries_hedges",
                    definition_path: "deploy/observability/grafana/retries-hedges.json",
                    focus: "retry amplification and hedge effectiveness",
                },
                ControlApiObservabilityDashboardPayload {
                    dashboard_id: "tls_certificates",
                    definition_path: "deploy/observability/grafana/tls-certificates.json",
                    focus: "tls handshake failures and certificate expiry",
                },
                ControlApiObservabilityDashboardPayload {
                    dashboard_id: "control_plane",
                    definition_path: "deploy/observability/grafana/control-plane.json",
                    focus: "runtime activity, watchdog state, and control-plane health",
                },
            ],
            backend_health_summary: ControlApiBackendHealthSummaryPayload::from_inventory(
                backend_inventory,
                *backend_summary,
            ),
            quota_backend_health_summary: ControlApiQuotaBackendHealthSummaryPayload::from_snapshot(
                state.resilience().quota.enabled,
                quota_backend_status,
            ),
            recent_admin_actions: state
                .runtime_bundle_handle()
                .map(|handle| {
                    handle
                        .generation_change_events()
                        .into_iter()
                        .take(RECENT_ADMIN_ACTION_LIMIT)
                        .map(ControlApiRecentAdminActionPayload::from_event)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

impl ControlApiQuotaBackendStatusPayload {
    fn from_snapshot(snapshot: QuotaBackendStatusSnapshot) -> Self {
        Self {
            availability: snapshot.availability,
            degraded: snapshot.degraded,
            health_reason: snapshot.health_reason,
            last_observed_at_unix_ms: snapshot.last_observed_at_unix_ms,
            recent_errors: snapshot
                .recent_errors
                .into_iter()
                .map(ControlApiQuotaBackendErrorPayload::from_snapshot)
                .collect(),
        }
    }
}

impl ControlApiBackendHealthSummaryPayload {
    fn from_inventory(
        inventory: &BackendLifecycleInventorySnapshot,
        summary: crate::runtime::backend::state::BackendLifecycleInventorySummary,
    ) -> Self {
        let mut unhealthy = 0;
        let mut unknown = 0;
        let mut active_membership = 0;
        let mut suppressed_membership = 0;
        let mut removed_membership = 0;

        for backend in &inventory.backends {
            match backend.health {
                BackendHealthState::Healthy => {}
                BackendHealthState::Unhealthy { .. } => unhealthy += 1,
                BackendHealthState::Unknown => unknown += 1,
            }

            match backend.membership {
                BackendMembershipState::Active => active_membership += 1,
                BackendMembershipState::Suppressed => suppressed_membership += 1,
                BackendMembershipState::Removed => removed_membership += 1,
            }
        }

        let availability = if summary.total_backends == 0 {
            "empty"
        } else if unhealthy == 0 && unknown == 0 {
            "healthy"
        } else {
            "degraded"
        };

        Self {
            availability,
            placed_total: summary.total_backends,
            healthy: summary.healthy_backends,
            unhealthy,
            unknown,
            active_membership,
            suppressed_membership,
            removed_membership,
        }
    }
}

impl ControlApiQuotaBackendHealthSummaryPayload {
    fn from_snapshot(enabled: bool, snapshot: &QuotaBackendStatusSnapshot) -> Self {
        Self {
            enabled,
            active_backend: snapshot.backend_mode.clone(),
            availability: snapshot.availability.clone(),
            degraded: snapshot.degraded,
            health_reason: snapshot.health_reason.clone(),
            last_observed_at_unix_ms: snapshot.last_observed_at_unix_ms,
            recent_error_count: snapshot.recent_errors.len(),
        }
    }
}

impl ControlApiRecentAdminActionPayload {
    fn from_event(event: GenerationChangeEvent) -> Self {
        Self {
            kind: event.kind,
            operation: event.entry.operation,
            generation: event.entry.generation,
            status: event.entry.status,
            config_version: event.entry.config_version,
            requested_by: event.entry.requested_by,
            trigger_source: event.entry.trigger_source,
            requested_at_ms: event.entry.requested_at_ms,
            completed_at_ms: event.entry.completed_at_ms,
            event_emitted_at_ms: event.emitted_at_ms,
            summary: event.entry.summary,
        }
    }
}

impl ControlApiQuotaBackendErrorPayload {
    fn from_snapshot(snapshot: QuotaBackendErrorSnapshot) -> Self {
        Self {
            observed_at_unix_ms: snapshot.observed_at_unix_ms,
            policy_name: snapshot.policy_name,
            reason: snapshot.reason,
            detail: snapshot.detail,
        }
    }
}

impl ControlApiQuotaPolicyPayload {
    fn from_snapshot(snapshot: QuotaPolicyIntrospectionSnapshot) -> Self {
        Self {
            name: snapshot.name,
            route_allowlist: snapshot.route_allowlist,
            selector: ControlApiQuotaSelectorPayload::from_snapshot(snapshot.selector),
            burst: snapshot
                .burst
                .map(ControlApiQuotaWindowPayload::from_snapshot),
            sustained: snapshot
                .sustained
                .map(ControlApiQuotaWindowPayload::from_snapshot),
        }
    }
}

impl ControlApiQuotaSelectorPayload {
    fn from_snapshot(snapshot: QuotaSelectorIntrospectionSnapshot) -> Self {
        Self {
            route: snapshot.route,
            tenant: snapshot.tenant,
            token: snapshot.token,
            client: snapshot.client,
        }
    }
}

impl ControlApiQuotaWindowPayload {
    fn from_snapshot(snapshot: QuotaWindowIntrospectionSnapshot) -> Self {
        Self {
            requests: snapshot.requests,
            window_secs: snapshot.window_secs,
        }
    }
}

fn jwt_provider_payload(
    jwt: &RuntimeJwtAuth,
    jwks_by_source_id: &HashMap<&str, &crate::quic_listener::admission::JwtJwksRuntimeSnapshot>,
) -> ControlApiJwtProviderPayload {
    let issuers = jwt
        .issuer
        .iter()
        .cloned()
        .chain(jwt.issuers.iter().cloned())
        .collect::<Vec<_>>();
    let audiences = jwt
        .audience
        .iter()
        .cloned()
        .chain(jwt.audiences.iter().cloned())
        .collect::<Vec<_>>();
    let jwks_snapshot = jwt
        .jwks_url
        .as_deref()
        .and_then(|_| crate::quic_listener::admission::runtime_jwks_source_identity(jwt))
        .and_then(|source_id| jwks_by_source_id.get(source_id.as_str()).copied());
    let jwks_cache_state = jwks_snapshot.map(|snapshot| snapshot.state);
    let serving_from_stale_cache = jwks_cache_state.map(|state| {
        matches!(
            state,
            "stale" | "refresh_failed_retained" | "quarantined_retained"
        )
    });
    let usable_key_count = jwks_snapshot.map(|snapshot| snapshot.active_key_count);
    let jwks_active = jwks_snapshot.is_some_and(|snapshot| {
        snapshot.active_key_count > 0
            && !matches!(snapshot.state, "never_fetched" | "empty_unusable")
    });

    ControlApiJwtProviderPayload {
        provider_mode: jwt_provider_mode(jwt),
        allowed_algorithms: jwt
            .allowed_algorithms
            .iter()
            .map(|algorithm| jwt_algorithm_name(*algorithm).to_string())
            .collect(),
        require_kid: jwt.require_kid,
        issuers,
        audiences,
        static_key_count: jwt.static_keys.len(),
        jwks_configured: jwt.jwks_url.is_some(),
        jwks_active,
        jwks_cache_state,
        serving_from_stale_cache,
        usable_key_count,
        last_refresh_success_unix_seconds: jwks_snapshot
            .and_then(|snapshot| snapshot.last_refresh_success_unix_seconds),
        last_refresh_attempt_unix_seconds: jwks_snapshot
            .and_then(|snapshot| snapshot.last_refresh_attempt_unix_seconds),
        last_failure_reason: jwks_snapshot
            .and_then(|snapshot| snapshot.last_failure_reason.clone()),
    }
}

fn jwt_provider_mode(jwt: &RuntimeJwtAuth) -> &'static str {
    let has_hs256 = !jwt.secret.is_empty();
    let has_static_asymmetric = !jwt.static_keys.is_empty();
    let has_jwks = jwt.jwks_url.is_some();
    match (has_hs256, has_static_asymmetric, has_jwks) {
        (true, false, false) => "hs256_only",
        (false, true, false) => "static_asymmetric",
        (false, false, true) => "remote_jwks",
        (false, true, true) => "hybrid_asymmetric",
        (true, true, false) | (true, false, true) | (true, true, true) => "hybrid",
        (false, false, false) => "unconfigured",
    }
}

fn jwt_algorithm_name(algorithm: spooky_config::config::JwtAlgorithm) -> &'static str {
    match algorithm {
        spooky_config::config::JwtAlgorithm::Hs256 => "HS256",
        spooky_config::config::JwtAlgorithm::Rs256 => "RS256",
        spooky_config::config::JwtAlgorithm::Es256 => "ES256",
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
