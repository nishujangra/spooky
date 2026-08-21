use crate::{
    backend_pool::BackendPool,
    hash::{expected_ring_entries, hash_backend_replica, hash64},
};

pub struct ConsistentHash {
    pub replicas: u32,
    pub ring: Vec<(u64, usize)>,
    pub ring_epoch: Option<u64>,
    pub ring_rebuilds: u64,
}

impl ConsistentHash {
    pub fn new(replicas: u32) -> Self {
        Self {
            replicas: replicas.max(1),
            ring: Vec::new(),
            ring_epoch: None,
            ring_rebuilds: 0,
        }
    }

    pub fn pick(&mut self, key: &str, pool: &BackendPool) -> Option<usize> {
        if pool.is_empty() {
            return None;
        }

        let epoch = pool.membership_epoch();
        if self.ring_epoch != Some(epoch) {
            self.rebuild_ring(pool);
            self.ring_epoch = Some(epoch);
            self.ring_rebuilds = self.ring_rebuilds.wrapping_add(1);
        }

        if self.ring.is_empty() {
            return None;
        }

        let key_hash = hash64(key.as_bytes());
        let lookup_idx = match self.ring.binary_search_by(|(hash, _)| hash.cmp(&key_hash)) {
            Ok(idx) => idx,
            Err(idx) if idx < self.ring.len() => idx,
            Err(_) => 0,
        };

        Some(self.ring[lookup_idx].1)
    }

    fn rebuild_ring(&mut self, pool: &BackendPool) {
        self.ring.clear();

        let expected = expected_ring_entries(pool, self.replicas);
        if self.ring.capacity() < expected {
            self.ring.reserve(expected - self.ring.capacity());
        }

        for &idx in &pool.healthy {
            let backend = &pool.backends[idx];
            let replicas = self.replicas.saturating_mul(backend.weight());
            for replica in 0..replicas {
                self.ring
                    .push((hash_backend_replica(backend.address(), replica), idx));
            }
        }

        self.ring.sort_unstable();
    }
}

#[cfg(test)]
mod tests {
    use impulse_config::config::{Backend, HealthCheck};

    use super::ConsistentHash;
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
    fn consistent_hash_is_stable() {
        let pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 1),
            create_backend_state("10.0.0.2:1", 1),
            create_backend_state("10.0.0.3:1", 1),
        ]);

        let mut ch = ConsistentHash::new(16);
        let first = ch.pick("user:123", &pool);
        let second = ch.pick("user:123", &pool);
        assert_eq!(first, second);
    }

    #[test]
    fn consistent_hash_rebuilds_only_when_membership_changes() {
        let mut pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 1),
            create_backend_state("10.0.0.2:1", 1),
            create_backend_state("10.0.0.3:1", 1),
        ]);

        let mut ch = ConsistentHash::new(16);

        let _ = ch.pick("user:123", &pool);
        let first_rebuilds = ch.ring_rebuilds;
        let first_len = ch.ring.len();
        assert_eq!(first_rebuilds, 1);

        for key in ["user:123", "user:124", "user:125", "user:126"] {
            let _ = ch.pick(key, &pool);
        }
        assert_eq!(ch.ring_rebuilds, first_rebuilds);
        assert_eq!(ch.ring.len(), first_len);

        pool.mark_failure(0);
        pool.mark_failure(0);
        pool.mark_failure(0);

        let _ = ch.pick("user:127", &pool);
        assert_eq!(ch.ring_rebuilds, first_rebuilds + 1);
        assert!(ch.ring.len() < first_len);
    }

    #[test]
    fn consistent_hash_ring_size_matches_weighted_healthy_membership() {
        let mut pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 2),
            create_backend_state("10.0.0.2:1", 3),
        ]);

        let mut ch = ConsistentHash::new(8);

        let _ = ch.pick("user:1", &pool);
        assert_eq!(ch.ring.len(), (8 * (2 + 3)) as usize);

        pool.mark_failure(0);
        pool.mark_failure(0);
        pool.mark_failure(0);

        let _ = ch.pick("user:2", &pool);
        assert_eq!(ch.ring.len(), (8 * 3) as usize);
    }
}
