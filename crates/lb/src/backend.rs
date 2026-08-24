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

    #[must_use = "this returns a bool, it does not modify the backend state"]
    pub fn is_probing(&self) -> bool {
        matches!(self.health_state, HealthState::Probing { .. })
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
        let success_threshold = self
            .health_check
            .as_ref()
            .map_or(1, |hc| hc.success_threshold);

        match self.health_state.clone() {
            HealthState::Healthy => {
                self.consecutive_failures = 0;
                None
            }
            HealthState::Unhealthy { until, .. } => {
                if Instant::now() < until {
                    return None;
                }

                self.consecutive_failures = 0;
                if success_threshold <= 1 {
                    self.consecutive_failures = 0;
                    self.health_state = HealthState::Healthy;
                    return Some(HealthTransition::BecameHealthy);
                }

                self.health_state = HealthState::Probing { successes: 1 };
                None
            }
            HealthState::Probing { successes } => {
                let next_successes = successes.saturating_add(1);
                if next_successes >= success_threshold {
                    self.consecutive_failures = 0;
                    self.health_state = HealthState::Healthy;
                    return Some(HealthTransition::BecameHealthy);
                }

                self.health_state = HealthState::Probing {
                    successes: next_successes,
                };
                None
            }
        }
    }

    pub fn record_failure(&mut self, _reason: HealthFailureReason) -> Option<HealthTransition> {
        if matches!(self.health_state, HealthState::Probing { .. }) {
            self.consecutive_failures = 0;
            self.health_state = HealthState::Unhealthy {
                until: Instant::now() + self.cooldown_duration(),
                successes: 0,
            };
            return None;
        }

        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let threshold = self
            .health_check
            .as_ref()
            .map_or(3, |hc| hc.failure_threshold);
        if self.consecutive_failures < threshold {
            return None;
        }

        self.consecutive_failures = 0;
        self.health_state = HealthState::Unhealthy {
            until: Instant::now() + self.cooldown_duration(),
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

    fn cooldown_duration(&self) -> Duration {
        Duration::from_millis(self.health_check.as_ref().map_or(10_000, |hc| hc.cooldown_ms))
    }
}

#[derive(Clone)]
enum HealthState {
    Healthy,
    // Half-open recovery state: a backend has served enough cooldown time to
    // be probe-eligible, but it is not healthy again until probe successes
    // reach the configured threshold.
    Probing { successes: u32 },
    Unhealthy { until: Instant, successes: u32 },
}
