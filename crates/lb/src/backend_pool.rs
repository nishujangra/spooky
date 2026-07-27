use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use spooky_config::config::HealthCheck;

use crate::{
    backend::BackendState,
    health::{HealthFailureReason, HealthTransition},
};

pub struct BackendPool {
    pub backends: Vec<BackendState>,
    pub healthy: Vec<usize>,
    pub healthy_pos: Vec<Option<usize>>,
    pub membership_epoch: u64,
    // Earliest cooldown expiry among passively-ejected backends (no active
    // health check), driving time-based re-admission. `None` when none pending.
    pub earliest_readmit: Option<Instant>,
}

impl BackendPool {
    pub fn new_from_states(backends: Vec<BackendState>) -> Self {
        let mut healthy = Vec::with_capacity(backends.len());
        let mut healthy_pos = vec![None; backends.len()];

        for (idx, backend) in backends.iter().enumerate() {
            if backend.is_healthy() {
                healthy_pos[idx] = Some(healthy.len());
                healthy.push(idx);
            }
        }

        Self {
            backends,
            healthy,
            healthy_pos,
            membership_epoch: 0,
            earliest_readmit: None,
        }
    }

    pub fn len(&self) -> usize {
        self.backends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }

    pub fn address(&self, index: usize) -> Option<&str> {
        self.backends.get(index).map(|b| b.address())
    }

    pub fn mark_success(&mut self, index: usize) -> Option<HealthTransition> {
        if index >= self.backends.len() {
            return None;
        }

        let (was_healthy, is_healthy, transition) = {
            let backend = &mut self.backends[index];
            let was_healthy = backend.is_healthy();
            let transition = backend.record_success();
            let is_healthy = backend.is_healthy();
            (was_healthy, is_healthy, transition)
        };

        if was_healthy != is_healthy {
            if is_healthy {
                debug_assert!(self.mark_healthy(index));
            } else {
                debug_assert!(self.mark_unhealthy(index));
            }
            self.membership_epoch = self.membership_epoch.wrapping_add(1);
        }

        transition
    }

    /// Mark a failure from the active health-check loop — always recorded.
    pub fn mark_failure(&mut self, index: usize) -> Option<HealthTransition> {
        self.mark_failure_with_reason(index, HealthFailureReason::HttpStatus5xx)
    }

    /// Mark a failure from the request path (passive).
    /// Skipped when an active health-check loop is running for this backend,
    /// because the loop is the sole authority on consecutive_failures in that case.
    pub fn mark_request_failure(
        &mut self,
        index: usize,
        reason: HealthFailureReason,
    ) -> Option<HealthTransition> {
        if index < self.backends.len() && self.backends[index].has_active_health_check() {
            return None;
        }
        self.mark_failure_with_reason(index, reason)
    }

    pub fn mark_failure_with_reason(
        &mut self,
        index: usize,
        reason: HealthFailureReason,
    ) -> Option<HealthTransition> {
        if index >= self.backends.len() {
            return None;
        }

        let (was_healthy, is_healthy, transition) = {
            let backend = &mut self.backends[index];
            let was_healthy = backend.is_healthy();
            let transition = backend.record_failure(reason);
            let is_healthy = backend.is_healthy();
            (was_healthy, is_healthy, transition)
        };

        if was_healthy != is_healthy {
            if is_healthy {
                debug_assert!(self.mark_healthy(index));
            } else {
                debug_assert!(self.mark_unhealthy(index));
                // Passive ejections have no active loop to recover them; record
                // the cooldown so `reconcile_readmit` can re-admit on expiry.
                if !self.backends[index].has_active_health_check()
                    && let Some(until) = self.backends[index].cooldown_until()
                {
                    self.earliest_readmit =
                        Some(self.earliest_readmit.map_or(until, |e| e.min(until)));
                }
            }
            self.membership_epoch = self.membership_epoch.wrapping_add(1);
        }

        transition
    }

