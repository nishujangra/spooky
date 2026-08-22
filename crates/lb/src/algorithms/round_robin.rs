use std::sync::{
    RwLock,
    atomic::{AtomicUsize, Ordering},
};

use crate::backend_pool::BackendPool;

pub struct RoundRobin {
    next: usize,
    next_read: AtomicUsize,
    schedule: RwLock<WeightedSchedule>,
}

#[derive(Default)]
struct WeightedSchedule {
    membership_epoch: Option<u64>,
    sequence: Vec<usize>,
}

impl RoundRobin {
    pub fn new() -> Self {
        Self {
            next: 0,
            next_read: AtomicUsize::new(0),
            schedule: RwLock::new(WeightedSchedule::default()),
        }
    }

    pub fn pick(&mut self, pool: &BackendPool) -> Option<usize> {
        let schedule_len = self.refresh_schedule(pool)?;
        let sequence_pos = self.next % schedule_len;
        self.next = self.next.wrapping_add(1);
        self.schedule
            .read()
            .expect("round-robin schedule lock poisoned")
            .sequence
            .get(sequence_pos)
            .copied()
    }

    pub fn pick_readonly(&self, pool: &BackendPool) -> Option<usize> {
        let schedule_len = self.refresh_schedule(pool)?;
        let next = self.next_read.fetch_add(1, Ordering::Relaxed);
        self.schedule
            .read()
            .expect("round-robin schedule lock poisoned")
            .sequence
            .get(next % schedule_len)
            .copied()
    }

    fn refresh_schedule(&self, pool: &BackendPool) -> Option<usize> {
        if pool.healthy.is_empty() {
            return None;
        }

        let membership_epoch = pool.membership_epoch();
        {
            let schedule = self
                .schedule
                .read()
                .expect("round-robin schedule lock poisoned");
            if schedule.membership_epoch == Some(membership_epoch) && !schedule.sequence.is_empty()
            {
                return Some(schedule.sequence.len());
            }
        }

        let mut schedule = self
            .schedule
            .write()
            .expect("round-robin schedule lock poisoned");
        if schedule.membership_epoch != Some(membership_epoch) || schedule.sequence.is_empty() {
            schedule.membership_epoch = Some(membership_epoch);
            schedule.sequence = build_weighted_sequence(pool);
        }

        (!schedule.sequence.is_empty()).then_some(schedule.sequence.len())
    }
}

fn build_weighted_sequence(pool: &BackendPool) -> Vec<usize> {
    let mut members = Vec::with_capacity(pool.healthy.len());
    for &backend_index in &pool.healthy {
        let Some(backend) = pool.backend(backend_index) else {
            continue;
        };
        members.push((backend_index, backend.weight()));
    }

    if members.is_empty() {
        return Vec::new();
    }

    let weight_gcd = members.iter().fold(0, |acc, (_, weight)| gcd(acc, *weight));
    let normalized: Vec<(usize, i64)> = members
        .into_iter()
        .map(|(backend_index, weight)| (backend_index, (weight / weight_gcd.max(1)) as i64))
        .collect();
    let total_weight: i64 = normalized.iter().map(|(_, weight)| *weight).sum();
    let mut current_weights = vec![0_i64; normalized.len()];
    let mut sequence = Vec::with_capacity(total_weight as usize);

    for _ in 0..total_weight {
        let mut selected = 0usize;
        let mut best_weight = i64::MIN;

        for (position, (_, weight)) in normalized.iter().enumerate() {
            current_weights[position] += *weight;
            if current_weights[position] > best_weight {
                best_weight = current_weights[position];
                selected = position;
            }
        }

        current_weights[selected] -= total_weight;
        sequence.push(normalized[selected].0);
    }

    sequence
}

fn gcd(mut left: u32, mut right: u32) -> u32 {
    if left == 0 {
        return right.max(1);
    }

    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }

    left.max(1)
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
    fn round_robin_honors_weights_deterministically() {
        let pool = BackendPool::new_from_states(vec![
            create_backend_state("127.0.0.1:1", 100),
            create_backend_state("127.0.0.1:2", 200),
        ]);
        let mut rr = RoundRobin::new();

        let picks: Vec<usize> = (0..12).filter_map(|_| rr.pick(&pool)).collect();
        assert_eq!(picks, vec![1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1]);
        assert_eq!(picks.iter().filter(|&&pick| pick == 0).count(), 4);
        assert_eq!(picks.iter().filter(|&&pick| pick == 1).count(), 8);
    }

    #[test]
    fn readonly_round_robin_honors_same_weighted_schedule() {
        let pool = BackendPool::new_from_states(vec![
            create_backend_state("127.0.0.1:1", 100),
            create_backend_state("127.0.0.1:2", 200),
        ]);
        let mut rr = RoundRobin::new();

        let writable_picks: Vec<usize> = (0..12).filter_map(|_| rr.pick(&pool)).collect();
        let readonly_rr = RoundRobin::new();
        let readonly_picks: Vec<usize> = (0..12)
            .filter_map(|_| readonly_rr.pick_readonly(&pool))
            .collect();

        assert_eq!(readonly_picks, writable_picks);
    }

    #[test]
    fn unhealthy_backends_are_skipped() {
        let mut pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 100),
            create_backend_state("10.0.0.2:1", 200),
            create_backend_state("10.0.0.3:1", 100),
        ]);
        let mut rr = RoundRobin::new();

        let initial = rr.pick(&pool).expect("initial weighted pick");
        assert_eq!(initial, 1);

        pool.mark_failure(1);
        pool.mark_failure(1);
        pool.mark_failure(1);

        let picks: Vec<usize> = (0..6).filter_map(|_| rr.pick(&pool)).collect();
        assert_eq!(picks, vec![2, 0, 2, 0, 2, 0]);
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
