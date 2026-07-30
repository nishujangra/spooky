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
use spooky_errors::ProxyError;

use crate::{
    runtime::{
        bundle::{
            ActiveRuntimeGeneration, RuntimeBundle, RuntimeBundleHandle,
            RuntimeGenerationRecordStatus,
        },
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

/// Operator-visible disposition for a diff domain.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReloadDiffDisposition {
    Reloadable,
    RejectedStartupOwned,
    NoOp,
}

/// One operator-visible entry in a reload preview diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReloadDiffEntry {
    pub domain: String,
    pub change: ReloadChangeKind,
    pub disposition: ReloadDiffDisposition,
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
                .all(|entry| matches!(entry.disposition, ReloadDiffDisposition::NoOp))
    }

    #[must_use]
    pub fn reloadable_entries(&self) -> Vec<&ReloadDiffEntry> {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.disposition, ReloadDiffDisposition::Reloadable))
            .collect()
    }

    #[must_use]
    pub fn rejected_startup_owned_entries(&self) -> Vec<&ReloadDiffEntry> {
        self.entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.disposition,
                    ReloadDiffDisposition::RejectedStartupOwned
                )
            })
            .collect()
    }

    #[must_use]
    pub fn noop_entries(&self) -> Vec<&ReloadDiffEntry> {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.disposition, ReloadDiffDisposition::NoOp))
            .collect()
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
        Self::from(&value)
    }
}

