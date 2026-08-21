use std::sync::atomic::{AtomicUsize, Ordering};

use crate::backend_pool::BackendPool;

pub struct RoundRobin {
    next: usize,
    next_read: AtomicUsize,
}

impl RoundRobin {
    pub fn new() -> Self {
        Self {
            next: 0,
            next_read: AtomicUsize::new(0),
        }
    }

    pub fn pick(&mut self, pool: &BackendPool) -> Option<usize> {
        if pool.healthy.is_empty() {
            return None;
        }

        let idx = pool.healthy[self.next % pool.healthy.len()];
        self.next = self.next.wrapping_add(1);
        Some(idx)
    }

    pub fn pick_readonly(&self, pool: &BackendPool) -> Option<usize> {
        if pool.healthy.is_empty() {
            return None;
        }

        let next = self.next_read.fetch_add(1, Ordering::Relaxed);
        let idx = pool.healthy[next % pool.healthy.len()];
        Some(idx)
    }
}

impl Default for RoundRobin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use impulse_config::config::{Backend, HealthCheck};

    use super::RoundRobin;
    use crate::{backend::BackendState, backend_pool::BackendPool};

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
    fn round_robin_cycles() {
        let pool = BackendPool::new_from_states(vec![
            create_backend_state("127.0.0.1:1", 1),
            create_backend_state("127.0.0.1:2", 1),
            create_backend_state("127.0.0.1:3", 1),
        ]);
        let mut rr = RoundRobin::new();

        let picks: Vec<usize> = (0..6).filter_map(|_| rr.pick(&pool)).collect();
        assert_eq!(picks, vec![0, 1, 2, 0, 1, 2]);
    }

    #[test]
    fn unhealthy_backends_are_skipped() {
        let mut pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 1),
            create_backend_state("10.0.0.2:1", 1),
        ]);

        pool.mark_failure(0);
        pool.mark_failure(0);
        pool.mark_failure(0);

        let mut rr = RoundRobin::new();
        let pick = rr.pick(&pool).expect("pick");
        assert_eq!(pick, 1);
    }

    #[test]
    fn no_healthy_backends_returns_none() {
        let mut pool = BackendPool::new_from_states(vec![create_backend_state("10.0.0.1:1", 1)]);
        pool.mark_failure(0);
        pool.mark_failure(0);
        pool.mark_failure(0);

        let mut rr = RoundRobin::new();
        assert!(rr.pick(&pool).is_none());
    }
}