    /// True when any backend is passively ejected and pending re-admission.
    /// Clock-free so the read-locked hot path pays only a branch (no syscall):
    /// while something is pending, callers take the write-locked slow path where
    /// `reconcile_readmit` checks the actual cooldown clock.
    pub fn readmit_due(&self) -> bool {
        self.earliest_readmit.is_some()
    }

    /// Re-admit passively-ejected backends whose cooldown has elapsed so live
    /// traffic can probe them. Reads the clock only when a re-admission is
    /// actually pending, keeping the healthy pick path syscall-free.
    pub fn reconcile_readmit(&mut self) {
        if self.earliest_readmit.is_some() {
            self.reconcile_readmit_at(Instant::now());
        }
    }

    /// Core of [`reconcile_readmit`] with an injectable clock. Recomputes the
    /// next pending expiry; early-returns while the soonest cooldown is unmet.
    pub fn reconcile_readmit_at(&mut self, now: Instant) {
        let Some(earliest) = self.earliest_readmit else {
            return;
        };
        if now < earliest {
            return;
        }
        let mut next: Option<Instant> = None;
        for index in 0..self.backends.len() {
            if self.backends[index].has_active_health_check() {
                continue;
            }
            if self.backends[index].readmit_if_expired(now) {
                debug_assert!(self.mark_healthy(index));
                self.membership_epoch = self.membership_epoch.wrapping_add(1);
            } else if let Some(until) = self.backends[index].cooldown_until() {
                next = Some(next.map_or(until, |e| e.min(until)));
            }
        }
        self.earliest_readmit = next;
    }

    pub fn health_check(&self, index: usize) -> Option<HealthCheck> {
        self.backends
            .get(index)
            .and_then(|b| b.health_check().cloned())
    }

    pub fn healthy_indices(&self) -> Vec<usize> {
        self.healthy.clone()
    }

    pub fn healthy_len(&self) -> usize {
        self.healthy.len()
    }