impl From<&TransitionRejection> for RejectedChange {
    fn from(value: &TransitionRejection) -> Self {
        Self {
            kind: value.kind.into(),
            field_path: value.field_path.clone(),
            current_value: value.current_mode.clone(),
            requested_value: value.requested_mode.clone(),
            operator_action: value.operator_action.to_string(),
            active_generation_changed: value.active_runtime_changed,
            message: value.to_string(),
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
}

impl StagedRuntimeReloadPlan {
    #[must_use]
    pub(crate) fn can_activate(&self) -> bool {
        self.plan.can_activate() && self.next_runtime.is_some()
    }
}

/// Canonical runtime activation service.
///
/// This owns the activation transaction for runtime reloads:
/// staged validation/planning, final lifecycle gate, generation swap, and the
/// explicit result object returned to control-plane callers.
pub(crate) struct RuntimeActivationService;

impl RuntimeActivationService {
    pub(crate) fn activate_reload(
        handle: &RuntimeBundleHandle,
        request: ActivationRequest,
        input: ReloadConfigInput,
    ) -> ActivationResult {
        let current = handle.current_view();
        let active_generation = current.generation();
        let current_log_level = current.startup().log_config.level.clone();

        if let Some(expected_generation) = request.expected_generation
            && expected_generation != active_generation
        {
            return rejected_activation_result(
                request,
                active_generation,
                active_generation.saturating_add(1),
                ReloadDiff::default(),
                vec![RejectedChange {
                    kind: RejectedChangeKind::IllegalTransition,
                    field_path: Some("runtime.generation".to_string()),
                    current_value: Some(active_generation.to_string()),
                    requested_value: Some(expected_generation.to_string()),
                    operator_action:
                        "refresh the active generation view and retry the activation".to_string(),
                    active_generation_changed: false,
                    message: format!(
                        "runtime reload rejected: expected active generation {} but current active generation is {}",
                        expected_generation, active_generation
                    ),
                }],
                "activation request targeted a stale runtime generation".to_string(),
            );
        }

        let plan = plan_runtime_reload(&current, request.clone(), input);
        if !plan.can_activate() {
            if plan
                .plan
                .rejected_changes
                .iter()
                .any(|rejection| {
                    matches!(
                        rejection.kind,
                        RejectedChangeKind::ResourcePreparationFailed
                    )
                })
            {
                handle.record_failed_prepare(
                    plan.plan.candidate_generation,
                    plan.plan
                        .rejection_summary
                        .clone()
                        .unwrap_or_else(|| plan.plan.summary.clone()),
                );
            }
            return rejected_activation_result(
                request,
                active_generation,
                plan.plan.candidate_generation,
                plan.plan.diff.clone(),
                plan.plan.rejected_changes.clone(),
                plan.plan
                    .rejection_summary
                    .clone()
                    .unwrap_or_else(|| plan.plan.summary.clone()),
            );
        }

        if let Some(rejection) = handle.lifecycle().check_reload_allowed().rejection() {
            return rejected_activation_result(
                request,
                active_generation,
                plan.plan.candidate_generation,
                plan.plan.diff.clone(),
                vec![RejectedChange::from(rejection)],
                rejection.to_string(),
            );
        }

        match commit_staged_runtime_reload(handle, plan) {
            Ok((generation, diff)) => successful_activation_result(
                request,
                generation,
                diff,
                current_log_level,
                handle.current_view().startup().log_config.level.clone(),
            ),
            Err((candidate_generation, diff, err)) => failed_activation_result(
                request,
                handle.current_generation(),
                candidate_generation,
                diff,
                err,
            ),
        }
    }

    pub(crate) fn rollback_generation(
        handle: &RuntimeBundleHandle,
        request: RollbackRequest,
    ) -> RollbackResult {
        let current = handle.current_view();
        let active_generation = current.generation();
        let target_generation = request.target_generation;

        if let Some(expected_active_generation) = request.expected_active_generation
            && expected_active_generation != active_generation
        {
            return rejected_rollback_result(
                request,
                active_generation,
                None,
                ReloadDiff::default(),
                vec![RejectedChange {
                    kind: RejectedChangeKind::IllegalTransition,
                    field_path: Some("runtime.generation".to_string()),
                    current_value: Some(active_generation.to_string()),
                    requested_value: Some(expected_active_generation.to_string()),
                    operator_action:
                        "refresh the active generation view and retry the rollback".to_string(),
                    active_generation_changed: false,
                    message: format!(
                        "runtime rollback rejected: expected active generation {} but current active generation is {}",
                        expected_active_generation, active_generation
                    ),
                }],
                "rollback request targeted a stale runtime generation".to_string(),
            );
        }

        if request.target_generation == active_generation {
            return rejected_rollback_result(
                request,
                active_generation,
                None,
                ReloadDiff::default(),
                vec![RejectedChange {
                    kind: RejectedChangeKind::IllegalTransition,
                    field_path: Some("runtime.rollback.target_generation".to_string()),
                    current_value: Some(active_generation.to_string()),
                    requested_value: Some(active_generation.to_string()),
                    operator_action:
                        "choose an older retained generation if rollback is still required"
                            .to_string(),
                    active_generation_changed: false,
                    message: format!(
                        "runtime rollback rejected: generation {} is already active",
                        active_generation
                    ),
                }],
                "rollback target is already the active generation".to_string(),
            );
        }

        let Some(target_record) = handle.generation_record(target_generation) else {
            return rejected_rollback_result(
                request,
                active_generation,
                None,
                ReloadDiff::default(),
                vec![RejectedChange {
                    kind: RejectedChangeKind::RuntimeStateUnavailable,
                    field_path: Some("runtime.rollback.target_generation".to_string()),
                    current_value: None,
                    requested_value: Some(target_generation.to_string()),
                    operator_action:
                        "choose a retained known-good generation from runtime history".to_string(),
                    active_generation_changed: false,
                    message: format!(
                        "runtime rollback rejected: generation {} is not retained as a rollback candidate",
                        target_generation
                    ),
                }],
                "rollback target is not retained in runtime history".to_string(),
            );
        };

        if !target_record.status().is_rollback_candidate() || !target_record.has_bundle() {
            return rejected_rollback_result(
                request,
                active_generation,
                Some(target_generation),
                ReloadDiff::default(),
                vec![RejectedChange {
                    kind: RejectedChangeKind::RuntimeStateUnavailable,
                    field_path: Some("runtime.rollback.target_generation".to_string()),
                    current_value: Some(target_record.generation().to_string()),
                    requested_value: Some(target_record.status().as_str().to_string()),
                    operator_action:
                        "choose a complete retained generation with a usable runtime bundle"
                            .to_string(),
                    active_generation_changed: false,
                    message: format!(
                        "runtime rollback rejected: generation {} is incomplete or not a usable rollback candidate",
                        target_generation
                    ),
                }],
                "rollback target is incomplete or unusable".to_string(),
            );
        }

        if let Some(rejection) = handle.lifecycle().check_reload_allowed().rejection() {
            return rejected_rollback_result(
                request,
                active_generation,
                Some(target_generation),
                ReloadDiff::default(),
                vec![RejectedChange::from(rejection)],
                rejection.to_string(),
            );
        }

        let target_bundle = target_record
            .bundle()
            .cloned()
            .expect("rollback candidate record with bundle");
        let candidate_generation = active_generation.saturating_add(1);
        let prepared = match prepare_rollback_bundle(&current, &target_bundle, candidate_generation)
        {
            Ok(prepared) => prepared,
            Err(rejected) => {
                handle.record_failed_prepare(candidate_generation, rejected.message.clone());
                return rejected_rollback_result(
                    request,
                    active_generation,
                    Some(target_generation),
                    ReloadDiff::default(),
                    vec![rejected],
                    "rollback preparation failed".to_string(),
                );
            }
        };

        let compatibility_rejections =
            QUICListener::evaluate_runtime_reload_compatibility(&current, &prepared);
        if let Err(rejections) = compatibility_rejections {
            let rejected_changes = rejections.iter().map(RejectedChange::from).collect::<Vec<_>>();
            let diff = build_reload_diff(
                current.bundle(),
                &prepared,
                rejected_startup_owned_domains(&rejected_changes),
            );
            return rejected_rollback_result(
                request,
                active_generation,
                Some(target_generation),
                diff,
                rejected_changes,
                render_rejections(rejections.as_slice()),
            );
        }

        let diff = build_reload_diff(
            current.bundle(),
            &prepared,
            std::collections::HashSet::new(),
        );
        match commit_runtime_bundle_swap(
            handle,
            prepared,
            RuntimeGenerationRecordStatus::RolledBack,
        ) {
            Ok(generation) => successful_rollback_result(
                request,
                generation,
                target_generation,
                diff,
            ),
            Err(err) => failed_rollback_result(
                request,
                handle.current_generation(),
                target_generation,
                diff,
                err.to_string(),
            ),
        }
    }
}

pub(crate) fn plan_runtime_reload(
    current: &ActiveRuntimeGeneration,
    request: ActivationRequest,
    input: ReloadConfigInput,
) -> StagedRuntimeReloadPlan {
    let current_generation = Some(current.generation());
    let candidate_generation = current.generation().saturating_add(1);
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
    let candidate_snapshot = Some(snapshot_from_bundle(&next_runtime));

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
    let diff = build_reload_diff(
        current.bundle(),
        &next_runtime,
        rejected_startup_owned_domains(&rejected_changes),
    );

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
    }
}

fn commit_staged_runtime_reload(
    handle: &RuntimeBundleHandle,
    plan: StagedRuntimeReloadPlan,
) -> Result<(GenerationId, ReloadDiff), (GenerationId, ReloadDiff, String)> {
    let candidate_generation = plan.plan.candidate_generation;
    let diff = plan.plan.diff.clone();
    let next_runtime = plan.next_runtime.ok_or_else(|| {
        (
            candidate_generation,
            diff.clone(),
            "staged reload plan missing next runtime".to_string(),
        )
    })?;

    commit_runtime_bundle_swap(
        handle,
        next_runtime,
        RuntimeGenerationRecordStatus::Previous,
    )
    .map(|generation| (generation, diff.clone()))
    .map_err(|err| (candidate_generation, diff, err.to_string()))
}

fn commit_runtime_bundle_swap(
    handle: &RuntimeBundleHandle,
    next_runtime: RuntimeBundle,
    previous_status: RuntimeGenerationRecordStatus,
) -> Result<GenerationId, ProxyError> {
    QUICListener::spawn_generation_background_tasks_for_runtime(
        &next_runtime.runtime_config,
        next_runtime.shared_state.as_ref(),
    );
    handle
        .replace_with_archive_status(next_runtime, previous_status)
}

fn prepare_rollback_bundle(
    current: &ActiveRuntimeGeneration,
    target: &RuntimeBundle,
    candidate_generation: GenerationId,
) -> Result<RuntimeBundle, RejectedChange> {
    let carried = CarriedProcessSharedServices::from_active(current.shared_services());
    let next_shared_state =
        QUICListener::build_shared_state_with_carried(&target.runtime_config, Some(carried))
            .map_err(|err| {
                RejectedChange::resource_preparation_failed("runtime rollback", err.to_string())
            })?;

    Ok(RuntimeBundle {
        generation: candidate_generation,
        startup: target.startup.clone(),
        runtime_config: target.runtime_config.clone(),
        shared_state: Arc::new(next_shared_state),
    })
}

fn successful_rollback_result(
    request: RollbackRequest,
    generation: GenerationId,
    target_generation: GenerationId,
    diff: ReloadDiff,
) -> RollbackResult {
    let completed_at_ms = crate::watchdog::time::now_millis();
    let history_entry = GenerationHistoryEntry {
        generation,
        operation: GenerationOperation::Rollback,
        status: GenerationStatus::RolledBack,
        requested_by: request.requested_by.clone(),
        requested_at_ms: request.requested_at_ms,
        completed_at_ms: Some(completed_at_ms),
        summary: format!(
            "rolled back runtime to retained generation {} as new active generation {}",
            target_generation, generation
        ),
        diff,
        rejected_changes: Vec::new(),
    };

    RollbackResult {
        request,
        active_generation: generation,
        rolled_back_to: Some(target_generation),
        status: GenerationStatus::RolledBack,
        rejected_changes: Vec::new(),
        history_entry,
    }
}

fn rejected_rollback_result(
    request: RollbackRequest,
    active_generation: GenerationId,
    target_generation: Option<GenerationId>,
    diff: ReloadDiff,
    rejected_changes: Vec<RejectedChange>,
    summary: String,
) -> RollbackResult {
    let completed_at_ms = crate::watchdog::time::now_millis();
    let history_entry = GenerationHistoryEntry {
        generation: target_generation.unwrap_or(active_generation.saturating_add(1)),
        operation: GenerationOperation::Rollback,
        status: GenerationStatus::Rejected,
        requested_by: request.requested_by.clone(),
        requested_at_ms: request.requested_at_ms,
        completed_at_ms: Some(completed_at_ms),
        summary,
        diff,
        rejected_changes: rejected_changes.clone(),
    };

    RollbackResult {
        request,
        active_generation,
        rolled_back_to: None,
        status: GenerationStatus::Rejected,
        rejected_changes,
        history_entry,
    }
}

fn failed_rollback_result(
    request: RollbackRequest,
    active_generation: GenerationId,
    target_generation: GenerationId,
    diff: ReloadDiff,
    error: String,
) -> RollbackResult {
    let rejected_changes = vec![RejectedChange {
        kind: RejectedChangeKind::RuntimeStateUnavailable,
        field_path: Some("runtime.rollback".to_string()),
        current_value: Some(active_generation.to_string()),
        requested_value: Some(target_generation.to_string()),
        operator_action:
            "inspect runtime state and retry the rollback once the runtime is healthy"
                .to_string(),
        active_generation_changed: false,
        message: error.clone(),
    }];
    let completed_at_ms = crate::watchdog::time::now_millis();
    let history_entry = GenerationHistoryEntry {
        generation: active_generation.saturating_add(1),
        operation: GenerationOperation::Rollback,
        status: GenerationStatus::Failed,
        requested_by: request.requested_by.clone(),
        requested_at_ms: request.requested_at_ms,
        completed_at_ms: Some(completed_at_ms),
        summary: format!(
            "failed to roll back runtime to retained generation {}: {}",
            target_generation, error
        ),
        diff,
        rejected_changes: rejected_changes.clone(),
    };

    RollbackResult {
        request,
        active_generation,
        rolled_back_to: None,
        status: GenerationStatus::Failed,
        rejected_changes,
        history_entry,
    }
}

fn successful_activation_result(
    request: ActivationRequest,
    generation: GenerationId,
    diff: ReloadDiff,
    previous_log_level: String,
    active_log_level: String,
) -> ActivationResult {
    let completed_at_ms = crate::watchdog::time::now_millis();
    let history_entry = GenerationHistoryEntry {
        generation,
        operation: GenerationOperation::Activate,
        status: GenerationStatus::Active,
        requested_by: request.requested_by.clone(),
        requested_at_ms: request.requested_at_ms,
        completed_at_ms: Some(completed_at_ms),
        summary: format!(
            "activated runtime generation {generation} (log.level: {previous_log_level} -> {active_log_level})"
        ),
        diff,
        rejected_changes: Vec::new(),
    };

    ActivationResult {
        request,
        active_generation: generation,
        activated_generation: Some(generation),
        status: GenerationStatus::Active,
        rejected_changes: Vec::new(),
        history_entry,
    }
}

fn rejected_activation_result(
    request: ActivationRequest,
    active_generation: GenerationId,
    candidate_generation: GenerationId,
    diff: ReloadDiff,
    rejected_changes: Vec<RejectedChange>,
    summary: String,
) -> ActivationResult {
    let completed_at_ms = crate::watchdog::time::now_millis();
    let history_entry = GenerationHistoryEntry {
        generation: candidate_generation,
        operation: GenerationOperation::Activate,
        status: GenerationStatus::Rejected,
        requested_by: request.requested_by.clone(),
        requested_at_ms: request.requested_at_ms,
        completed_at_ms: Some(completed_at_ms),
        summary,
        diff,
        rejected_changes: rejected_changes.clone(),
    };

    ActivationResult {
        request,
        active_generation,
        activated_generation: None,
        status: GenerationStatus::Rejected,
        rejected_changes,
        history_entry,
    }
}

fn failed_activation_result(
    request: ActivationRequest,
    active_generation: GenerationId,
    candidate_generation: GenerationId,
    diff: ReloadDiff,
    error: String,
) -> ActivationResult {
    let rejected_changes = vec![RejectedChange {
        kind: RejectedChangeKind::RuntimeStateUnavailable,
        field_path: Some("runtime.activation".to_string()),
        current_value: Some(active_generation.to_string()),
        requested_value: Some(candidate_generation.to_string()),
        operator_action:
            "inspect runtime state and retry the activation once the runtime is healthy"
                .to_string(),
        active_generation_changed: false,
        message: error.clone(),
    }];
    let completed_at_ms = crate::watchdog::time::now_millis();
    let history_entry = GenerationHistoryEntry {
        generation: candidate_generation,
        operation: GenerationOperation::Activate,
        status: GenerationStatus::Failed,
        requested_by: request.requested_by.clone(),
        requested_at_ms: request.requested_at_ms,
        completed_at_ms: Some(completed_at_ms),
        summary: format!("failed to activate runtime generation {candidate_generation}: {error}"),
        diff,
        rejected_changes: rejected_changes.clone(),
    };

    ActivationResult {
        request,
        active_generation,
        activated_generation: None,
        status: GenerationStatus::Failed,
        rejected_changes,
        history_entry,
    }
}

fn rejected_reload_plan(
    request: ActivationRequest,
    current_generation: Option<GenerationId>,
    candidate_generation: GenerationId,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ReloadDiffDomain {
    Listeners,
    RoutesUpstreams,
    BackendPolicies,
    AuthAdmissionResilience,
    ObservabilityControlPlane,
}

impl ReloadDiffDomain {
    fn as_str(self) -> &'static str {
        match self {
            Self::Listeners => "listeners",
            Self::RoutesUpstreams => "routes_upstreams",
            Self::BackendPolicies => "backend_policies",
            Self::AuthAdmissionResilience => "auth_admission_resilience",
            Self::ObservabilityControlPlane => "observability_control_plane",
        }
    }
}

fn build_reload_diff(
    current: &RuntimeBundle,
    next: &RuntimeBundle,
    rejected_domains: std::collections::HashSet<ReloadDiffDomain>,
) -> ReloadDiff {
    let specs = [
        (
            ReloadDiffDomain::Listeners,
            summarize_listeners(current),
            summarize_listeners(next),
        ),
        (
            ReloadDiffDomain::RoutesUpstreams,
            summarize_routes_upstreams(current),
            summarize_routes_upstreams(next),
        ),
        (
            ReloadDiffDomain::BackendPolicies,
            summarize_backend_policies(current),
            summarize_backend_policies(next),
        ),
        (
            ReloadDiffDomain::AuthAdmissionResilience,
            summarize_auth_admission_resilience(current),
            summarize_auth_admission_resilience(next),
        ),
        (
            ReloadDiffDomain::ObservabilityControlPlane,
            summarize_observability_control_plane(current),
            summarize_observability_control_plane(next),
        ),
    ];

    let entries = specs
        .into_iter()
        .map(|(domain, current_summary, next_summary)| {
            let change = text_change_kind(&current_summary, &next_summary);
            let disposition = if matches!(change, ReloadChangeKind::Unchanged) {
                ReloadDiffDisposition::NoOp
            } else if rejected_domains.contains(&domain) {
                ReloadDiffDisposition::RejectedStartupOwned
            } else {
                ReloadDiffDisposition::Reloadable
            };

            ReloadDiffEntry {
                domain: domain.as_str().to_string(),
                change,
                disposition,
                summary: format!(
                    "{}: [{}] -> [{}]",
                    domain.as_str(),
                    current_summary,
                    next_summary
                ),
            }
        })
        .collect();

    ReloadDiff { entries }
}

fn summarize_listeners(bundle: &RuntimeBundle) -> String {
    let mut listeners = bundle
        .runtime_config
        .listeners
        .iter()
        .map(|listener| {
            format!(
                "{}:{:?}:{}:{}:{}:client_auth(enabled={},required={})",
                listener.index,
                listener.source,
                listener.listen.protocol,
                listener.listen.address,
                listener.listen.port,
                listener.tls.client_auth.enabled,
                listener.tls.client_auth.require_client_cert,
            )
        })
        .collect::<Vec<_>>();
    listeners.sort_unstable();
    listeners.join(" | ")
}

fn summarize_routes_upstreams(bundle: &RuntimeBundle) -> String {
    let mut upstreams = bundle
        .runtime_config
        .upstreams
        .iter()
        .map(|(name, upstream)| {
            format!(
                "{}:{}:{:?}:{:?}:{:?}:{}:{:?}",
                name,
                upstream.load_balancing.strategy.canonical_name(),
                upstream.route.host,
                upstream.route.path_prefix,
                upstream.route.method,
                upstream.policy.protocol.0.allow_connect,
                upstream.load_balancing.key_spec
            )
        })
        .collect::<Vec<_>>();
    upstreams.sort_unstable();
    upstreams.join(" | ")
}

fn summarize_backend_policies(bundle: &RuntimeBundle) -> String {
    let mut upstreams = bundle
        .runtime_config
        .upstreams
        .iter()
        .map(|(name, upstream)| {
            let backend_ids = upstream
                .backends
                .iter()
                .map(|backend| {
                    format!(
                        "{}:{:?}:{:?}:hc={}",
                        backend.backend.id,
                        backend.endpoint.transport_kind,
                        backend.endpoint.address_kind,
                        backend.health_check.is_some()
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{}:tls(v={},sni={}):dns(enabled={},refresh={}s):{}",
                name,
                upstream.backend_tls_policy().verify_certificates,
                upstream.backend_tls_policy().strict_sni,
                bundle.runtime_config.performance.backend_dns_refresh_enabled,
                bundle
                    .runtime_config
                    .performance
                    .backend_dns_refresh_interval_ms
                    / 1000,
                backend_ids
            )
        })
        .collect::<Vec<_>>();
    upstreams.sort_unstable();
    upstreams.join(" | ")
}

fn summarize_auth_admission_resilience(bundle: &RuntimeBundle) -> String {
    let mut auth = bundle
        .runtime_config
        .upstreams
        .iter()
        .map(|(name, upstream)| {
            format!(
                "{}:api_key={}:jwt={}:external={}:scopes={}:roles={}",
                name,
                upstream.policy.upstream_auth.api_key.is_some(),
                upstream.policy.upstream_auth.jwt.is_some(),
                upstream.policy.upstream_auth.external_auth.is_some(),
                upstream.policy.upstream_auth.required_scopes.join(","),
                upstream.policy.upstream_auth.required_roles.join(","),
            )
        })
        .collect::<Vec<_>>();
    auth.sort_unstable();

    let admission = &bundle.runtime_config.policies.admission;
    let rate_limits = &bundle.runtime_config.policies.rate_limits;
    let resilience = format!(
        "adaptive={}..{:?};route_queue={}..{};circuit={}#{};hedging={}@{:?};retry={}@{};brownout={}%;watchdog={};scoped_rate_limits={}",
        admission.adaptive_admission.min_limit,
        admission.adaptive_admission.max_limit,
        admission.route_queue.default_cap,
        admission.route_queue.global_cap,
        admission.circuit_breaker.enabled,
        admission.circuit_breaker.failure_threshold,
        admission.hedging.enabled,
        admission.hedging.delay,
        admission.retry_budget.enabled,
        admission.retry_budget.ratio_percent,
        admission.brownout.trigger_inflight_percent,
        admission.watchdog.enabled,
        rate_limits.scoped_limits.len(),
    );

    format!("auth=[{}]; policies=[{}]", auth.join(" | "), resilience)
}

fn summarize_observability_control_plane(bundle: &RuntimeBundle) -> String {
    let startup = bundle.startup();
    let observability = &bundle.runtime_config.observability;
    let performance = &bundle.runtime_config.performance;
    format!(
        "log(level={},format={:?},file_enabled={},file_path={});control_api(enabled={},bind={}:{},path={});metrics(enabled={},bind={}:{},path={});tracing(enabled={},service={},otlp={:?},ratio={});control_plane_threads={}",
        startup.log_config.level,
        startup.log_config.format,
        startup.log_config.file.enabled,
        startup.log_config.file.path,
        observability.control_api.enabled,
        observability.control_api.address,
        observability.control_api.port,
        observability.control_api.runtime_path,
        observability.metrics.enabled,
        observability.metrics.address,
        observability.metrics.port,
        observability.metrics.path,
        observability.tracing.enabled,
        observability.tracing.service_name,
        observability.tracing.otlp_endpoint,
        observability.tracing.sample_ratio,
        performance.control_plane_threads,
    )
}

fn text_change_kind(current: &str, next: &str) -> ReloadChangeKind {
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

fn rejected_startup_owned_domains(
    rejected_changes: &[RejectedChange],
) -> std::collections::HashSet<ReloadDiffDomain> {
    rejected_changes
        .iter()
        .filter(|rejection| rejection.kind == RejectedChangeKind::RestartRequired)
        .filter_map(|rejection| rejection.field_path.as_deref())
        .filter_map(|field_path| {
            if field_path.starts_with("log.")
                || field_path.starts_with("observability.tracing.")
                || field_path == "performance.control_plane_threads"
            {
                Some(ReloadDiffDomain::ObservabilityControlPlane)
            } else {
                None
            }
        })
        .collect()
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
                disposition: ReloadDiffDisposition::NoOp,
                summary: "no effective change".to_string(),
            }],
        };
        assert!(diff.is_noop());
    }
}
