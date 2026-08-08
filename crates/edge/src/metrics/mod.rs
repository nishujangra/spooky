use std::{
    cell::Cell,
    collections::HashMap,
    env,
    sync::{
        RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use spooky_errors::{
    HedgeOutcomeTelemetryReason, HedgeTriggerTelemetryReason, RetryAttemptTelemetryReason,
    RetryPolicyDenialReason,
};
use spooky_lb::health::HealthFailureReason;

use crate::{
    observability::{QuotaBackendHealthReason, QuotaPolicyDecision, QuotaPolicyReason},
    runtime::activation::{RuntimeOperationOutcomeReason, RuntimeRejectionReason},
};

pub struct Metrics {
    pub requests_total: AtomicU64,
    pub requests_success: AtomicU64,
    pub requests_failure: AtomicU64,
    pub request_validation_rejects: AtomicU64,
    pub policy_denied: AtomicU64,
    pub external_auth_allowed: AtomicU64,
    pub external_auth_denied: AtomicU64,
    pub external_auth_timeout: AtomicU64,
    pub external_auth_error: AtomicU64,
    pub request_rate_limited: AtomicU64,
    pub early_data_accepted: AtomicU64,
    pub early_data_rejected: AtomicU64,
    pub health_checks_total: AtomicU64,
    pub health_checks_success: AtomicU64,
    pub health_checks_failure: AtomicU64,
    pub backend_timeouts: AtomicU64,
    pub backend_errors: AtomicU64,
    pub overload_shed: AtomicU64,
    pub overload_shed_brownout: AtomicU64,
    pub overload_shed_adaptive: AtomicU64,
    pub overload_shed_route_cap: AtomicU64,
    pub overload_shed_route_global_cap: AtomicU64,
    pub overload_shed_global_inflight: AtomicU64,
    pub overload_shed_upstream_inflight: AtomicU64,
    pub inflight_wait_admit_global: AtomicU64,
    pub inflight_wait_admit_upstream: AtomicU64,
    pub overload_shed_backend_inflight: AtomicU64,
    pub overload_shed_circuit_open: AtomicU64,
    pub overload_shed_request_buffer: AtomicU64,
    pub overload_shed_response_prebuffer: AtomicU64,
    pub overload_shed_connection_cap: AtomicU64,
    pub active_connections: AtomicU64,
    pub connection_cap_rejects: AtomicU64,
    pub hedge_triggered: AtomicU64,
    pub hedge_won: AtomicU64,
    pub hedge_wasted: AtomicU64,
    pub hedge_primary_won_after_trigger: AtomicU64,
    pub hedge_primary_late_ms_total: AtomicU64,
    pub hedge_primary_late_samples: AtomicU64,
    pub ingress_packets_total: AtomicU64,
    pub ingress_queue_drops: AtomicU64,
    pub ingress_queue_drop_bytes: AtomicU64,
    pub ingress_queue_bytes: AtomicU64,
    pub ingress_bad_header_total: AtomicU64,
    pub ingress_rate_limited_total: AtomicU64,
    pub ingress_unroutable_total: AtomicU64,
    pub ingress_draining_drops_total: AtomicU64,
    pub ingress_connection_create_failed_total: AtomicU64,
    pub ingress_version_neg_failed_total: AtomicU64,
    pub request_buffered_bytes: AtomicU64,
    pub request_buffered_high_watermark_bytes: AtomicU64,
    pub request_buffer_limit_rejects: AtomicU64,
    pub response_prebuffer_limit_rejects: AtomicU64,
    pub scid_rotations: AtomicU64,
    pub control_api_connection_limit_drops: AtomicU64,
    pub watchdog_restart_requests: AtomicU64,
    pub watchdog_restart_hooks: AtomicU64,
    pub watchdog_degraded_windows: AtomicU64,
    pub runtime_panics: AtomicU64,
    pub runtime_rejection_invalid_config: AtomicU64,
    pub runtime_rejection_startup_owned_change: AtomicU64,
    pub runtime_rejection_bind_conflict: AtomicU64,
    pub runtime_rejection_resource_prepare_failed: AtomicU64,
    pub runtime_rejection_incompatible_reload: AtomicU64,
    pub runtime_rejection_unknown_generation: AtomicU64,
    pub runtime_rejection_rollback_not_allowed: AtomicU64,
    pub runtime_validation_attempts: AtomicU64,
    pub runtime_preview_attempts: AtomicU64,
    pub runtime_active_generation: AtomicU64,
    pub runtime_history_depth: AtomicU64,
    runtime_activation_outcomes: [AtomicU64; RuntimeOperationOutcomeReason::COUNT],
    runtime_rollback_outcomes: [AtomicU64; RuntimeOperationOutcomeReason::COUNT],
    pub retries_total: AtomicU64,
    pub retry_denied_budget: AtomicU64,
    pub retry_denied_no_bodyless: AtomicU64,
    pub retry_denied_no_alternate: AtomicU64,
    pub retry_reason_timeout: AtomicU64,
    pub retry_reason_transport: AtomicU64,
    pub retry_reason_pool: AtomicU64,
    pub circuit_breaker_rejected_total: AtomicU64,
    pub brownout_active: AtomicU64,
    pub health_failure_5xx: AtomicU64,
    pub health_failure_timeout: AtomicU64,
    pub health_failure_transport: AtomicU64,
    pub health_failure_tls: AtomicU64,
    pub downstream_tls_handshake_success: AtomicU64,
    pub backend_dns_refresh_success: AtomicU64,
    pub backend_dns_refresh_failure: AtomicU64,
    pub backend_dns_refresh_address_changes: AtomicU64,
    pub backend_client_rotations: AtomicU64,
    pub backend_client_rotation_failures: AtomicU64,
    jwt_validation_failures: RwLock<HashMap<String, u64>>,
    jwt_algorithm_rejections: RwLock<HashMap<String, u64>>,
    jwks_unknown_kid_events: RwLock<HashMap<String, u64>>,
    jwks_source_state: RwLock<HashMap<String, JwksSourceState>>,
    route_latency_sample_every: u64,
    route_latency_sample_counter: AtomicU64,
    route_labels: Vec<String>,
    route_label_to_id: HashMap<String, usize>,
    route_stats: Vec<RouteStatsAtomic>,
    unrouted_route_id: usize,
    worker_labels: Vec<String>,
    worker_stats: Vec<WorkerStatsAtomic>,
    backend_dns_state: RwLock<HashMap<String, BackendDnsState>>,
    backend_rotation_state: RwLock<HashMap<String, BackendRotationState>>,
    backend_connect_attempts: RwLock<HashMap<BackendConnectAttemptKey, u64>>,
    upstream_request_counts: RwLock<HashMap<UpstreamRequestCountKey, u64>>,
    backend_request_counts: RwLock<HashMap<BackendRequestCountKey, u64>>,
    upstream_request_latency: RwLock<HashMap<UpstreamRequestLatencyKey, RequestLatencyStats>>,
    quota_policy_outcomes: RwLock<HashMap<QuotaPolicyOutcomeKey, u64>>,
    quota_backend_health: RwLock<HashMap<QuotaBackendHealthKey, u64>>,
    downstream_tls_handshake_failures: RwLock<HashMap<DownstreamTlsHandshakeFailureKey, u64>>,
    downstream_tls_cert_selections: RwLock<HashMap<DownstreamTlsCertSelectionKey, u64>>,
    downstream_tls_alpn_negotiated: RwLock<HashMap<DownstreamTlsAlpnKey, u64>>,
    downstream_tls_cert_expiry: RwLock<HashMap<DownstreamTlsCertExpiryKey, i64>>,
    upstream_tls_failures: RwLock<HashMap<UpstreamTlsFailureKey, u64>>,
}

#[derive(Default, Clone)]
pub(crate) struct BackendDnsState {
    pub(crate) last_success_unix_seconds: u64,
    pub(crate) resolved_address_count: u64,
}

#[derive(Default, Clone)]
pub(crate) struct BackendRotationState {
    pub(crate) rotations: u64,
}

#[derive(Default, Clone)]
pub(crate) struct JwksSourceState {
    pub(crate) jwks_source_id: String,
    pub(crate) refresh_success_total: u64,
    pub(crate) refresh_failure_total: u64,
    pub(crate) active_key_count: u64,
    pub(crate) state: String,
    pub(crate) last_refresh_attempt_unix_seconds: Option<u64>,
    pub(crate) last_refresh_success_unix_seconds: Option<u64>,
    pub(crate) last_failure_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BackendConnectAttemptKey {
    pub(crate) backend: String,
    pub(crate) hostname: String,
    pub(crate) resolved_addr: String,
}

/// Upper bound on the number of distinct `spooky_backend_connect_attempt_total`
/// series (obs Phase 2, step 7). `resolved_addr`/`hostname` are otherwise
/// unbounded across DNS rotation and multi-A records; once this many distinct
/// keys exist, further keys collapse into a single stable overflow series so
/// label cardinality does not grow with DNS churn.
pub(crate) const BACKEND_CONNECT_ATTEMPT_LABEL_CAP: usize = 512;

/// Stable sentinel used for the `hostname`/`resolved_addr` labels once the
/// cardinality cap is reached.
pub(crate) const METRIC_LABEL_OVER_CAP: &str = "__over_cap__";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UpstreamRequestCountKey {
    pub(crate) upstream: String,
    pub(crate) status_class: String,
    pub(crate) outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BackendRequestCountKey {
    pub(crate) upstream: String,
    pub(crate) backend: String,
    pub(crate) status_class: String,
    pub(crate) outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UpstreamRequestLatencyKey {
    pub(crate) upstream: String,
    pub(crate) outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct QuotaPolicyOutcomeKey {
    pub(crate) policy: String,
    pub(crate) decision: String,
    pub(crate) reason: String,
    pub(crate) selector_dimensions: String,
    pub(crate) backend_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct QuotaBackendHealthKey {
    pub(crate) backend_mode: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DownstreamTlsHandshakeFailureKey {
    pub(crate) listener: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DownstreamTlsCertSelectionKey {
    pub(crate) listener: String,
    pub(crate) selection: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DownstreamTlsAlpnKey {
    pub(crate) listener: String,
    pub(crate) protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UpstreamTlsFailureKey {
    pub(crate) backend: String,
    pub(crate) phase: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DownstreamTlsCertExpiryKey {
    pub(crate) listener: String,
    pub(crate) server_name: String,
}

const LATENCY_BUCKETS_MS: [u64; 14] = [
    1, 5, 10, 25, 50, 100, 250, 500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000,
];
const ROUTE_LATENCY_SAMPLE_EVERY_ENV: &str = "SPOOKY_ROUTE_LATENCY_SAMPLE_EVERY";

#[derive(Default, Clone)]
struct RouteStats {
    requests_total: u64,
    success: u64,
    failure: u64,
    rate_limited: u64,
    timeout: u64,
    backend_error: u64,
    overload_shed: u64,
    latency_buckets: [u64; LATENCY_BUCKETS_MS.len() + 1],
}

#[derive(Default, Clone)]
struct WorkerStats {
    requests_total: u64,
    requests_success: u64,
    requests_failure: u64,
    ingress_packets_total: u64,
    ingress_queue_drops: u64,
    ingress_queue_drop_bytes: u64,
}

#[derive(Default, Clone)]
pub(crate) struct RequestLatencyStats {
    pub(crate) latency_buckets: [u64; LATENCY_BUCKETS_MS.len() + 1],
    pub(crate) latency_ms_sum: u64,
    pub(crate) count: u64,
}

struct RouteStatsAtomic {
    requests_total: AtomicU64,
    success: AtomicU64,
    failure: AtomicU64,
    rate_limited: AtomicU64,
    timeout: AtomicU64,
    backend_error: AtomicU64,
    overload_shed: AtomicU64,
    latency_buckets: [AtomicU64; LATENCY_BUCKETS_MS.len() + 1],
}

impl RouteStatsAtomic {
    fn new() -> Self {
        Self {
            requests_total: AtomicU64::new(0),
            success: AtomicU64::new(0),
            failure: AtomicU64::new(0),
            rate_limited: AtomicU64::new(0),
            timeout: AtomicU64::new(0),
            backend_error: AtomicU64::new(0),
            overload_shed: AtomicU64::new(0),
            latency_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    fn snapshot(&self) -> RouteStats {
        let mut latency_buckets = [0u64; LATENCY_BUCKETS_MS.len() + 1];
        for (idx, bucket) in self.latency_buckets.iter().enumerate() {
            latency_buckets[idx] = bucket.load(Ordering::Relaxed);
        }

        RouteStats {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            success: self.success.load(Ordering::Relaxed),
            failure: self.failure.load(Ordering::Relaxed),
            rate_limited: self.rate_limited.load(Ordering::Relaxed),
            timeout: self.timeout.load(Ordering::Relaxed),
            backend_error: self.backend_error.load(Ordering::Relaxed),
            overload_shed: self.overload_shed.load(Ordering::Relaxed),
            latency_buckets,
        }
    }
}

struct WorkerStatsAtomic {
    requests_total: AtomicU64,
    requests_success: AtomicU64,
    requests_failure: AtomicU64,
    ingress_packets_total: AtomicU64,
    ingress_queue_drops: AtomicU64,
    ingress_queue_drop_bytes: AtomicU64,
}

impl WorkerStatsAtomic {
    fn new() -> Self {
        Self {
            requests_total: AtomicU64::new(0),
            requests_success: AtomicU64::new(0),
            requests_failure: AtomicU64::new(0),
            ingress_packets_total: AtomicU64::new(0),
            ingress_queue_drops: AtomicU64::new(0),
            ingress_queue_drop_bytes: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> WorkerStats {
        WorkerStats {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            requests_success: self.requests_success.load(Ordering::Relaxed),
            requests_failure: self.requests_failure.load(Ordering::Relaxed),
            ingress_packets_total: self.ingress_packets_total.load(Ordering::Relaxed),
            ingress_queue_drops: self.ingress_queue_drops.load(Ordering::Relaxed),
            ingress_queue_drop_bytes: self.ingress_queue_drop_bytes.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum RouteOutcome {
    Success,
    Failure,
    RateLimited,
    Timeout,
    BackendError,
    OverloadShed,
}

fn normalize_metric_label(raw: &str, fallback: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn status_class_label(status: Option<u16>) -> &'static str {
    match status {
        Some(100..=199) => "1xx",
        Some(200..=299) => "2xx",
        Some(300..=399) => "3xx",
        Some(400..=499) => "4xx",
        Some(500..=599) => "5xx",
        Some(_) => "other",
        None => "unknown",
    }
}

fn route_outcome_label(outcome: RouteOutcome) -> &'static str {
    match outcome {
        RouteOutcome::Success => "success",
        RouteOutcome::Failure => "failure",
        RouteOutcome::RateLimited => "rate_limited",
        RouteOutcome::Timeout => "timeout",
        RouteOutcome::BackendError => "backend_error",
        RouteOutcome::OverloadShed => "overload_shed",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverloadShedReason {
    Brownout,
    AdaptiveAdmission,
    RouteCap,
    RouteGlobalCap,
    GlobalInflight,
    UpstreamInflight,
    BackendInflight,
    CircuitOpen,
    RequestBufferCap,
    ResponsePrebufferCap,
    ConnectionCap,
}

impl OverloadShedReason {
    /// Bridge to the canonical overload cause (obs Phase 2, step 5). The
    /// `reason=` label emitted for `spooky_overload_shed_by_reason_total` is the
    /// canonical [`crate::observability::AdmissionOverloadCause::slug`]; this makes
    /// the label vocabulary come from the canonical enum rather than the ad hoc
    /// string literals in `prometheus.rs`.
    pub fn canonical(self) -> crate::observability::AdmissionOverloadCause {
        use crate::observability::AdmissionOverloadCause as C;
        match self {
            Self::Brownout => C::Brownout,
            Self::AdaptiveAdmission => C::AdaptiveAdmission,
            Self::RouteCap => C::RouteCap,
            Self::RouteGlobalCap => C::RouteGlobalCap,
            Self::GlobalInflight => C::GlobalInflight,
            Self::UpstreamInflight => C::UpstreamInflight,
            Self::BackendInflight => C::BackendInflight,
            Self::CircuitOpen => C::CircuitOpen,
            Self::RequestBufferCap => C::RequestBufferCap,
            Self::ResponsePrebufferCap => C::ResponsePrebufferCap,
            Self::ConnectionCap => C::ConnectionCap,
        }
    }

    /// The canonical, stable `reason=` label value for this shed reason.
    pub fn reason_label(self) -> &'static str {
        self.canonical().slug()
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new(1, [String::from("unrouted")])
    }
}

thread_local! {
    static WORKER_METRICS_SLOT: Cell<usize> = const { Cell::new(0) };
}

impl Metrics {
    pub fn new<I>(worker_slots: usize, route_labels: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let route_latency_sample_every = env::var(ROUTE_LATENCY_SAMPLE_EVERY_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1);

        let mut route_labels_dedup = Vec::new();
        let mut route_label_to_id = HashMap::new();
        for raw in route_labels {
            let label = raw.trim();
            if label.is_empty() || route_label_to_id.contains_key(label) {
                continue;
            }
            let id = route_labels_dedup.len();
            route_labels_dedup.push(label.to_string());
            route_label_to_id.insert(label.to_string(), id);
        }
        if !route_label_to_id.contains_key("unrouted") {
            let id = route_labels_dedup.len();
            route_labels_dedup.push("unrouted".to_string());
            route_label_to_id.insert("unrouted".to_string(), id);
        }
        let unrouted_route_id = route_label_to_id.get("unrouted").copied().unwrap_or(0);

        let worker_slots = worker_slots.max(1);
        let worker_labels = (0..worker_slots)
            .map(|idx| format!("worker-{idx}"))
            .collect::<Vec<_>>();
        let worker_stats = (0..worker_slots)
            .map(|_| WorkerStatsAtomic::new())
            .collect::<Vec<_>>();
        let route_stats = route_labels_dedup
            .iter()
            .map(|_| RouteStatsAtomic::new())
            .collect::<Vec<_>>();

        Self {
            requests_total: AtomicU64::new(0),
            requests_success: AtomicU64::new(0),
            requests_failure: AtomicU64::new(0),
            request_validation_rejects: AtomicU64::new(0),
            policy_denied: AtomicU64::new(0),
            external_auth_allowed: AtomicU64::new(0),
            external_auth_denied: AtomicU64::new(0),
            external_auth_timeout: AtomicU64::new(0),
            external_auth_error: AtomicU64::new(0),
            request_rate_limited: AtomicU64::new(0),
            early_data_accepted: AtomicU64::new(0),
            early_data_rejected: AtomicU64::new(0),
            health_checks_total: AtomicU64::new(0),
            health_checks_success: AtomicU64::new(0),
            health_checks_failure: AtomicU64::new(0),
            backend_timeouts: AtomicU64::new(0),
            backend_errors: AtomicU64::new(0),
            overload_shed: AtomicU64::new(0),
            overload_shed_brownout: AtomicU64::new(0),
            overload_shed_adaptive: AtomicU64::new(0),
            overload_shed_route_cap: AtomicU64::new(0),
            overload_shed_route_global_cap: AtomicU64::new(0),
            overload_shed_global_inflight: AtomicU64::new(0),
            overload_shed_upstream_inflight: AtomicU64::new(0),
            inflight_wait_admit_global: AtomicU64::new(0),
            inflight_wait_admit_upstream: AtomicU64::new(0),
            overload_shed_backend_inflight: AtomicU64::new(0),
            overload_shed_circuit_open: AtomicU64::new(0),
            overload_shed_request_buffer: AtomicU64::new(0),
            overload_shed_response_prebuffer: AtomicU64::new(0),
            overload_shed_connection_cap: AtomicU64::new(0),
            active_connections: AtomicU64::new(0),
            connection_cap_rejects: AtomicU64::new(0),
            hedge_triggered: AtomicU64::new(0),
            hedge_won: AtomicU64::new(0),
            hedge_wasted: AtomicU64::new(0),
            hedge_primary_won_after_trigger: AtomicU64::new(0),
            hedge_primary_late_ms_total: AtomicU64::new(0),
            hedge_primary_late_samples: AtomicU64::new(0),
            ingress_packets_total: AtomicU64::new(0),
            ingress_queue_drops: AtomicU64::new(0),
            ingress_queue_drop_bytes: AtomicU64::new(0),
            ingress_queue_bytes: AtomicU64::new(0),
            ingress_bad_header_total: AtomicU64::new(0),
            ingress_rate_limited_total: AtomicU64::new(0),
            ingress_unroutable_total: AtomicU64::new(0),
            ingress_draining_drops_total: AtomicU64::new(0),
            ingress_connection_create_failed_total: AtomicU64::new(0),
            ingress_version_neg_failed_total: AtomicU64::new(0),
            request_buffered_bytes: AtomicU64::new(0),
            request_buffered_high_watermark_bytes: AtomicU64::new(0),
            request_buffer_limit_rejects: AtomicU64::new(0),
            response_prebuffer_limit_rejects: AtomicU64::new(0),
            scid_rotations: AtomicU64::new(0),
            control_api_connection_limit_drops: AtomicU64::new(0),
            watchdog_restart_requests: AtomicU64::new(0),
            watchdog_restart_hooks: AtomicU64::new(0),
            watchdog_degraded_windows: AtomicU64::new(0),
            runtime_panics: AtomicU64::new(0),
            runtime_rejection_invalid_config: AtomicU64::new(0),
            runtime_rejection_startup_owned_change: AtomicU64::new(0),
            runtime_rejection_bind_conflict: AtomicU64::new(0),
            runtime_rejection_resource_prepare_failed: AtomicU64::new(0),
            runtime_rejection_incompatible_reload: AtomicU64::new(0),
            runtime_rejection_unknown_generation: AtomicU64::new(0),
            runtime_rejection_rollback_not_allowed: AtomicU64::new(0),
            runtime_validation_attempts: AtomicU64::new(0),
            runtime_preview_attempts: AtomicU64::new(0),
            runtime_active_generation: AtomicU64::new(0),
            runtime_history_depth: AtomicU64::new(0),
            runtime_activation_outcomes: std::array::from_fn(|_| AtomicU64::new(0)),
            runtime_rollback_outcomes: std::array::from_fn(|_| AtomicU64::new(0)),
            retries_total: AtomicU64::new(0),
            retry_denied_budget: AtomicU64::new(0),
            retry_denied_no_bodyless: AtomicU64::new(0),
            retry_denied_no_alternate: AtomicU64::new(0),
            retry_reason_timeout: AtomicU64::new(0),
            retry_reason_transport: AtomicU64::new(0),
            retry_reason_pool: AtomicU64::new(0),
            circuit_breaker_rejected_total: AtomicU64::new(0),
            brownout_active: AtomicU64::new(0),
            health_failure_5xx: AtomicU64::new(0),
            health_failure_timeout: AtomicU64::new(0),
            health_failure_transport: AtomicU64::new(0),
            health_failure_tls: AtomicU64::new(0),
            downstream_tls_handshake_success: AtomicU64::new(0),
            backend_dns_refresh_success: AtomicU64::new(0),
            backend_dns_refresh_failure: AtomicU64::new(0),
            backend_dns_refresh_address_changes: AtomicU64::new(0),
            backend_client_rotations: AtomicU64::new(0),
            backend_client_rotation_failures: AtomicU64::new(0),
            jwt_validation_failures: RwLock::new(HashMap::new()),
            jwt_algorithm_rejections: RwLock::new(HashMap::new()),
            jwks_unknown_kid_events: RwLock::new(HashMap::new()),
            jwks_source_state: RwLock::new(HashMap::new()),
            route_latency_sample_every,
            route_latency_sample_counter: AtomicU64::new(0),
            route_labels: route_labels_dedup,
            route_label_to_id,
            route_stats,
            unrouted_route_id,
            worker_labels,
            worker_stats,
            backend_dns_state: RwLock::new(HashMap::new()),
            backend_rotation_state: RwLock::new(HashMap::new()),
            backend_connect_attempts: RwLock::new(HashMap::new()),
            upstream_request_counts: RwLock::new(HashMap::new()),
            backend_request_counts: RwLock::new(HashMap::new()),
            upstream_request_latency: RwLock::new(HashMap::new()),
            quota_policy_outcomes: RwLock::new(HashMap::new()),
            quota_backend_health: RwLock::new(HashMap::new()),
            downstream_tls_handshake_failures: RwLock::new(HashMap::new()),
            downstream_tls_cert_selections: RwLock::new(HashMap::new()),
            downstream_tls_alpn_negotiated: RwLock::new(HashMap::new()),
            downstream_tls_cert_expiry: RwLock::new(HashMap::new()),
            upstream_tls_failures: RwLock::new(HashMap::new()),
        }
    }

    pub fn bind_worker_slot(&self, slot: usize) {
        let max_index = self.worker_stats.len().saturating_sub(1);
        WORKER_METRICS_SLOT.with(|current| current.set(slot.min(max_index)));
    }

    pub fn inc_total(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        self.inc_worker_requests_total();
    }

    pub fn inc_success(&self) {
        self.requests_success.fetch_add(1, Ordering::Relaxed);
        self.inc_worker_requests_success();
    }

    pub fn inc_failure(&self) {
        self.requests_failure.fetch_add(1, Ordering::Relaxed);
        self.inc_worker_requests_failure();
    }

    pub fn inc_request_validation_reject(&self) {
        self.request_validation_rejects
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_policy_denied(&self) {
        self.policy_denied.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_external_auth_allowed(&self) {
        self.external_auth_allowed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_external_auth_denied(&self) {
        self.external_auth_denied.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_external_auth_timeout(&self) {
        self.external_auth_timeout.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_external_auth_error(&self) {
        self.external_auth_error.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_request_rate_limited(&self) {
        self.request_rate_limited.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_quota_policy_outcome(
        &self,
        policy: &str,
        decision: QuotaPolicyDecision,
        reason: QuotaPolicyReason,
        selector_dimensions: &str,
        backend_mode: &str,
    ) {
        if let Ok(mut guard) = self.quota_policy_outcomes.write() {
            let key = QuotaPolicyOutcomeKey {
                policy: policy.to_string(),
                decision: decision.slug().to_string(),
                reason: reason.slug().to_string(),
                selector_dimensions: selector_dimensions.to_string(),
                backend_mode: backend_mode.to_string(),
            };
            *guard.entry(key).or_default() += 1;
        }
    }

    pub fn record_quota_backend_health(
        &self,
        backend_mode: &str,
        reason: QuotaBackendHealthReason,
    ) {
        if let Ok(mut guard) = self.quota_backend_health.write() {
            let key = QuotaBackendHealthKey {
                backend_mode: backend_mode.to_string(),
                reason: reason.slug().to_string(),
            };
            *guard.entry(key).or_default() += 1;
        }
    }

    pub fn inc_early_data_accepted(&self) {
        self.early_data_accepted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_early_data_rejected(&self) {
        self.early_data_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_health_check_success(&self) {
        self.health_checks_total.fetch_add(1, Ordering::Relaxed);
        self.health_checks_success.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_health_check_failure(&self) {
        self.health_checks_total.fetch_add(1, Ordering::Relaxed);
        self.health_checks_failure.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_timeout(&self) {
        self.backend_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_backend_error(&self) {
        self.backend_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_overload_shed(&self) {
        self.overload_shed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_overload_shed_reason(&self, reason: OverloadShedReason) {
        self.overload_shed.fetch_add(1, Ordering::Relaxed);
        match reason {
            OverloadShedReason::Brownout => {
                self.overload_shed_brownout.fetch_add(1, Ordering::Relaxed);
            }
            OverloadShedReason::AdaptiveAdmission => {
                self.overload_shed_adaptive.fetch_add(1, Ordering::Relaxed);
            }
            OverloadShedReason::RouteCap => {
                self.overload_shed_route_cap.fetch_add(1, Ordering::Relaxed);
            }
            OverloadShedReason::RouteGlobalCap => {
                self.overload_shed_route_global_cap
                    .fetch_add(1, Ordering::Relaxed);
            }
            OverloadShedReason::GlobalInflight => {
                self.overload_shed_global_inflight
                    .fetch_add(1, Ordering::Relaxed);
            }
            OverloadShedReason::UpstreamInflight => {
                self.overload_shed_upstream_inflight
                    .fetch_add(1, Ordering::Relaxed);
            }
            OverloadShedReason::BackendInflight => {
                self.overload_shed_backend_inflight
                    .fetch_add(1, Ordering::Relaxed);
            }
            OverloadShedReason::CircuitOpen => {
                self.overload_shed_circuit_open
                    .fetch_add(1, Ordering::Relaxed);
            }
            OverloadShedReason::RequestBufferCap => {
                self.overload_shed_request_buffer
                    .fetch_add(1, Ordering::Relaxed);
            }
            OverloadShedReason::ResponsePrebufferCap => {
                self.overload_shed_response_prebuffer
                    .fetch_add(1, Ordering::Relaxed);
            }
            OverloadShedReason::ConnectionCap => {
                self.overload_shed_connection_cap
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn set_active_connections(&self, count: usize) {
        self.active_connections
            .store(count as u64, Ordering::Relaxed);
    }

    pub fn inc_connection_cap_reject(&self) {
        self.connection_cap_rejects.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_inflight_wait_admit_global(&self) {
        self.inflight_wait_admit_global
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_inflight_wait_admit_upstream(&self) {
        self.inflight_wait_admit_upstream
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_hedge_trigger(&self, _reason: HedgeTriggerTelemetryReason) {
        self.hedge_triggered.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_hedge_outcome(&self, reason: HedgeOutcomeTelemetryReason) {
        match reason {
            HedgeOutcomeTelemetryReason::PrimaryWonAfterTrigger => {
                self.hedge_primary_won_after_trigger
                    .fetch_add(1, Ordering::Relaxed);
                self.hedge_wasted.fetch_add(1, Ordering::Relaxed);
            }
            HedgeOutcomeTelemetryReason::HedgeWon => {
                self.hedge_won.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn observe_hedge_primary_late_ms(&self, late_ms: u64) {
        self.hedge_primary_late_ms_total
            .fetch_add(late_ms, Ordering::Relaxed);
        self.hedge_primary_late_samples
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_ingress_packet(&self) {
        self.ingress_packets_total.fetch_add(1, Ordering::Relaxed);
        self.inc_worker_ingress_packets_total();
    }

    pub fn inc_ingress_queue_drop(&self) {
        self.ingress_queue_drops.fetch_add(1, Ordering::Relaxed);
        self.inc_worker_ingress_queue_drops();
    }

    pub fn inc_ingress_queue_drop_bytes(&self, bytes: usize) {
        self.ingress_queue_drop_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        self.inc_worker_ingress_queue_drop_bytes(bytes as u64);
    }

    pub fn set_ingress_queue_bytes(&self, bytes: usize) {
        self.ingress_queue_bytes
            .store(bytes as u64, Ordering::Relaxed);
    }

    pub fn inc_ingress_bad_header(&self) {
        self.ingress_bad_header_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_ingress_rate_limited(&self) {
        self.ingress_rate_limited_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_ingress_unroutable(&self) {
        self.ingress_unroutable_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_ingress_draining_drop(&self) {
        self.ingress_draining_drops_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_ingress_connection_create_failed(&self) {
        self.ingress_connection_create_failed_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_ingress_version_neg_failed(&self) {
        self.ingress_version_neg_failed_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_backend_dns_refresh_success(
        &self,
        backend: &str,
        refreshed_at: SystemTime,
        resolved_address_count: usize,
        changed: bool,
    ) {
        self.backend_dns_refresh_success
            .fetch_add(1, Ordering::Relaxed);
        if changed {
            self.backend_dns_refresh_address_changes
                .fetch_add(1, Ordering::Relaxed);
        }

        let last_success_unix_seconds = refreshed_at
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        if let Ok(mut guard) = self.backend_dns_state.write() {
            guard.insert(
                backend.to_string(),
                BackendDnsState {
                    last_success_unix_seconds,
                    resolved_address_count: resolved_address_count as u64,
                },
            );
        }
    }

    pub fn inc_backend_dns_refresh_failure(&self) {
        self.backend_dns_refresh_failure
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_backend_client_rotation(&self, backend: &str) {
        self.backend_client_rotations
            .fetch_add(1, Ordering::Relaxed);
        if let Ok(mut guard) = self.backend_rotation_state.write() {
            guard.entry(backend.to_string()).or_default().rotations += 1;
        }
    }

    /// Count a failed attempt to rotate pooled clients after a backend resolution
    /// change. The DNS refresh itself still succeeded; this surfaces that stale
    /// pooled connections may persist until they idle out.
    pub fn inc_backend_client_rotation_failure(&self) {
        self.backend_client_rotation_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_jwt_validation_failure(&self, reason: &str) {
        if let Ok(mut guard) = self.jwt_validation_failures.write() {
            *guard.entry(reason.to_string()).or_default() += 1;
        }
    }

    pub fn record_jwt_algorithm_rejection(&self, algorithm: &str) {
        if let Ok(mut guard) = self.jwt_algorithm_rejections.write() {
            *guard.entry(algorithm.to_string()).or_default() += 1;
        }
    }

    pub fn record_jwks_unknown_kid(&self, jwks_source_id: &str) {
        if let Ok(mut guard) = self.jwks_unknown_kid_events.write() {
            *guard.entry(jwks_source_id.to_string()).or_default() += 1;
        }
    }

    pub fn record_jwks_refresh_started(&self, jwks_source_id: &str, refreshed_at: SystemTime) {
        let refreshed_at = refreshed_at
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs());
        if let Ok(mut guard) = self.jwks_source_state.write() {
            let entry =
                guard
                    .entry(jwks_source_id.to_string())
                    .or_insert_with(|| JwksSourceState {
                        jwks_source_id: jwks_source_id.to_string(),
                        ..JwksSourceState::default()
                    });
            entry.last_refresh_attempt_unix_seconds = refreshed_at;
        }
    }

    pub fn record_jwks_refresh_success(
        &self,
        jwks_source_id: &str,
        state: &str,
        active_key_count: usize,
        refreshed_at: SystemTime,
        last_success_at: Option<SystemTime>,
    ) {
        let refreshed_at = refreshed_at
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs());
        let last_success_at = last_success_at
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs());
        if let Ok(mut guard) = self.jwks_source_state.write() {
            let entry =
                guard
                    .entry(jwks_source_id.to_string())
                    .or_insert_with(|| JwksSourceState {
                        jwks_source_id: jwks_source_id.to_string(),
                        ..JwksSourceState::default()
                    });
            entry.refresh_success_total = entry.refresh_success_total.saturating_add(1);
            entry.active_key_count = active_key_count as u64;
            entry.state = state.to_string();
            entry.last_refresh_attempt_unix_seconds = refreshed_at;
            entry.last_refresh_success_unix_seconds = last_success_at.or(refreshed_at);
            entry.last_failure_reason = None;
        }
    }

    pub fn record_jwks_refresh_failure(
        &self,
        jwks_source_id: &str,
        state: &str,
        active_key_count: usize,
        refreshed_at: SystemTime,
        last_success_at: Option<SystemTime>,
        failure_reason: Option<&str>,
    ) {
        let refreshed_at = refreshed_at
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs());
        let last_success_at = last_success_at
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs());
        if let Ok(mut guard) = self.jwks_source_state.write() {
            let entry =
                guard
                    .entry(jwks_source_id.to_string())
                    .or_insert_with(|| JwksSourceState {
                        jwks_source_id: jwks_source_id.to_string(),
                        ..JwksSourceState::default()
                    });
            entry.refresh_failure_total = entry.refresh_failure_total.saturating_add(1);
            entry.active_key_count = active_key_count as u64;
            entry.state = state.to_string();
            entry.last_refresh_attempt_unix_seconds = refreshed_at;
            if let Some(last_success_at) = last_success_at {
                entry.last_refresh_success_unix_seconds = Some(last_success_at);
            }
            entry.last_failure_reason = failure_reason.map(ToOwned::to_owned);
        }
    }

    pub fn record_backend_connect(
        &self,
        backend: &str,
        hostname: &str,
        resolved_addr: std::net::SocketAddr,
    ) {
        if let Ok(mut guard) = self.backend_connect_attempts.write() {
            let key = BackendConnectAttemptKey {
                backend: backend.to_string(),
                hostname: hostname.to_string(),
                resolved_addr: resolved_addr.to_string(),
            };
            // Phase 2 (step 7): bound label cardinality. Existing series always
            // update; a new series is only created while under the cap, otherwise
            // it folds into a stable overflow key keyed only by backend identity
            // (which is bounded by the configured backend set).
            if guard.contains_key(&key) || guard.len() < BACKEND_CONNECT_ATTEMPT_LABEL_CAP {
                *guard.entry(key).or_default() += 1;
            } else {
                *guard
                    .entry(BackendConnectAttemptKey {
                        backend: backend.to_string(),
                        hostname: METRIC_LABEL_OVER_CAP.to_string(),
                        resolved_addr: METRIC_LABEL_OVER_CAP.to_string(),
                    })
                    .or_default() += 1;
            }
        }
    }

    pub fn record_request_result(
        &self,
        upstream: &str,
        backend: Option<&str>,
        status: Option<u16>,
        outcome: RouteOutcome,
        latency: Duration,
    ) {
        let upstream = normalize_metric_label(upstream, "unrouted");
        let backend = normalize_metric_label(backend.unwrap_or("__none__"), "__none__");
        let status_class = status_class_label(status).to_string();
        let outcome = route_outcome_label(outcome).to_string();

        if let Ok(mut guard) = self.upstream_request_counts.write() {
            *guard
                .entry(UpstreamRequestCountKey {
                    upstream: upstream.clone(),
                    status_class: status_class.clone(),
                    outcome: outcome.clone(),
                })
                .or_default() += 1;
        }

        if let Ok(mut guard) = self.backend_request_counts.write() {
            *guard
                .entry(BackendRequestCountKey {
                    upstream: upstream.clone(),
                    backend,
                    status_class,
                    outcome: outcome.clone(),
                })
                .or_default() += 1;
        }

        let latency_ms = latency.as_millis() as u64;
        let bucket = LATENCY_BUCKETS_MS
            .iter()
            .position(|cutoff| latency_ms <= *cutoff)
            .unwrap_or(LATENCY_BUCKETS_MS.len());
        if let Ok(mut guard) = self.upstream_request_latency.write() {
            let stats = guard
                .entry(UpstreamRequestLatencyKey { upstream, outcome })
                .or_default();
            stats.count = stats.count.saturating_add(1);
            stats.latency_ms_sum = stats.latency_ms_sum.saturating_add(latency_ms);
            stats.latency_buckets[bucket] = stats.latency_buckets[bucket].saturating_add(1);
        }
    }

    pub(crate) fn snapshot_backend_dns_state(&self) -> Vec<(String, BackendDnsState)> {
        self.backend_dns_state
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(backend, state)| (backend.clone(), state.clone()))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_backend_rotation_state(&self) -> Vec<(String, BackendRotationState)> {
        self.backend_rotation_state
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(backend, state)| (backend.clone(), state.clone()))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_jwt_validation_failures(&self) -> Vec<(String, u64)> {
        self.jwt_validation_failures
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(reason, value)| (reason.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_jwt_algorithm_rejections(&self) -> Vec<(String, u64)> {
        self.jwt_algorithm_rejections
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(algorithm, value)| (algorithm.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_jwks_unknown_kid_events(&self) -> Vec<(String, u64)> {
        self.jwks_unknown_kid_events
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(jwks_source_id, value)| (jwks_source_id.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_jwks_source_state(&self) -> Vec<JwksSourceState> {
        self.jwks_source_state
            .read()
            .map(|guard| {
                let mut entries = guard.values().cloned().collect::<Vec<_>>();
                entries.sort_by(|left, right| left.jwks_source_id.cmp(&right.jwks_source_id));
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_backend_connect_attempts(&self) -> Vec<(BackendConnectAttemptKey, u64)> {
        self.backend_connect_attempts
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(key, value)| (key.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| {
                    left.backend
                        .cmp(&right.backend)
                        .then_with(|| left.hostname.cmp(&right.hostname))
                        .then_with(|| left.resolved_addr.cmp(&right.resolved_addr))
                });
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_upstream_request_counts(&self) -> Vec<(UpstreamRequestCountKey, u64)> {
        self.upstream_request_counts
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(key, value)| (key.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| {
                    left.upstream
                        .cmp(&right.upstream)
                        .then_with(|| left.status_class.cmp(&right.status_class))
                        .then_with(|| left.outcome.cmp(&right.outcome))
                });
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_backend_request_counts(&self) -> Vec<(BackendRequestCountKey, u64)> {
        self.backend_request_counts
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(key, value)| (key.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| {
                    left.upstream
                        .cmp(&right.upstream)
                        .then_with(|| left.backend.cmp(&right.backend))
                        .then_with(|| left.status_class.cmp(&right.status_class))
                        .then_with(|| left.outcome.cmp(&right.outcome))
                });
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_upstream_request_latency(
        &self,
    ) -> Vec<(UpstreamRequestLatencyKey, RequestLatencyStats)> {
        self.upstream_request_latency
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| {
                    left.upstream
                        .cmp(&right.upstream)
                        .then_with(|| left.outcome.cmp(&right.outcome))
                });
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_downstream_tls_handshake_failures(
        &self,
    ) -> Vec<(DownstreamTlsHandshakeFailureKey, u64)> {
        self.downstream_tls_handshake_failures
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(key, value)| (key.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| {
                    left.listener
                        .cmp(&right.listener)
                        .then_with(|| left.reason.cmp(&right.reason))
                });
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_downstream_tls_cert_selections(
        &self,
    ) -> Vec<(DownstreamTlsCertSelectionKey, u64)> {
        self.downstream_tls_cert_selections
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(key, value)| (key.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| {
                    left.listener
                        .cmp(&right.listener)
                        .then_with(|| left.selection.cmp(&right.selection))
                });
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_downstream_tls_alpn(&self) -> Vec<(DownstreamTlsAlpnKey, u64)> {
        self.downstream_tls_alpn_negotiated
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(key, value)| (key.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| {
                    left.listener
                        .cmp(&right.listener)
                        .then_with(|| left.protocol.cmp(&right.protocol))
                });
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_upstream_tls_failures(&self) -> Vec<(UpstreamTlsFailureKey, u64)> {
        self.upstream_tls_failures
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(key, value)| (key.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| {
                    left.backend
                        .cmp(&right.backend)
                        .then_with(|| left.phase.cmp(&right.phase))
                        .then_with(|| left.reason.cmp(&right.reason))
                });
                entries
            })
            .unwrap_or_default()
    }

    pub(crate) fn snapshot_downstream_tls_cert_expiry(
        &self,
    ) -> Vec<(DownstreamTlsCertExpiryKey, i64)> {
        self.downstream_tls_cert_expiry
            .read()
            .map(|guard| {
                let mut entries = guard
                    .iter()
                    .map(|(key, value)| (key.clone(), *value))
                    .collect::<Vec<_>>();
                entries.sort_by(|(left, _), (right, _)| {
                    left.listener
                        .cmp(&right.listener)
                        .then_with(|| left.server_name.cmp(&right.server_name))
                });
                entries
            })
            .unwrap_or_default()
    }

    fn current_worker_stats(&self) -> Option<&WorkerStatsAtomic> {
        let idx = WORKER_METRICS_SLOT.with(|current| current.get());
        self.worker_stats
            .get(idx)
            .or_else(|| self.worker_stats.first())
    }

    fn inc_worker_requests_total(&self) {
        if let Some(stats) = self.current_worker_stats() {
            stats.requests_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn inc_worker_requests_success(&self) {
        if let Some(stats) = self.current_worker_stats() {
            stats.requests_success.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn inc_worker_requests_failure(&self) {
        if let Some(stats) = self.current_worker_stats() {
            stats.requests_failure.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn inc_worker_ingress_packets_total(&self) {
        if let Some(stats) = self.current_worker_stats() {
            stats.ingress_packets_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn inc_worker_ingress_queue_drops(&self) {
        if let Some(stats) = self.current_worker_stats() {
            stats.ingress_queue_drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn inc_worker_ingress_queue_drop_bytes(&self, bytes: u64) {
        if let Some(stats) = self.current_worker_stats() {
            stats
                .ingress_queue_drop_bytes
                .fetch_add(bytes, Ordering::Relaxed);
        }
    }

    pub fn try_reserve_request_buffer(&self, bytes: usize, cap_bytes: usize) -> bool {
        let add = bytes as u64;
        let cap = cap_bytes as u64;
        loop {
            let current = self.request_buffered_bytes.load(Ordering::Relaxed);
            let next = current.saturating_add(add);
            if next > cap {
                return false;
            }
            if self
                .request_buffered_bytes
                .compare_exchange(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                self.observe_request_buffer_high_water(next);
                return true;
            }
        }
    }

    pub fn release_request_buffer(&self, bytes: usize) {
        let sub = bytes as u64;
        loop {
            let current = self.request_buffered_bytes.load(Ordering::Relaxed);
            let next = current.saturating_sub(sub);
            if self
                .request_buffered_bytes
                .compare_exchange(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    pub fn inc_request_buffer_limit_reject(&self) {
        self.request_buffer_limit_rejects
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_response_prebuffer_limit_reject(&self) {
        self.response_prebuffer_limit_rejects
            .fetch_add(1, Ordering::Relaxed);
    }

    fn observe_request_buffer_high_water(&self, candidate: u64) {
        loop {
            let current = self
                .request_buffered_high_watermark_bytes
                .load(Ordering::Relaxed);
            if candidate <= current {
                return;
            }
            if self
                .request_buffered_high_watermark_bytes
                .compare_exchange(current, candidate, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    pub fn inc_scid_rotation(&self) {
        self.scid_rotations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_control_api_connection_limit_drop(&self) {
        self.control_api_connection_limit_drops
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_runtime_rejection_reason(&self, reason: RuntimeRejectionReason) {
        match reason {
            RuntimeRejectionReason::InvalidConfig => {
                self.runtime_rejection_invalid_config
                    .fetch_add(1, Ordering::Relaxed);
            }
            RuntimeRejectionReason::StartupOwnedChange => {
                self.runtime_rejection_startup_owned_change
                    .fetch_add(1, Ordering::Relaxed);
            }
            RuntimeRejectionReason::BindConflict => {
                self.runtime_rejection_bind_conflict
                    .fetch_add(1, Ordering::Relaxed);
            }
            RuntimeRejectionReason::ResourcePrepareFailed => {
                self.runtime_rejection_resource_prepare_failed
                    .fetch_add(1, Ordering::Relaxed);
            }
            RuntimeRejectionReason::IncompatibleReload => {
                self.runtime_rejection_incompatible_reload
                    .fetch_add(1, Ordering::Relaxed);
            }
            RuntimeRejectionReason::UnknownGeneration => {
                self.runtime_rejection_unknown_generation
                    .fetch_add(1, Ordering::Relaxed);
            }
            RuntimeRejectionReason::RollbackNotAllowed => {
                self.runtime_rejection_rollback_not_allowed
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn inc_runtime_validation_attempt(&self) {
        self.runtime_validation_attempts
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_runtime_preview_attempt(&self) {
        self.runtime_preview_attempts
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_runtime_activation_outcome(&self, reason: RuntimeOperationOutcomeReason) {
        self.runtime_activation_outcomes[reason.index()].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_runtime_rollback_outcome(&self, reason: RuntimeOperationOutcomeReason) {
        self.runtime_rollback_outcomes[reason.index()].fetch_add(1, Ordering::Relaxed);
    }

    pub fn runtime_rejection_reason_count(&self, reason: RuntimeRejectionReason) -> u64 {
        match reason {
            RuntimeRejectionReason::InvalidConfig => self
                .runtime_rejection_invalid_config
                .load(Ordering::Relaxed),
            RuntimeRejectionReason::StartupOwnedChange => self
                .runtime_rejection_startup_owned_change
                .load(Ordering::Relaxed),
            RuntimeRejectionReason::BindConflict => {
                self.runtime_rejection_bind_conflict.load(Ordering::Relaxed)
            }
            RuntimeRejectionReason::ResourcePrepareFailed => self
                .runtime_rejection_resource_prepare_failed
                .load(Ordering::Relaxed),
            RuntimeRejectionReason::IncompatibleReload => self
                .runtime_rejection_incompatible_reload
                .load(Ordering::Relaxed),
            RuntimeRejectionReason::UnknownGeneration => self
                .runtime_rejection_unknown_generation
                .load(Ordering::Relaxed),
            RuntimeRejectionReason::RollbackNotAllowed => self
                .runtime_rejection_rollback_not_allowed
                .load(Ordering::Relaxed),
        }
    }

    pub fn runtime_activation_outcome_count(&self, reason: RuntimeOperationOutcomeReason) -> u64 {
        self.runtime_activation_outcomes[reason.index()].load(Ordering::Relaxed)
    }

    pub fn runtime_rollback_outcome_count(&self, reason: RuntimeOperationOutcomeReason) -> u64 {
        self.runtime_rollback_outcomes[reason.index()].load(Ordering::Relaxed)
    }

    pub fn set_runtime_active_generation(&self, generation: u64) {
        self.runtime_active_generation
            .store(generation, Ordering::Relaxed);
    }

    pub fn set_runtime_history_depth(&self, depth: usize) {
        let depth = u64::try_from(depth).unwrap_or(u64::MAX);
        self.runtime_history_depth.store(depth, Ordering::Relaxed);
    }

    pub fn inc_watchdog_restart_request(&self) {
        self.watchdog_restart_requests
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_watchdog_restart_hook(&self) {
        self.watchdog_restart_hooks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_watchdog_degraded_window(&self) {
        self.watchdog_degraded_windows
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_runtime_panic(&self) {
        self.runtime_panics.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_retry_attempt(&self, reason: RetryAttemptTelemetryReason) {
        self.retries_total.fetch_add(1, Ordering::Relaxed);
        match reason {
            RetryAttemptTelemetryReason::Timeout => {
                self.retry_reason_timeout.fetch_add(1, Ordering::Relaxed);
            }
            RetryAttemptTelemetryReason::Transport => {
                self.retry_reason_transport.fetch_add(1, Ordering::Relaxed);
            }
            RetryAttemptTelemetryReason::Pool => {
                self.retry_reason_pool.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn inc_retry_denied(&self, reason: RetryPolicyDenialReason) {
        match reason {
            RetryPolicyDenialReason::BudgetDenied => {
                self.retry_denied_budget.fetch_add(1, Ordering::Relaxed);
            }
            RetryPolicyDenialReason::MethodNotIdempotent
            | RetryPolicyDenialReason::RequestBodyNotReplayable => {
                self.retry_denied_no_bodyless
                    .fetch_add(1, Ordering::Relaxed);
            }
            RetryPolicyDenialReason::AlternateBackendUnavailable(_) => {
                self.retry_denied_no_alternate
                    .fetch_add(1, Ordering::Relaxed);
            }
            RetryPolicyDenialReason::TerminalError(_)
            | RetryPolicyDenialReason::AttemptLimitReached => {}
        }
    }

    pub fn inc_circuit_breaker_rejected(&self) {
        self.circuit_breaker_rejected_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_brownout_active(&self, active: bool) {
        self.brownout_active
            .store(if active { 1 } else { 0 }, Ordering::Relaxed);
    }

    pub fn inc_health_failure(&self, reason: HealthFailureReason) {
        match reason {
            HealthFailureReason::HttpStatus5xx => {
                self.health_failure_5xx.fetch_add(1, Ordering::Relaxed);
            }
            HealthFailureReason::Timeout => {
                self.health_failure_timeout.fetch_add(1, Ordering::Relaxed);
            }
            HealthFailureReason::Transport => {
                self.health_failure_transport
                    .fetch_add(1, Ordering::Relaxed);
            }
            HealthFailureReason::Tls => {
                self.health_failure_tls.fetch_add(1, Ordering::Relaxed);
            }
            HealthFailureReason::CircuitOpen => {}
        }
    }

    pub fn inc_downstream_tls_handshake_success(&self) {
        self.downstream_tls_handshake_success
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_downstream_tls_handshake_failure(&self, listener: &str, reason: &str) {
        if let Ok(mut guard) = self.downstream_tls_handshake_failures.write() {
            *guard
                .entry(DownstreamTlsHandshakeFailureKey {
                    listener: listener.to_string(),
                    reason: reason.to_string(),
                })
                .or_default() += 1;
        }
    }

    pub fn record_downstream_tls_cert_selection(&self, listener: &str, selection: &str) {
        if let Ok(mut guard) = self.downstream_tls_cert_selections.write() {
            *guard
                .entry(DownstreamTlsCertSelectionKey {
                    listener: listener.to_string(),
                    selection: selection.to_string(),
                })
                .or_default() += 1;
        }
    }

    pub fn record_downstream_tls_alpn(&self, listener: &str, protocol: &str) {
        if let Ok(mut guard) = self.downstream_tls_alpn_negotiated.write() {
            *guard
                .entry(DownstreamTlsAlpnKey {
                    listener: listener.to_string(),
                    protocol: protocol.to_string(),
                })
                .or_default() += 1;
        }
    }

    pub fn record_upstream_tls_failure(&self, backend: &str, phase: &str, reason: &str) {
        if let Ok(mut guard) = self.upstream_tls_failures.write() {
            *guard
                .entry(UpstreamTlsFailureKey {
                    backend: backend.to_string(),
                    phase: phase.to_string(),
                    reason: reason.to_string(),
                })
                .or_default() += 1;
        }
    }

    pub fn replace_downstream_tls_cert_expiry<I>(&self, listener: &str, certs: I)
    where
        I: IntoIterator<Item = (String, i64)>,
    {
        if let Ok(mut guard) = self.downstream_tls_cert_expiry.write() {
            guard.retain(|key, _| key.listener != listener);
            for (server_name, not_after_unix_seconds) in certs {
                guard.insert(
                    DownstreamTlsCertExpiryKey {
                        listener: listener.to_string(),
                        server_name,
                    },
                    not_after_unix_seconds,
                );
            }
        }
    }

    pub fn record_route(&self, route: &str, latency: Duration, outcome: RouteOutcome) {
        let route_id = self
            .route_label_to_id
            .get(route)
            .copied()
            .unwrap_or(self.unrouted_route_id);
        let Some(entry) = self.route_stats.get(route_id) else {
            return;
        };
        entry.requests_total.fetch_add(1, Ordering::Relaxed);

        match outcome {
            RouteOutcome::Success => {
                entry.success.fetch_add(1, Ordering::Relaxed);
            }
            RouteOutcome::Failure => {
                entry.failure.fetch_add(1, Ordering::Relaxed);
            }
            RouteOutcome::RateLimited => {
                entry.rate_limited.fetch_add(1, Ordering::Relaxed);
            }
            RouteOutcome::Timeout => {
                entry.timeout.fetch_add(1, Ordering::Relaxed);
            }
            RouteOutcome::BackendError => {
                entry.backend_error.fetch_add(1, Ordering::Relaxed);
            }
            RouteOutcome::OverloadShed => {
                entry.overload_shed.fetch_add(1, Ordering::Relaxed);
            }
        }

        if self.route_latency_sample_every > 1 {
            let seq = self
                .route_latency_sample_counter
                .fetch_add(1, Ordering::Relaxed);
            if !seq.is_multiple_of(self.route_latency_sample_every) {
                return;
            }
        }

        let latency_ms = latency.as_millis() as u64;
        let bucket = LATENCY_BUCKETS_MS
            .iter()
            .position(|cutoff| latency_ms <= *cutoff)
            .unwrap_or(LATENCY_BUCKETS_MS.len());
        entry.latency_buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }
}

mod prometheus;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::observability::{
        QuotaBackendHealthReason, QuotaPolicyDecision, QuotaPolicyReason,
    };

    #[test]
    fn prometheus_render_includes_jwt_and_jwks_observability_series() {
        let metrics = Metrics::default();
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        metrics.record_jwt_validation_failure("issuer_mismatch");
        metrics.record_jwt_algorithm_rejection("RS256");
        metrics.record_jwks_unknown_kid("jwks:example");
        metrics.record_jwks_refresh_started("jwks:example", now);
        metrics.record_jwks_refresh_success("jwks:example", "fresh", 2, now, Some(now));
        metrics.record_jwks_refresh_failure(
            "jwks:example",
            "refresh_failed_retained",
            2,
            now,
            Some(now),
            Some("request_failed"),
        );

        let rendered = metrics.render_prometheus();

        assert!(
            rendered.contains("spooky_jwt_validation_failures_total{reason=\"issuer_mismatch\"} 1")
        );
        assert!(rendered.contains("spooky_jwt_algorithm_rejections_total{algorithm=\"RS256\"} 1"));
        assert!(
            rendered.contains("spooky_jwks_unknown_kid_total{jwks_source_id=\"jwks:example\"} 1")
        );
        assert!(
            rendered
                .contains("spooky_jwks_refresh_success_total{jwks_source_id=\"jwks:example\"} 1")
        );
        assert!(
            rendered
                .contains("spooky_jwks_refresh_failure_total{jwks_source_id=\"jwks:example\"} 1")
        );
        assert!(rendered.contains(
            "spooky_jwks_state{jwks_source_id=\"jwks:example\",state=\"refresh_failed_retained\"} 1"
        ));
        assert!(rendered.contains("spooky_jwks_active_keys{jwks_source_id=\"jwks:example\"} 2"));
    }

    #[test]
    fn prometheus_render_includes_quota_observability_series() {
        let metrics = Metrics::default();
        metrics.record_quota_policy_outcome(
            "tenant-write-quota",
            QuotaPolicyDecision::Denied,
            QuotaPolicyReason::BurstQuotaExhausted,
            "route+tenant+token",
            "redis",
        );
        metrics.record_quota_backend_health("redis", QuotaBackendHealthReason::Available);
        metrics.record_quota_backend_health("redis", QuotaBackendHealthReason::Timeout);

        let rendered = metrics.render_prometheus();

        assert!(rendered.contains(
            "spooky_quota_policy_outcomes_total{policy=\"tenant-write-quota\",decision=\"denied\",reason=\"burst_quota_exhausted\",selector_dimensions=\"route+tenant+token\",backend_mode=\"redis\"} 1"
        ));
        assert!(
            rendered.contains(
                "spooky_quota_backend_health_total{backend_mode=\"redis\",reason=\"available\"} 1"
            )
        );
        assert!(
            rendered
                .contains("spooky_quota_backend_health_total{backend_mode=\"redis\",reason=\"timeout\"} 1")
        );
    }

    #[test]
    fn prometheus_render_includes_degraded_quota_backend_modes() {
        let metrics = Metrics::default();
        metrics.record_quota_policy_outcome(
            "tenant-write-quota",
            QuotaPolicyDecision::Allowed,
            QuotaPolicyReason::Allowed,
            "route+tenant+client",
            "redis_local_fallback_backend_timeout",
        );
        metrics.record_quota_backend_health(
            "redis_local_fallback_backend_timeout",
            QuotaBackendHealthReason::Timeout,
        );

        let rendered = metrics.render_prometheus();

        assert!(rendered.contains(
            "spooky_quota_policy_outcomes_total{policy=\"tenant-write-quota\",decision=\"allowed\",reason=\"allowed\",selector_dimensions=\"route+tenant+client\",backend_mode=\"redis_local_fallback_backend_timeout\"} 1"
        ));
        assert!(rendered.contains(
            "spooky_quota_backend_health_total{backend_mode=\"redis_local_fallback_backend_timeout\",reason=\"timeout\"} 1"
        ));
    }
}
