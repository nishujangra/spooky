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
        let mut sampled_latency_sum = 0.0_f64;
        let mut sampled_count = 0_usize;

        for &idx in &pool.healthy {
            let backend = &pool.backends[idx];
            let active = backend.active_requests();
            if let Some(ewma) = backend.ewma_latency_ms() {
                let score = ewma + (active as f64 * 10.0);
                sampled_latency_sum += ewma;
                sampled_count += 1;
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

        if sampled_count == 0 {
            return unsampled_best.map(|(_, idx)| idx);
        }

        let Some((best_score, best_active, best_idx)) = sampled_best else {
            return unsampled_best.map(|(_, idx)| idx);
        };

        if let Some((unsampled_active, unsampled_idx)) = unsampled_best {
            let neutral_baseline = sampled_latency_sum / sampled_count as f64;
            let unsampled_score = neutral_baseline + (unsampled_active as f64 * 10.0);
            if unsampled_score < best_score
                || (unsampled_score == best_score
                    && (unsampled_active < best_active
                        || (unsampled_active == best_active && unsampled_idx < best_idx)))
            {
                return Some(unsampled_idx);
            }
        }

        Some(best_idx)
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

    use impulse_config::config::{Backend, HealthCheck};

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

    #[test]
    fn latency_aware_does_not_implicitly_prefer_unsampled_backends() {
        let mut pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 1),
            create_backend_state("10.0.0.2:1", 1),
        ]);

        pool.finish_request(0, Duration::from_millis(25), Some(200));

        let lb = LatencyAware::new();
        assert_eq!(lb.pick_readonly(&pool), Some(0));
    }

    #[test]
    fn latency_aware_preserves_startup_behavior_when_all_backends_are_unsampled() {
        let pool = BackendPool::new_from_states(vec![
            create_backend_state("10.0.0.1:1", 1),
            create_backend_state("10.0.0.2:1", 1),
        ]);

        let lb = LatencyAware::new();
        assert_eq!(lb.pick_readonly(&pool), Some(0));
    }
}
