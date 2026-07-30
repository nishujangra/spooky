//! Canonical activation, preview, and rollback contract for runtime generations.
//!
//! This module intentionally contains *only* the typed domain vocabulary for
//! config validation, reload preview, activation, rollback, and history. It does
//! not perform any runtime mutation itself. Control-plane handlers and future
//! activation services should consume these types rather than inventing
//! endpoint-local request/result payloads.

use serde::{Deserialize, Serialize};

use crate::runtime::policy::{TransitionRejection, TransitionRejectionKind};

/// Stable identifier for a published or staged runtime generation.
pub type GenerationId = u64;

/// Milliseconds since the Unix epoch.
///
/// Stored as an integer so the activation contract stays transport-agnostic and
/// trivially serializable for control-plane responses and audit history.
pub type TimestampMillis = u64;

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
    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn is_rollback_candidate(self) -> bool {
        matches!(self, Self::RollbackCandidate | Self::Superseded)
    }
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
    pub diff: ReloadDiff,
    pub rejected_changes: Vec<RejectedChange>,
}

impl ReloadPlan {
    pub fn can_activate(&self) -> bool {
        self.rejected_changes.is_empty() && !matches!(self.candidate_status, GenerationStatus::Rejected)
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
    pub fn succeeded(&self) -> bool {
        self.rolled_back_to.is_some() && self.rejected_changes.is_empty()
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
