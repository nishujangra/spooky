//! Canonical activation, preview, and rollback contract for runtime generations.
//!
//! This module intentionally contains *only* the typed domain vocabulary for
//! config validation, reload preview, activation, rollback, and history. It does
//! not perform any runtime mutation itself. Control-plane handlers and future
//! activation services should consume these types rather than inventing
//! endpoint-local request/result payloads.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use spooky_config::{loader::read_config, runtime::RuntimeConfig};

use crate::{
    runtime::{
        bundle::{ActiveRuntimeGeneration, RuntimeBundle},
        generation::CarriedProcessSharedServices,
        listener::QUICListener,
        policy::{TransitionRejection, TransitionRejectionKind, render_rejections},
    },
};

/// Stable identifier for a published or staged runtime generation.
pub type GenerationId = u64;

/// Milliseconds since the Unix epoch.
///
/// Stored as an integer so the activation contract stays transport-agnostic and
/// trivially serializable for control-plane responses and audit history.
pub type TimestampMillis = u64;

/// Raw configuration source for staged validation/planning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReloadConfigInput {
    Path { path: String },
}

impl ReloadConfigInput {
}

/// Coarse kind of generation operation recorded in history.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum GenerationOperation {
    Validate,
    Preview,
    Activate,
    Rollback,
}

/// Lifecycle status for a runtime generation or staged candidate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum GenerationStatus {
    Staged,
    Active,
    Superseded,
    RollbackCandidate,
    RolledBack,
    Rejected,
    Failed,
}

impl GenerationStatus {
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }

    #[must_use]
    pub fn is_rollback_candidate(self) -> bool {
        matches!(self, Self::RollbackCandidate | Self::Superseded)
    }
}

/// Staged validation/planning phase.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PlanningPhase {
    ReadConfig,
    ValidateConfig,
    NormalizeRuntime,
    EvaluateCompatibility,
}

/// Result for an individual staged validation/planning phase.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PlanningPhaseStatus {
    Accepted,
    Rejected,
    Skipped,
}

/// One staged validation/planning outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanningPhaseResult {
    pub phase: PlanningPhase,
    pub status: PlanningPhaseStatus,
    pub summary: String,
}

/// Coarse classification for the compatibility outcome of a staged reload.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReloadCompatibilityClassification {
    NotEvaluated,
    LiveReloadable,
    RestartRequired,
    Rejected,
}

/// Operator-visible summary of the next generation the planner would activate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProposedGenerationSnapshot {
    pub generation: GenerationId,
    pub config_path: String,
    pub log_level: String,
    pub listener_labels: Vec<String>,
    pub upstream_count: usize,
    pub backend_count: usize,
}

/// Kind of field/domain diff detected between the active and candidate
/// generations.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReloadChangeKind {
    Added,
    Modified,
    Removed,
    Unchanged,
}

/// One operator-visible entry in a reload preview diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReloadDiffEntry {
    pub domain: String,
    pub change: ReloadChangeKind,
    pub summary: String,
}

/// Canonical preview diff for a staged reload candidate.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReloadDiff {
    pub entries: Vec<ReloadDiffEntry>,
}

impl ReloadDiff {
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.entries.is_empty()
            || self
                .entries
                .iter()
                .all(|entry| matches!(entry.change, ReloadChangeKind::Unchanged))
    }
}

/// Stable rejection category for preview/activation/rollback flows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RejectedChangeKind {
    RestartRequired,
    ResourcePreparationFailed,
    InvalidConfiguration,
    IllegalTransition,
    RuntimeStateUnavailable,
}

impl From<TransitionRejectionKind> for RejectedChangeKind {
    fn from(value: TransitionRejectionKind) -> Self {
        match value {
            TransitionRejectionKind::RestartRequired => Self::RestartRequired,
            TransitionRejectionKind::ResourcePreparationFailed => {
                Self::ResourcePreparationFailed
            }
            TransitionRejectionKind::InvalidConfiguration => Self::InvalidConfiguration,
            TransitionRejectionKind::IllegalTransition => Self::IllegalTransition,
            TransitionRejectionKind::RuntimeStateUnavailable => Self::RuntimeStateUnavailable,
        }
    }
}