    pub fn healthy_indices_iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.healthy.iter().copied()
    }

    pub fn all_indices(&self) -> Vec<usize> {
        (0..self.backends.len()).collect()
    }

    pub fn backend(&self, index: usize) -> Option<&BackendState> {
        self.backends.get(index)
    }

    pub fn membership_epoch(&self) -> u64 {
        self.membership_epoch
    }

    pub fn is_healthy_index(&self, index: usize) -> bool {
        self.healthy_pos.get(index).copied().flatten().is_some()
    }

    pub fn begin_request(&self, index: usize) {
        if let Some(backend) = self.backends.get(index) {
            backend.active_requests.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn finish_request(&mut self, index: usize, latency: Duration, status: Option<u16>) {
        let Some(backend) = self.backends.get_mut(index) else {
            return;
        };

        let _ =
            backend
                .active_requests
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    Some(current.saturating_sub(1))
                });

        if status.is_some_and(|code| (500..=599).contains(&code)) {
            return;
        }

        let observed_ms = latency.as_secs_f64() * 1_000.0;
        let alpha = 0.2_f64;
        backend.ewma_latency_ms = Some(match backend.ewma_latency_ms {
            Some(previous) => alpha * observed_ms + (1.0 - alpha) * previous,
            None => observed_ms,
        });
    }

    fn mark_healthy(&mut self, index: usize) -> bool {
        if index >= self.backends.len() {
            return false;
        }

        if self.healthy_pos[index].is_some() {
            return false;
        }

        let pos = self.healthy.len();
        self.healthy.push(index);
        self.healthy_pos[index] = Some(pos);
        true
    }

    fn mark_unhealthy(&mut self, index: usize) -> bool {
        if index >= self.backends.len() {
            return false;
        }

        let Some(pos) = self.healthy_pos[index] else {
            return false;
        };

        let removed = self.healthy.swap_remove(pos);
        debug_assert_eq!(removed, index);

        if pos < self.healthy.len() {
            let moved_index = self.healthy[pos];
            self.healthy_pos[moved_index] = Some(pos);
        }

        self.healthy_pos[index] = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use spooky_config::config::{Backend, HealthCheck};

    use super::BackendPool;
    use crate::{
        backend::BackendState,
        health::{HealthFailureReason, HealthTransition},
    };

    fn create_backend_state(address: &str, weight: u32) -> BackendState {
        BackendState::new(&Backend {
            id: format!("backend-{address}"),
            address: address.to_string(),
            weight,
            health_check: Some(HealthCheck {
                path: "/health".to_string(),
                interval: 1000,
                timeout_ms: 1000,
                failure_threshold: 3,
                success_threshold: 1,
                cooldown_ms: 0,
            }),
        })
    }

    #[test]
    fn backend_pool_epoch_changes_only_on_health_membership_transition() {
        let mut pool = BackendPool::new_from_states(vec![create_backend_state("10.0.0.1:1", 1)]);
        assert_eq!(pool.membership_epoch(), 0);

        pool.mark_failure(0);
        pool.mark_failure(0);
        assert_eq!(pool.membership_epoch(), 0);

        pool.mark_failure(0);
        assert_eq!(pool.membership_epoch(), 1);

        pool.mark_failure(0);
        assert_eq!(pool.membership_epoch(), 1);

        pool.mark_success(0);
        assert_eq!(pool.membership_epoch(), 2);

        pool.mark_success(0);
        assert_eq!(pool.membership_epoch(), 2);
    }

    #[test]
    fn healthy_cache_tracks_membership_changes_without_duplicates() {
        let mut pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 1),
            create_backend_state("10.0.0.2:1", 1),
            create_backend_state("10.0.0.3:1", 1),
        ]);

        assert_eq!(pool.healthy_indices(), vec![0, 1, 2]);

        pool.mark_failure(1);
        pool.mark_failure(1);
        pool.mark_failure(1);
        assert_eq!(pool.healthy_indices(), vec![0, 2]);

        pool.mark_failure(1);
        assert_eq!(pool.healthy_indices(), vec![0, 2]);

        pool.mark_success(1);
        let healthy = pool.healthy_indices();
        assert_eq!(healthy.len(), 3);
        assert!(healthy.contains(&0));
        assert!(healthy.contains(&1));
        assert!(healthy.contains(&2));
    }

    #[test]
    fn backend_recovers_after_success_threshold() {
        let mut pool = BackendPool::new_from_states(vec![create_backend_state("10.0.0.1:1", 1)]);
        pool.mark_failure(0);
        pool.mark_failure(0);
        pool.mark_failure(0);

        assert!(pool.healthy_indices().is_empty());
        pool.mark_success(0);
        assert_eq!(pool.healthy_indices(), vec![0]);
    }

    #[test]
    fn passively_ejected_backend_recovers_after_cooldown() {
        let backend = Backend {
            id: "b1".to_string(),
            address: "10.0.0.1:1".to_string(),
            weight: 1,
            health_check: Some(HealthCheck {
                path: "/health".to_string(),
                interval: 0,
                timeout_ms: 1000,
                failure_threshold: 2,
                success_threshold: 1,
                cooldown_ms: 10_000,
            }),
        };
        let mut pool = BackendPool::new_from_states(vec![BackendState::new(&backend)]);
        assert_eq!(pool.healthy_len(), 1);

        pool.mark_request_failure(0, HealthFailureReason::Transport);
        let transition = pool.mark_request_failure(0, HealthFailureReason::Transport);
        assert!(matches!(
            transition,
            Some(HealthTransition::BecameUnhealthy)
        ));
        assert_eq!(pool.healthy_len(), 0);
        assert!(pool.readmit_due());

        pool.reconcile_readmit_at(Instant::now());
        assert_eq!(pool.healthy_len(), 0);
        assert!(pool.readmit_due());

        pool.reconcile_readmit_at(Instant::now() + Duration::from_millis(10_001));
        assert_eq!(pool.healthy_len(), 1);
        assert!(!pool.readmit_due());
    }
}
