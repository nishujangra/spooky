use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use impulse_lb::{HealthTransition, upstream_pool::UpstreamPool};

use crate::runtime::backend::{
    event::{
        BackendHealthObservation, BackendHealthObservationOutcome, BackendHealthObservationSource,
        BackendRequestFeedback, BackendRequestFeedbackOutcome,
    },
    state::BackendIdentity,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveHealthCheckEvaluation {
    pub observation: BackendHealthObservation,
    pub next_consecutive_failures: u32,
    pub next_delay: Duration,
}

pub(crate) fn apply_backend_request_accounting(
    upstream_pool: Option<&Arc<RwLock<UpstreamPool>>>,
    backend_index: Option<usize>,
    elapsed: Duration,
    status: Option<u16>,
) {
    if let (Some(pool), Some(index)) = (upstream_pool, backend_index)
        && let Ok(mut guard) = pool.write()
    {
        guard.finish_request(index, elapsed, status);
    }
}

pub(crate) fn apply_backend_request_feedback(
    upstream_pool: Option<&Arc<RwLock<UpstreamPool>>>,
    backend_index: Option<usize>,
    feedback: &BackendRequestFeedback,
) -> Option<HealthTransition> {
    let (Some(pool), Some(index)) = (upstream_pool, backend_index) else {
        return None;
    };
    let mut pool = pool.write().ok()?;
    match feedback.outcome {
        BackendRequestFeedbackOutcome::Success => pool.mark_backend_request_success(index),
        BackendRequestFeedbackOutcome::Neutral => None,
        BackendRequestFeedbackOutcome::Failure { reason } => {
            pool.observe_backend_request_failure(index, feedback.elapsed, reason)
        }
    }
}

pub(crate) fn evaluate_active_health_check(
    identity: BackendIdentity,
    outcome: BackendHealthObservationOutcome,
    reason: Option<impulse_lb::health::HealthFailureReason>,
    base_interval_ms: u64,
    consecutive_failures: u32,
) -> ActiveHealthCheckEvaluation {
    let next_consecutive_failures = match outcome {
        BackendHealthObservationOutcome::Failure => consecutive_failures.saturating_add(1),
        BackendHealthObservationOutcome::Success | BackendHealthObservationOutcome::Neutral => 0,
    };
    let backoff_multiplier = 1u64 << next_consecutive_failures.min(2);
    let delay_ms = base_interval_ms.saturating_mul(backoff_multiplier);

    ActiveHealthCheckEvaluation {
        observation: BackendHealthObservation::active_check(identity, outcome, reason),
        next_consecutive_failures,
        next_delay: Duration::from_millis(delay_ms),
    }
}

pub(crate) fn apply_backend_health_observation(
    upstream_pool: Option<&Arc<RwLock<UpstreamPool>>>,
    backend_index: Option<usize>,
    observation: &BackendHealthObservation,
) -> Option<HealthTransition> {
    let (Some(pool), Some(index)) = (upstream_pool, backend_index) else {
        return None;
    };
    let mut pool = pool.write().ok()?;
    match (observation.source, observation.outcome) {
        (BackendHealthObservationSource::ActiveCheck, BackendHealthObservationOutcome::Success) => {
            pool.mark_backend_healthy(index)
        }
        (BackendHealthObservationSource::ActiveCheck, BackendHealthObservationOutcome::Failure) => {
            pool.mark_backend_failure_from_active_check(index)
        }
        (BackendHealthObservationSource::ActiveCheck, BackendHealthObservationOutcome::Neutral) => {
            None
        }
        (_, BackendHealthObservationOutcome::Success) => pool.mark_backend_request_success(index),
        (_, BackendHealthObservationOutcome::Neutral) => None,
        (_, BackendHealthObservationOutcome::Failure) => observation
            .reason
            .and_then(|reason| pool.mark_backend_request_failure(index, reason)),
    }
}