/// Canonical operator-visible rejection payload for a planned change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RejectedChange {
    pub kind: RejectedChangeKind,
    pub field_path: Option<String>,
    pub current_value: Option<String>,
    pub requested_value: Option<String>,
    pub operator_action: String,
    pub active_generation_changed: bool,
    pub message: String,
}

impl From<TransitionRejection> for RejectedChange {
    fn from(value: TransitionRejection) -> Self {
        let message = value.to_string();
        Self {
            kind: value.kind.into(),
            field_path: value.field_path,
            current_value: value.current_mode,
            requested_value: value.requested_mode,
            operator_action: value.operator_action.to_string(),
            active_generation_changed: value.active_runtime_changed,
            message,
        }
    }
}

impl RejectedChange {
    fn invalid_configuration(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            kind: RejectedChangeKind::InvalidConfiguration,
            field_path: None,
            current_value: None,
            requested_value: None,
            operator_action: "fix the configuration and retry the reload".to_string(),
            active_generation_changed: false,
            message,
        }
    }

    fn resource_preparation_failed(field_path: impl Into<String>, detail: impl Into<String>) -> Self {
        let field_path = field_path.into();
        let detail = detail.into();
        let message = format!(
            "runtime reload rejected: could not prepare {field_path}: {detail}; active runtime unchanged (no change applied)"
        );
        Self {
            kind: RejectedChangeKind::ResourcePreparationFailed,
            field_path: Some(field_path),
            current_value: None,
            requested_value: Some(detail),
            operator_action:
                "resolve the resource conflict (e.g. free the address/port) and retry the reload"
                    .to_string(),
            active_generation_changed: false,
            message,
        }
    }
}

/// Activation request metadata.
///
/// This intentionally carries only operator/audit context and concurrency
/// expectations. The actual config source stays outside this step until the
/// activation service is implemented.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivationRequest {
    pub requested_by: Option<String>,
    pub reason: Option<String>,
    pub expected_generation: Option<GenerationId>,
    pub requested_at_ms: TimestampMillis,
}

/// Rollback request metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollbackRequest {
    pub target_generation: GenerationId,
    pub requested_by: Option<String>,
    pub reason: Option<String>,
    pub expected_active_generation: Option<GenerationId>,
    pub requested_at_ms: TimestampMillis,
}

/// Canonical staged plan returned by validate/preview flows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReloadPlan {
    pub request: ActivationRequest,
    pub current_generation: Option<GenerationId>,
    pub candidate_generation: GenerationId,
    pub candidate_status: GenerationStatus,
    pub summary: String,
    pub validation: Vec<PlanningPhaseResult>,
    pub compatibility: ReloadCompatibilityClassification,
    pub candidate_snapshot: Option<ProposedGenerationSnapshot>,
    pub rejection_summary: Option<String>,
    pub diff: ReloadDiff,
    pub rejected_changes: Vec<RejectedChange>,
}

impl ReloadPlan {
    #[must_use]
    pub fn can_activate(&self) -> bool {
        self.rejected_changes.is_empty()
            && self.candidate_snapshot.is_some()
            && self.compatibility == ReloadCompatibilityClassification::LiveReloadable
            && !matches!(self.candidate_status, GenerationStatus::Rejected)
    }

    #[must_use]
    pub fn phase_status(&self, phase: PlanningPhase) -> Option<PlanningPhaseStatus> {
        self.validation
            .iter()
            .find(|result| result.phase == phase)
            .map(|result| result.status)
    }
}

/// Audit/history record for validation, preview, activation, and rollback.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationHistoryEntry {
    pub generation: GenerationId,
    pub operation: GenerationOperation,
    pub status: GenerationStatus,
    pub requested_by: Option<String>,
    pub requested_at_ms: TimestampMillis,
    pub completed_at_ms: Option<TimestampMillis>,
    pub summary: String,
    pub diff: ReloadDiff,
    pub rejected_changes: Vec<RejectedChange>,
}

