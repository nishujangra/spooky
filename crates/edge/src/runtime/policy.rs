//! Operational policy vocabulary for runtime ownership and reload rules.
//!
//! This module is the **canonical, handler-free description** of the operational
//! contract frozen in Phase 0 (`machine-hardening-phase0.md`). It defines the
//! stable types that later phases build on:
//!
//! - who *owns* a runtime resource ([`OperationalOwnership`]),
//! - whether a config domain can change at runtime ([`ReloadCapability`]),
//! - what kind of runtime transition is being attempted ([`RuntimeTransitionKind`]),
//! - why a transition was rejected ([`TransitionRejectionKind`], [`TransitionRejection`]),
//! - the typed outcome of evaluating a transition ([`RuntimeTransitionDecision`]).
//!
//! It also carries a single source-of-truth table ([`RESOURCE_DOMAINS`]) mapping
//! every config/runtime domain to its ownership class and reload capability, so a
//! reader can answer "what can reload live?" and "who owns this?" from this file
//! alone — the Phase 1 exit criterion.
//!
//! # Phase 1 scope
//!
//! This layer introduces **no semantic change**. Existing validators keep their
//! current behavior; they are given a way to *describe* their outcomes with these
//! types (see [`TransitionRejection::restart_required`]). Wiring the reload flow to
//! *decide* from this table is Phase 2 work and is intentionally not done here.

use std::fmt;

/// Which lifecycle phase owns a runtime resource, i.e. who is allowed to create
/// or replace it and when.
///
/// Derived from the Phase 0 invariants table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationalOwnership {
    /// Constructed once at process startup and never replaced while the process
    /// lives. Changing it requires a full restart (e.g. worker topology,
    /// listener bind, log/tracing sinks).
    StartupOwned,
    /// Rebuilt fresh for each runtime generation and swapped atomically on reload
    /// (e.g. routing index, upstream pools, backend resolution store).
    GenerationOwned,
    /// A single instance shared across all generations for the life of the
    /// process (e.g. the watchdog coordinator, the runtime-bundle handle itself).
    ProcessShared,
}

/// Whether a config domain can change while the process is running, and if so
/// how the change takes effect.
///
/// Derived from the Phase 0 reload-rejection inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReloadCapability {
    /// Change takes effect on the next generation (or immediately, as with
    /// `log.level`) without a restart or a rejection.
    LiveReloadable,
    /// Change is understood but cannot be applied to the running process; the
    /// reload is rejected with a restart-required message and the active
    /// generation is preserved.
    RestartRequired,
    /// The value is fixed for the life of the process and must never differ from
    /// the value the process booted with. Distinct from [`Self::RestartRequired`]
    /// only in intent: these are never expected to change across a restart of the
    /// same deployment (e.g. an identity established at boot).
    ImmutableAtRuntime,
}

/// The kind of runtime transition being attempted. Later phases represent each
/// transition with typed input/output; Phase 1 only names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeTransitionKind {
    /// Process boot to first active generation.
    Startup,
    /// A reload candidate is being validated but not yet committed.
    ReloadValidate,
    /// A validated reload candidate is being installed as the active generation.
    ReloadCommit,
    /// A worker or generation has begun draining in-flight work.
    DrainStart,
    /// Draining has completed (or its deadline forced completion).
    DrainFinish,
    /// Process shutdown has been requested.
    ShutdownStart,
    /// Process shutdown has completed.
    ShutdownFinish,
}

/// Why a runtime transition was rejected. This is the typed replacement for the
/// free-form rejection strings enumerated in Phase 0 §8; Phase 1 introduces it,
/// Phase 2 routes decisions through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransitionRejectionKind {
    /// A startup-owned or immutable field changed; the operator must restart.
    RestartRequired,
    /// A live-reloadable resource could not be prepared (e.g. a new listener
    /// failed its preflight bind). The active generation is preserved.
    ResourcePreparationFailed,
    /// The incoming configuration failed validation or normalization.
    InvalidConfiguration,
    /// The transition is illegal from the current state (e.g. reload while
    /// shutting down). Reserved for Phase 6.
    IllegalTransition,
    /// Required runtime state was unavailable to evaluate the transition (e.g. no
    /// active generation yet).
    RuntimeStateUnavailable,
}

