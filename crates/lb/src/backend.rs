use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use impulse_config::config::{Backend, HealthCheck};

use crate::health::{HealthFailureReason, HealthTransition};

#[derive(Clone)]
pub struct BackendState {
    pub address: String,
    pub weight: u32,
    pub health_check: Option<HealthCheck>,
    pub consecutive_failures: u32,
    health_state: HealthState,
    pub active_requests: Arc<AtomicUsize>,
    pub ewma_latency_ms: Option<f64>,
}

impl BackendState {
    pub fn new(backend: &Backend) -> Self {
        Self {
            address: backend.address.clone(),
            weight: backend.weight.max(1),
            health_check: backend.health_check.clone(),
            consecutive_failures: 0,
            health_state: HealthState::Healthy,
            active_requests: Arc::new(AtomicUsize::new(0)),
            ewma_latency_ms: None,
        }
    }

    #[must_use = "this returns a bool, it does not modify the backend state"]
    pub fn is_healthy(&self) -> bool {
        matches!(self.health_state, HealthState::Healthy)
    }

    /// Returns true when an active health-check loop is running for this backend.
    /// When active checks are present, only the health-check loop should drive
    /// consecutive_failures — request-path failures should not contribute.
    pub fn has_active_health_check(&self) -> bool {
        self.health_check.as_ref().is_some_and(|hc| hc.interval > 0)
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn health_check(&self) -> Option<&HealthCheck> {
        self.health_check.as_ref()
    }

    #[must_use]
    pub fn weight(&self) -> u32 {
        self.weight
    }

    pub fn active_requests(&self) -> usize {
        self.active_requests.load(Ordering::Relaxed)
    }

    pub fn ewma_latency_ms(&self) -> Option<f64> {
        self.ewma_latency_ms
    }

    pub fn record_success(&mut self) -> Option<HealthTransition> {
        match &mut self.health_state {
            HealthState::Healthy => {
                self.consecutive_failures = 0;
                None
            }
            HealthState::Unhealthy {
                until, successes, ..
            } => {
                if Instant::now() < *until {
                    return None;
                }

                *successes += 1;
                let success_threshold = self
                    .health_check
                    .as_ref()
                    .map_or(1, |hc| hc.success_threshold);
                if *successes >= success_threshold {
                    self.consecutive_failures = 0;
                    self.health_state = HealthState::Healthy;
                    return Some(HealthTransition::BecameHealthy);
                }
                None
            }
        }
    }

    pub fn record_failure(&mut self, _reason: HealthFailureReason) -> Option<HealthTransition> {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let threshold = self
            .health_check
            .as_ref()
            .map_or(3, |hc| hc.failure_threshold);
        if self.consecutive_failures < threshold {
            return None;
        }

        self.consecutive_failures = 0;
        let cooldown = Duration::from_millis(
            self.health_check
                .as_ref()
                .map_or(10_000, |hc| hc.cooldown_ms),
        );
        self.health_state = HealthState::Unhealthy {
            until: Instant::now() + cooldown,
            successes: 0,
        };
        Some(HealthTransition::BecameUnhealthy)
    }

    /// Cooldown expiry, if this backend is currently unhealthy.
    pub fn cooldown_until(&self) -> Option<Instant> {
        if let HealthState::Unhealthy { until, .. } = self.health_state {
            Some(until)
        } else {
            None
        }
    }

    /// Optimistically re-admit an ejected backend once its cooldown has elapsed
    /// so live traffic can probe it again. Returns true on transition.
    pub fn readmit_if_expired(&mut self, now: Instant) -> bool {
        if let HealthState::Unhealthy { until, .. } = self.health_state
            && now >= until
        {
            self.consecutive_failures = 0;
            self.ewma_latency_ms = None;
            self.health_state = HealthState::Healthy;
            return true;
        }
        false
    }
}

#[derive(Clone)]
enum HealthState {
    Healthy,
    Unhealthy { until: Instant, successes: u32 },
}