/// Canonical activation result payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivationResult {
    pub request: ActivationRequest,
    pub active_generation: GenerationId,
    pub activated_generation: Option<GenerationId>,
    pub status: GenerationStatus,
    pub rejected_changes: Vec<RejectedChange>,
    pub history_entry: GenerationHistoryEntry,
}

impl ActivationResult {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.activated_generation.is_some() && self.rejected_changes.is_empty()
    }
}

/// Canonical rollback result payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollbackResult {
    pub request: RollbackRequest,
    pub active_generation: GenerationId,
    pub rolled_back_to: Option<GenerationId>,
    pub status: GenerationStatus,
    pub rejected_changes: Vec<RejectedChange>,
    pub history_entry: GenerationHistoryEntry,
}

impl RollbackResult {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.rolled_back_to.is_some() && self.rejected_changes.is_empty()
    }
}

/// Internal, non-serialized staged runtime reload result.
///
/// This is the pure planning payload that control-plane code can validate and
/// preview before any activation mutates the active bundle.
pub(crate) struct StagedRuntimeReloadPlan {
    pub(crate) plan: ReloadPlan,
    pub(crate) next_runtime: Option<RuntimeBundle>,
    pub(crate) current_log_level: String,
    pub(crate) next_log_level: Option<String>,
}

impl StagedRuntimeReloadPlan {
    #[must_use]
    pub(crate) fn can_activate(&self) -> bool {
        self.plan.can_activate() && self.next_runtime.is_some()
    }
}