impl TransitionRejectionKind {
    /// Whether this rejection means the operator must restart the process rather
    /// than retry the reload.
    pub fn requires_restart(self) -> bool {
        matches!(self, Self::RestartRequired)
    }

    /// A short, stable machine-readable slug for this rejection kind, suitable for
    /// metric labels and structured logs.
    pub fn slug(self) -> &'static str {
        match self {
            Self::RestartRequired => "restart_required",
            Self::ResourcePreparationFailed => "resource_preparation_failed",
            Self::InvalidConfiguration => "invalid_configuration",
            Self::IllegalTransition => "illegal_transition",
            Self::RuntimeStateUnavailable => "runtime_state_unavailable",
        }
    }
}

/// A typed, operator-facing rejection payload.
///
/// It answers the Phase 1 step-4 questions: *what changed*, *why it is rejected*,
/// and *what action the operator should take*. It is deliberately owned (`String`)
/// so it can be constructed from borrowed config values at any call site and
/// carried across the reload boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionRejection {
    /// Category of rejection.
    pub kind: TransitionRejectionKind,
    /// Dotted config field / subsystem path involved, when known
    /// (e.g. `"performance.control_plane_threads"`).
    pub field_path: Option<String>,
    /// The currently-active value, rendered for the operator, when known.
    pub current_mode: Option<String>,
    /// The requested value, rendered for the operator, when known.
    pub requested_mode: Option<String>,
    /// A fixed, human-readable instruction telling the operator what to do next.
    pub operator_action: &'static str,
}

impl TransitionRejection {
    /// Build a `RestartRequired` rejection for a field that changed at runtime.
    ///
    /// This mirrors the existing `note_restart_required_change` behavior in
    /// `control_api/reload.rs` so validators can emit the *same* decision as a
    /// typed value without changing what they reject.
    pub fn restart_required(
        field_path: impl Into<String>,
        current_mode: impl fmt::Debug,
        requested_mode: impl fmt::Debug,
    ) -> Self {
        Self {
            kind: TransitionRejectionKind::RestartRequired,
            field_path: Some(field_path.into()),
            current_mode: Some(format!("{current_mode:?}")),
            requested_mode: Some(format!("{requested_mode:?}")),
            operator_action: "restart the process to apply this change",
        }
    }

    /// Build a `ResourcePreparationFailed` rejection (e.g. a preflight bind
    /// failed). The active generation is unaffected.
    pub fn resource_preparation_failed(field_path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            kind: TransitionRejectionKind::ResourcePreparationFailed,
            field_path: Some(field_path.into()),
            current_mode: None,
            requested_mode: Some(reason.into()),
            operator_action: "fix the reported resource conflict and retry the reload",
        }
    }

    /// Build an `InvalidConfiguration` rejection carrying the underlying message.
    pub fn invalid_configuration(reason: impl Into<String>) -> Self {
        Self {
            kind: TransitionRejectionKind::InvalidConfiguration,
            field_path: None,
            current_mode: None,
            requested_mode: Some(reason.into()),
            operator_action: "fix the configuration and retry the reload",
        }
    }

    /// Build a `RuntimeStateUnavailable` rejection.
    pub fn runtime_state_unavailable(reason: impl Into<String>) -> Self {
        Self {
            kind: TransitionRejectionKind::RuntimeStateUnavailable,
            field_path: None,
            current_mode: None,
            requested_mode: Some(reason.into()),
            operator_action: "inspect process state; the reload could not be evaluated",
        }
    }

    /// Whether the operator must restart to apply the rejected change.
    pub fn requires_restart(&self) -> bool {
        self.kind.requires_restart()
    }
}

impl fmt::Display for TransitionRejection {
    /// Renders a single-line operator message. The wording is intentionally close
    /// to the current free-form strings so that migrating a call site does not
    /// change what an operator reads.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.field_path, &self.current_mode, &self.requested_mode) {
            (Some(field), Some(current), Some(requested))
                if self.kind == TransitionRejectionKind::RestartRequired =>
            {
                write!(
                    f,
                    "runtime reload rejected: {field} changed from {current} to {requested}; restart required"
                )
            }
            (Some(field), _, Some(detail)) => {
                write!(f, "runtime reload rejected: {field}: {detail}")
            }
            (_, _, Some(detail)) => write!(f, "runtime reload rejected: {detail}"),
            _ => write!(f, "runtime reload rejected: {}", self.kind.slug()),
        }
    }
}

