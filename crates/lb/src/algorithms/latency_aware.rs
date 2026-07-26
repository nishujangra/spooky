use crate::backend_pool::BackendPool;

pub struct LatencyAware;

impl LatencyAware {
    pub fn new() -> Self {
        Self
    }

    pub fn pick(&mut self, pool: &BackendPool) -> Option<usize> {
        self.pick_readonly(pool)
    }

    pub fn pick_readonly(&self, pool: &BackendPool) -> Option<usize> {
        let mut unsampled_best: Option<(usize, usize)> = None;
        let mut sampled_best: Option<(f64, usize, usize)> = None;

        for &idx in &pool.healthy {
            let backend = &pool.backends[idx];
            let active = backend.active_requests();
            if let Some(ewma) = backend.ewma_latency_ms() {
                let score = ewma + (active as f64 * 10.0);
                match sampled_best {
                    Some((best_score, best_active, best_idx)) => {
                        if score < best_score
                            || (score == best_score
                                && (active < best_active
                                    || (active == best_active && idx < best_idx)))
                        {
                            sampled_best = Some((score, active, idx));
                        }
                    }
                    None => sampled_best = Some((score, active, idx)),
                }
            } else {
                match unsampled_best {
                    Some((best_active, best_idx)) => {
                        if active < best_active || (active == best_active && idx < best_idx) {
                            unsampled_best = Some((active, idx));
                        }
                    }
                    None => unsampled_best = Some((active, idx)),
                }
            }
        }

        if let Some((_, idx)) = unsampled_best {
            return Some(idx);
        }
        sampled_best.map(|(_, _, idx)| idx)
    }
}

impl Default for LatencyAware {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use spooky_config::config::{Backend, HealthCheck};

    use super::LatencyAware;
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
    fn latency_aware_prefers_lower_ewma() {
        let mut pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 1),
            create_backend_state("10.0.0.2:1", 1),
        ]);

        pool.finish_request(0, Duration::from_millis(150), Some(200));
        pool.finish_request(1, Duration::from_millis(20), Some(200));

        let mut lb = LatencyAware::new();
        assert_eq!(lb.pick(&pool), Some(1));
    }
}