pub(crate) fn plan_runtime_reload(
    current: &ActiveRuntimeGeneration,
    request: ActivationRequest,
    input: ReloadConfigInput,
) -> StagedRuntimeReloadPlan {
    let current_generation = Some(current.generation());
    let candidate_generation = current.generation().saturating_add(1);
    let current_log_level = current.startup().log_config.level.clone();
    let mut validation = Vec::with_capacity(4);

    let config = match input {
        ReloadConfigInput::Path { path } => match read_config(&path) {
            Ok(config) => {
                validation.push(PlanningPhaseResult {
                    phase: PlanningPhase::ReadConfig,
                    status: PlanningPhaseStatus::Accepted,
                    summary: format!("read config from '{path}'"),
                });
                config
            }
            Err(err) => {
                validation.push(PlanningPhaseResult {
                    phase: PlanningPhase::ReadConfig,
                    status: PlanningPhaseStatus::Rejected,
                    summary: err.clone(),
                });
                return rejected_reload_plan(
                    request,
                    current_generation,
                    candidate_generation,
                    current_log_level,
                    validation,
                    vec![
                        RejectedChange::invalid_configuration(err),
                    ],
                );
            }
        },
    };

    match spooky_config::validator::validate(&config) {
        Ok(()) => validation.push(PlanningPhaseResult {
            phase: PlanningPhase::ValidateConfig,
            status: PlanningPhaseStatus::Accepted,
            summary: "raw config validation passed".to_string(),
        }),
        Err(err) => {
            let message = format!("Configuration validation failed: {err}");
            validation.push(PlanningPhaseResult {
                phase: PlanningPhase::ValidateConfig,
                status: PlanningPhaseStatus::Rejected,
                summary: message.clone(),
            });
            return rejected_reload_plan(
                request,
                current_generation,
                candidate_generation,
                current_log_level,
                validation,
                vec![RejectedChange::invalid_configuration(message)],
            );
        }
    }

    let runtime_config = match RuntimeConfig::from_config(&config) {
        Ok(runtime_config) => runtime_config,
        Err(err) => {
            let message = format!("Runtime configuration normalization failed: {err}");
            validation.push(PlanningPhaseResult {
                phase: PlanningPhase::NormalizeRuntime,
                status: PlanningPhaseStatus::Rejected,
                summary: message.clone(),
            });
            return rejected_reload_plan(
                request,
                current_generation,
                candidate_generation,
                current_log_level,
                validation,
                vec![RejectedChange::invalid_configuration(message)],
            );
        }
    };

    let carried = CarriedProcessSharedServices::from_active(current.shared_services());
    let next_shared_state =
        match QUICListener::build_shared_state_with_carried(&runtime_config, Some(carried)) {
            Ok(shared_state) => {
                validation.push(PlanningPhaseResult {
                    phase: PlanningPhase::NormalizeRuntime,
                    status: PlanningPhaseStatus::Accepted,
                    summary: "runtime generation assembled successfully".to_string(),
                });
                Arc::new(shared_state)
            }
            Err(err) => {
                let rejected = RejectedChange::resource_preparation_failed(
                    "runtime generation",
                    err.to_string(),
                );
                validation.push(PlanningPhaseResult {
                    phase: PlanningPhase::NormalizeRuntime,
                    status: PlanningPhaseStatus::Rejected,
                    summary: rejected.message.clone(),
                });
                return rejected_reload_plan(
                    request,
                    current_generation,
                    candidate_generation,
                    current_log_level,
                    validation,
                    vec![rejected],
                );
            }
        };

    let next_runtime = RuntimeBundle {
        generation: candidate_generation,
        startup: crate::runtime::generation::StartupOwnedRuntimeState {
            config_path: current.startup().config_path.clone(),
            log_config: config.log.clone(),
        },
        runtime_config,
        shared_state: next_shared_state,
    };
    let next_log_level = next_runtime.startup.log_config.level.clone();
    let candidate_snapshot = Some(snapshot_from_bundle(&next_runtime));
    let diff = build_reload_diff(current.bundle(), &next_runtime);

    let compatibility_rejections: Result<(), Vec<TransitionRejection>> =
        QUICListener::evaluate_runtime_reload_compatibility(current, &next_runtime);
    let (compatibility, rejected_changes, compatibility_summary, candidate_status) =
        match compatibility_rejections {
            Ok(()) => (
                ReloadCompatibilityClassification::LiveReloadable,
                Vec::new(),
                "reload candidate is live-reloadable".to_string(),
                GenerationStatus::Staged,
            ),
            Err(rejections) => {
                let classification = classify_compatibility(&rejections);
                let rendered = render_rejections(rejections.as_slice());
                (
                    classification,
                    rejections.into_iter().map(RejectedChange::from).collect(),
                    rendered,
                    GenerationStatus::Rejected,
                )
            }
        };
    validation.push(PlanningPhaseResult {
        phase: PlanningPhase::EvaluateCompatibility,
        status: if rejected_changes.is_empty() {
            PlanningPhaseStatus::Accepted
        } else {
            PlanningPhaseStatus::Rejected
        },
        summary: compatibility_summary.clone(),
    });

    let rejection_summary = (!rejected_changes.is_empty()).then(|| compatibility_summary.clone());
    let summary = if rejected_changes.is_empty() {
        format!(
            "validated reload candidate for generation {candidate_generation}; ready for activation"
        )
    } else {
        format!("validated reload candidate for generation {candidate_generation}; activation blocked")
    };

    StagedRuntimeReloadPlan {
        plan: ReloadPlan {
            request,
            current_generation,
            candidate_generation,
            candidate_status,
            summary,
            validation,
            compatibility,
            candidate_snapshot,
            rejection_summary,
            diff,
            rejected_changes,
        },
        next_runtime: Some(next_runtime),
        current_log_level,
        next_log_level: Some(next_log_level),
    }
}