/// A plan describing an accepted transition. Phase 1 keeps this a thin,
/// zero-cost marker so [`RuntimeTransitionDecision`] can be exhaustive; Phase 2/6
/// give it real content (the prepared next generation, resources to move, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionPlan {
    /// The kind of transition this plan authorizes.
    pub kind: RuntimeTransitionKind,
}

/// The typed outcome of evaluating a runtime transition attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTransitionDecision {
    /// The transition may proceed.
    Allowed(TransitionPlan),
    /// The transition is rejected; the active generation is preserved.
    Rejected(TransitionRejection),
}

impl RuntimeTransitionDecision {
    /// Whether the transition was allowed.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed(_))
    }

    /// Borrow the rejection, if this decision rejected the transition.
    pub fn rejection(&self) -> Option<&TransitionRejection> {
        match self {
            Self::Rejected(rejection) => Some(rejection),
            Self::Allowed(_) => None,
        }
    }
}

/// A single runtime-managed resource or config domain, described by its ownership
/// class and reload capability. One row per operational domain identified in
/// Phase 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceDomain {
    /// Dotted config path or subsystem name this domain covers.
    pub field_path: &'static str,
    /// A one-line human description of the domain.
    pub description: &'static str,
    /// Which lifecycle phase owns the underlying resource.
    pub ownership: OperationalOwnership,
    /// Whether the domain can change at runtime.
    pub reload: ReloadCapability,
}

impl ResourceDomain {
    /// Whether a change to this domain takes effect without a restart.
    pub fn is_live_reloadable(&self) -> bool {
        matches!(self.reload, ReloadCapability::LiveReloadable)
    }

    /// Whether a change to this domain forces a restart (or is fixed for the
    /// life of the process).
    pub fn requires_restart(&self) -> bool {
        matches!(
            self.reload,
            ReloadCapability::RestartRequired | ReloadCapability::ImmutableAtRuntime
        )
    }

    /// Whether the underlying resource is created once at startup and never
    /// replaced while the process runs.
    pub fn is_startup_owned_only(&self) -> bool {
        matches!(self.ownership, OperationalOwnership::StartupOwned)
    }

    /// Whether the underlying resource is rebuilt per generation.
    pub fn is_generation_owned(&self) -> bool {
        matches!(self.ownership, OperationalOwnership::GenerationOwned)
    }
}

