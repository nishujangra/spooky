use std::{cell::RefCell, sync::RwLock};

use rand::{Rng, SeedableRng, rngs::StdRng};

use crate::backend_pool::BackendPool;

thread_local! {
    static LB_RANDOM_RNG: RefCell<StdRng> = RefCell::new(StdRng::from_entropy());
}

pub struct Random {
    weighted_members: RwLock<WeightedHealthyMembers>,
}

#[derive(Default)]
struct WeightedHealthyMembers {
    membership_epoch: Option<u64>,
    cumulative: Vec<(u64, usize)>,
    total_weight: u64,
}

impl Random {
    pub fn new() -> Self {
        Self {
            weighted_members: RwLock::new(WeightedHealthyMembers::default()),
        }
    }

    pub fn pick(&mut self, pool: &BackendPool) -> Option<usize> {
        self.pick_readonly(pool)
    }

    pub fn pick_readonly(&self, pool: &BackendPool) -> Option<usize> {
        if pool.healthy.is_empty() {
            return None;
        }

        let membership_epoch = pool.membership_epoch();
        {
            let weighted_members = self
                .weighted_members
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if weighted_members.membership_epoch == Some(membership_epoch)
                && weighted_members.total_weight > 0
            {
                return Self::pick_from_members(&weighted_members);
            }
        }

        let mut weighted_members = self
            .weighted_members
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if weighted_members.membership_epoch != Some(membership_epoch)
            || weighted_members.total_weight == 0
        {
            *weighted_members = build_weighted_members(pool, membership_epoch);
        }

        Self::pick_from_members(&weighted_members)
    }

    fn pick_from_members(weighted_members: &WeightedHealthyMembers) -> Option<usize> {
        if weighted_members.total_weight == 0 {
            return None;
        }

        let draw = LB_RANDOM_RNG.with(|state| {
            let mut rng = state.borrow_mut();
            rng.gen_range(0..weighted_members.total_weight)
        });

        let index = weighted_members
            .cumulative
            .partition_point(|(cumulative_weight, _)| *cumulative_weight <= draw);
        weighted_members
            .cumulative
            .get(index)
            .map(|(_, backend)| *backend)
    }
}

fn build_weighted_members(pool: &BackendPool, membership_epoch: u64) -> WeightedHealthyMembers {
    let mut cumulative = Vec::with_capacity(pool.healthy.len());
    let mut total_weight = 0_u64;

    for &backend_index in &pool.healthy {
        let Some(backend) = pool.backend(backend_index) else {
            continue;
        };
        total_weight = total_weight.saturating_add(backend.weight() as u64);
        cumulative.push((total_weight, backend_index));
    }

    WeightedHealthyMembers {
        membership_epoch: Some(membership_epoch),
        cumulative,
        total_weight,
    }
}

impl Default for Random {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use impulse_config::config::{Backend, HealthCheck};

    use super::{Random, build_weighted_members};
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
    fn weighted_random_favors_higher_weight_backend_over_large_sample() {
        let pool = BackendPool::new_from_states(vec![
            create_backend_state("127.0.0.1:1", 100),
            create_backend_state("127.0.0.1:2", 200),
        ]);
        let random = Random::new();

        let picks: Vec<usize> = (0..6000)
            .filter_map(|_| random.pick_readonly(&pool))
            .collect();
        let backend_zero_picks = picks.iter().filter(|&&pick| pick == 0).count();
        let backend_one_picks = picks.iter().filter(|&&pick| pick == 1).count();

        let backend_zero_share = backend_zero_picks as f64 / picks.len() as f64;
        let backend_one_share = backend_one_picks as f64 / picks.len() as f64;

        assert!(
            (0.28..0.38).contains(&backend_zero_share),
            "expected backend 0 share near 1/3, got {backend_zero_share}"
        );
        assert!(
            (0.62..0.72).contains(&backend_one_share),
            "expected backend 1 share near 2/3, got {backend_one_share}"
        );
    }

    #[test]
    fn single_healthy_backend_always_wins_regardless_of_weight() {
        let mut pool = BackendPool::new_from_states(vec![
            create_backend_state("127.0.0.1:1", 100),
            create_backend_state("127.0.0.1:2", 200),
        ]);
        pool.mark_failure(0);
        pool.mark_failure(0);
        pool.mark_failure(0);

        let random = Random::new();
        let picks: Vec<usize> = (0..64)
            .filter_map(|_| random.pick_readonly(&pool))
            .collect();
        assert!(picks.iter().all(|pick| *pick == 1));
    }

    #[test]
    fn unhealthy_backends_are_excluded_from_weighted_pool() {
        let mut pool = BackendPool::new_from_states(vec![
            create_backend_state("127.0.0.1:1", 100),
            create_backend_state("127.0.0.1:2", 200),
            create_backend_state("127.0.0.1:3", 300),
        ]);
        pool.mark_failure(1);
        pool.mark_failure(1);
        pool.mark_failure(1);

        let weighted_members = build_weighted_members(&pool, pool.membership_epoch());
        assert_eq!(weighted_members.total_weight, 400);
        assert_eq!(weighted_members.cumulative, vec![(100, 0), (400, 2)]);
    }

    #[test]
    fn pick_and_pick_readonly_share_the_same_weighted_membership_contract() {
        let pool = BackendPool::new_from_states(vec![
            create_backend_state("127.0.0.1:1", 100),
            create_backend_state("127.0.0.1:2", 200),
            create_backend_state("127.0.0.1:3", 300),
        ]);
        let mut random = Random::new();

        let _ = random.pick(&pool);
        let cache_after_pick = random
            .weighted_members
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .total_weight;
        let _ = random.pick_readonly(&pool);
        let cache_after_readonly = random
            .weighted_members
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .total_weight;

        assert_eq!(cache_after_pick, 600);
        assert_eq!(cache_after_readonly, cache_after_pick);
    }
}