fn rejected_reload_plan(
    request: ActivationRequest,
    current_generation: Option<GenerationId>,
    candidate_generation: GenerationId,
    current_log_level: String,
    mut validation: Vec<PlanningPhaseResult>,
    rejected_changes: Vec<RejectedChange>,
) -> StagedRuntimeReloadPlan {
    if validation.iter().all(|step| step.phase != PlanningPhase::NormalizeRuntime) {
        validation.push(PlanningPhaseResult {
            phase: PlanningPhase::NormalizeRuntime,
            status: PlanningPhaseStatus::Skipped,
            summary: "skipped because an earlier validation phase failed".to_string(),
        });
    }
    if validation
        .iter()
        .all(|step| step.phase != PlanningPhase::EvaluateCompatibility)
    {
        validation.push(PlanningPhaseResult {
            phase: PlanningPhase::EvaluateCompatibility,
            status: PlanningPhaseStatus::Skipped,
            summary: "skipped because no valid candidate generation was assembled".to_string(),
        });
    }

    let rejection_summary = Some(render_rejected_changes(&rejected_changes));
    StagedRuntimeReloadPlan {
        plan: ReloadPlan {
            request,
            current_generation,
            candidate_generation,
            candidate_status: GenerationStatus::Rejected,
            summary: format!(
                "reload candidate for generation {candidate_generation} was rejected during staged validation"
            ),
            validation,
            compatibility: ReloadCompatibilityClassification::NotEvaluated,
            candidate_snapshot: None,
            rejection_summary,
            diff: ReloadDiff::default(),
            rejected_changes,
        },
        next_runtime: None,
        current_log_level,
        next_log_level: None,
    }
}

