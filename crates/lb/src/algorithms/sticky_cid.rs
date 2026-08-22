use crate::{algorithms::consistent_hash::ConsistentHash, backend_pool::BackendPool};

pub struct StickyCid {
    inner: ConsistentHash,
}

impl StickyCid {
    pub fn new(replicas: u32) -> Self {
        Self {
            inner: ConsistentHash::new(replicas),
        }
    }

    pub fn pick(&mut self, key: &str, pool: &BackendPool) -> Option<usize> {
        if key.is_empty() {
            return pool.healthy.first().copied();
        }
        self.inner.pick(key, pool)
    }
}

#[cfg(test)]
mod tests {
    use impulse_config::config::{Backend, HealthCheck};

    use super::StickyCid;
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
    fn sticky_cid_is_deterministic_for_same_key() {
        let pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 1),
            create_backend_state("10.0.0.2:1", 1),
            create_backend_state("10.0.0.3:1", 1),
        ]);

        let mut lb = StickyCid::new(16);
        let first = lb.pick("cid:abc123", &pool);
        let second = lb.pick("cid:abc123", &pool);
        assert_eq!(first, second);
    }

    #[test]
    fn sticky_cid_keeps_weighted_ring_membership() {
        let mut pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 100),
            create_backend_state("10.0.0.2:1", 200),
        ]);

        let mut lb = StickyCid::new(16);
        let _ = lb.pick("cid:alpha", &pool);
        assert_eq!(lb.inner.ring.len(), (16 * (100 + 200)) as usize);

        pool.mark_failure(1);
        pool.mark_failure(1);
        pool.mark_failure(1);

        let _ = lb.pick("cid:beta", &pool);
        assert_eq!(lb.inner.ring.len(), (16 * 100) as usize);
    }
}