/// The single source of truth mapping config/runtime domains to their ownership
/// class and reload capability. Phase 2 will make the reload validators *decide*
/// from this table; Phase 1 only records it so the rules are readable in one
/// place.
///
/// Each restart-required entry corresponds to a `note_restart_required_change`
/// call or a bind-compatibility check in `control_api/reload.rs`.
pub static RESOURCE_DOMAINS: &[ResourceDomain] = &[
    // -- listener bind and protocol settings ------------------------------------
    ResourceDomain {
        field_path: "listeners[].listen",
        description: "UDP listener bind address/port and reuseport sockets",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    // -- control API settings ---------------------------------------------------
    ResourceDomain {
        field_path: "observability.control_api.address/port",
        description: "control API endpoint bind (preflighted, new bind only)",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    // -- metrics / observability settings ---------------------------------------
    ResourceDomain {
        field_path: "observability.metrics.address/port",
        description: "metrics exporter bind (preflighted, new bind only)",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    ResourceDomain {
        field_path: "observability.tracing.enabled",
        description: "tracing pipeline enablement",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    ResourceDomain {
        field_path: "observability.tracing.service_name",
        description: "tracing service name",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    ResourceDomain {
        field_path: "observability.tracing.otlp_endpoint",
        description: "tracing OTLP exporter endpoint",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    ResourceDomain {
        field_path: "observability.tracing.sample_ratio",
        description: "tracing sample ratio",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    // -- logging sinks ----------------------------------------------------------
    ResourceDomain {
        field_path: "log.file.enabled",
        description: "file log sink enablement",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    ResourceDomain {
        field_path: "log.file.path",
        description: "file log sink path",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    ResourceDomain {
        field_path: "log.format",
        description: "log output format",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    ResourceDomain {
        field_path: "log.level",
        description: "log verbosity (applied live post-commit)",
        ownership: OperationalOwnership::ProcessShared,
        reload: ReloadCapability::LiveReloadable,
    },
    // -- worker topology / runtime threading ------------------------------------
    ResourceDomain {
        field_path: "performance.control_plane_threads",
        description: "control-plane Tokio runtime thread count",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    ResourceDomain {
        field_path: "performance.worker_threads",
        description: "data-plane worker thread count / listener topology",
        ownership: OperationalOwnership::StartupOwned,
        reload: ReloadCapability::RestartRequired,
    },
    // -- route table / upstream definitions -------------------------------------
    ResourceDomain {
        field_path: "routes",
        description: "routing index (rebuilt per generation)",
        ownership: OperationalOwnership::GenerationOwned,
        reload: ReloadCapability::LiveReloadable,
    },
    ResourceDomain {
        field_path: "upstreams",
        description: "upstream pools, inflight semaphores, transport pool",
        ownership: OperationalOwnership::GenerationOwned,
        reload: ReloadCapability::LiveReloadable,
    },
    // -- backend health and load-balancing policy -------------------------------
    ResourceDomain {
        field_path: "upstreams[].backends",
        description: "backend resolution store and DNS refresh (per generation)",
        ownership: OperationalOwnership::GenerationOwned,
        reload: ReloadCapability::LiveReloadable,
    },
    // -- watchdog ownership and restart semantics -------------------------------
    ResourceDomain {
        field_path: "watchdog",
        description: "watchdog coordinator (shared) + generation-scoped service task",
        ownership: OperationalOwnership::ProcessShared,
        reload: ReloadCapability::LiveReloadable,
    },
];

/// Look up the policy row for a dotted config field path, if one is recorded.
pub fn resource_domain(field_path: &str) -> Option<&'static ResourceDomain> {
    RESOURCE_DOMAINS.iter().find(|d| d.field_path == field_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_required_rejection_matches_legacy_wording() {
        // Legacy string: "runtime reload rejected: {field} changed from {current:?} to {next:?}; restart required"
        let rejection = TransitionRejection::restart_required(
            "performance.control_plane_threads",
            2usize,
            4usize,
        );
        assert!(rejection.requires_restart());
        assert_eq!(
            rejection.to_string(),
            "runtime reload rejected: performance.control_plane_threads changed from 2 to 4; restart required"
        );
    }

    #[test]
    fn decision_helpers() {
        let allowed = RuntimeTransitionDecision::Allowed(TransitionPlan {
            kind: RuntimeTransitionKind::ReloadCommit,
        });
        assert!(allowed.is_allowed());
        assert!(allowed.rejection().is_none());

        let rejected = RuntimeTransitionDecision::Rejected(
            TransitionRejection::invalid_configuration("bad yaml"),
        );
        assert!(!rejected.is_allowed());
        assert_eq!(
            rejected.rejection().map(|r| r.kind),
            Some(TransitionRejectionKind::InvalidConfiguration)
        );
    }

    #[test]
    fn domain_helpers_agree_with_table() {
        let threads = resource_domain("performance.control_plane_threads").unwrap();
        assert!(threads.is_startup_owned_only());
        assert!(threads.requires_restart());
        assert!(!threads.is_live_reloadable());

        let routes = resource_domain("routes").unwrap();
        assert!(routes.is_generation_owned());
        assert!(routes.is_live_reloadable());
        assert!(!routes.requires_restart());
    }

    #[test]
    fn every_domain_path_is_unique() {
        let mut paths: Vec<_> = RESOURCE_DOMAINS.iter().map(|d| d.field_path).collect();
        paths.sort_unstable();
        let before = paths.len();
        paths.dedup();
        assert_eq!(before, paths.len(), "duplicate field_path in RESOURCE_DOMAINS");
    }
}