fn render_rejected_changes(rejected_changes: &[RejectedChange]) -> String {
    rejected_changes
        .iter()
        .map(|rejected| rejected.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

fn classify_compatibility(
    rejections: &[TransitionRejection],
) -> ReloadCompatibilityClassification {
    if rejections.is_empty() {
        ReloadCompatibilityClassification::LiveReloadable
    } else if rejections.iter().any(TransitionRejection::requires_restart) {
        ReloadCompatibilityClassification::RestartRequired
    } else {
        ReloadCompatibilityClassification::Rejected
    }
}

fn snapshot_from_bundle(bundle: &RuntimeBundle) -> ProposedGenerationSnapshot {
    let state = bundle.shared_state.generation_state();
    let mut listener_labels = state.listener_runtime_configs.keys().cloned().collect::<Vec<_>>();
    listener_labels.sort_unstable();

    ProposedGenerationSnapshot {
        generation: bundle.generation,
        config_path: bundle.startup.config_path.clone(),
        log_level: bundle.startup.log_config.level.clone(),
        listener_labels,
        upstream_count: state.upstream_policies.len(),
        backend_count: state.backend_endpoints.len(),
    }
}

fn build_reload_diff(current: &RuntimeBundle, next: &RuntimeBundle) -> ReloadDiff {
    let mut entries = Vec::new();

    let current_log_level = current.startup.log_config.level.as_str();
    let next_log_level = next.startup.log_config.level.as_str();
    entries.push(ReloadDiffEntry {
        domain: "log.level".to_string(),
        change: if current_log_level == next_log_level {
            ReloadChangeKind::Unchanged
        } else {
            ReloadChangeKind::Modified
        },
        summary: format!("log.level: {current_log_level} -> {next_log_level}"),
    });

    let current_listeners = sorted_listener_labels(current);
    let next_listeners = sorted_listener_labels(next);
    entries.push(ReloadDiffEntry {
        domain: "listeners".to_string(),
        change: slice_change_kind(&current_listeners, &next_listeners),
        summary: format!(
            "listeners: [{}] -> [{}]",
            current_listeners.join(", "),
            next_listeners.join(", ")
        ),
    });

    let current_upstreams = current.shared_state.generation_state().upstream_policies.len();
    let next_upstreams = next.shared_state.generation_state().upstream_policies.len();
    entries.push(ReloadDiffEntry {
        domain: "upstreams".to_string(),
        change: count_change_kind(current_upstreams, next_upstreams),
        summary: format!("upstreams: {current_upstreams} -> {next_upstreams}"),
    });

    let current_backends = current.shared_state.generation_state().backend_endpoints.len();
    let next_backends = next.shared_state.generation_state().backend_endpoints.len();
    entries.push(ReloadDiffEntry {
        domain: "backends".to_string(),
        change: count_change_kind(current_backends, next_backends),
        summary: format!("backends: {current_backends} -> {next_backends}"),
    });

    ReloadDiff { entries }
}

fn sorted_listener_labels(bundle: &RuntimeBundle) -> Vec<String> {
    let mut labels = bundle
        .shared_state
        .generation_state()
        .listener_runtime_configs
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    labels.sort_unstable();
    labels
}

fn count_change_kind(current: usize, next: usize) -> ReloadChangeKind {
    match next.cmp(&current) {
        std::cmp::Ordering::Less => ReloadChangeKind::Removed,
        std::cmp::Ordering::Equal => ReloadChangeKind::Unchanged,
        std::cmp::Ordering::Greater => ReloadChangeKind::Added,
    }
}

fn slice_change_kind(current: &[String], next: &[String]) -> ReloadChangeKind {
    if current == next {
        ReloadChangeKind::Unchanged
    } else if current.is_empty() && !next.is_empty() {
        ReloadChangeKind::Added
    } else if !current.is_empty() && next.is_empty() {
        ReloadChangeKind::Removed
    } else {
        ReloadChangeKind::Modified
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::policy::TransitionRejection;

    #[test]
    fn rejected_change_preserves_transition_rejection_contract() {
        let rejection = TransitionRejection::restart_required(
            "performance.worker_threads",
            "1",
            "8",
        );
        let rejected = RejectedChange::from(rejection);

        assert_eq!(rejected.kind, RejectedChangeKind::RestartRequired);
        assert_eq!(
            rejected.field_path.as_deref(),
            Some("performance.worker_threads")
        );
        assert_eq!(rejected.operator_action, "restart the process to apply this change");
        assert!(!rejected.active_generation_changed);
        assert!(rejected.message.contains("restart"));
    }

    #[test]
    fn reload_plan_only_activates_without_rejections() {
        let plan = ReloadPlan {
            request: ActivationRequest {
                requested_by: Some("operator".to_string()),
                reason: Some("rotate routes".to_string()),
                expected_generation: Some(7),
                requested_at_ms: 1_720_000_000_000,
            },
            current_generation: Some(7),
            candidate_generation: 8,
            candidate_status: GenerationStatus::Staged,
            summary: "listener policies unchanged".to_string(),
            validation: vec![PlanningPhaseResult {
                phase: PlanningPhase::EvaluateCompatibility,
                status: PlanningPhaseStatus::Accepted,
                summary: "reload candidate is live-reloadable".to_string(),
            }],
            compatibility: ReloadCompatibilityClassification::LiveReloadable,
            candidate_snapshot: Some(ProposedGenerationSnapshot {
                generation: 8,
                config_path: "config.yaml".to_string(),
                log_level: "info".to_string(),
                listener_labels: vec!["edge-primary".to_string()],
                upstream_count: 1,
                backend_count: 2,
            }),
            rejection_summary: None,
            diff: ReloadDiff::default(),
            rejected_changes: Vec::new(),
        };
        assert!(plan.can_activate());

        let mut rejected = plan.clone();
        rejected.rejected_changes.push(RejectedChange {
            kind: RejectedChangeKind::InvalidConfiguration,
            field_path: Some("resilience.watchdog".to_string()),
            current_value: None,
            requested_value: Some("0".to_string()),
            operator_action: "fix the configuration and retry the reload".to_string(),
            active_generation_changed: false,
            message: "invalid watchdog value".to_string(),
        });
        assert!(!rejected.can_activate());
    }

    #[test]
    fn reload_diff_reports_noop_when_no_effective_changes_exist() {
        assert!(ReloadDiff::default().is_noop());

        let diff = ReloadDiff {
            entries: vec![ReloadDiffEntry {
                domain: "observability.metrics".to_string(),
                change: ReloadChangeKind::Unchanged,
                summary: "no effective change".to_string(),
            }],
        };
        assert!(diff.is_noop());
    }
}
