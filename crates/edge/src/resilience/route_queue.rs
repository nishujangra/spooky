use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub struct RouteQueueLimiter {
    default_cap: usize,
    global_cap: usize,
    caps: HashMap<String, usize>,
    inflight: Mutex<RouteQueueState>,
}

#[derive(Default)]
struct RouteQueueState {
    total: usize,
    by_route: HashMap<String, usize>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RouteQueueRejection {
    GlobalCap,
    RouteCap,
}

impl RouteQueueLimiter {
    pub fn new(default_cap: usize, global_cap: usize, caps: HashMap<String, usize>) -> Self {
        Self {
            default_cap: default_cap.max(1),
            global_cap: global_cap.max(1),
            caps,
            inflight: Mutex::new(RouteQueueState::default()),
        }
    }

    pub fn try_acquire(
        self: &Arc<Self>,
        route: &str,
    ) -> Result<RouteQueuePermit, RouteQueueRejection> {
        let cap = self
            .caps
            .get(route)
            .copied()
            .unwrap_or(self.default_cap)
            .max(1);
        let mut guard = self
            .inflight
            .lock()
            .map_err(|_| RouteQueueRejection::GlobalCap)?;
        if guard.total >= self.global_cap {
            return Err(RouteQueueRejection::GlobalCap);
        }
        let current = guard.by_route.get(route).copied().unwrap_or(0);
        if current >= cap {
            return Err(RouteQueueRejection::RouteCap);
        }
        guard.total = guard.total.saturating_add(1);
        guard.by_route.insert(route.to_string(), current + 1);
        Ok(RouteQueuePermit {
            limiter: Arc::clone(self),
            route: route.to_string(),
        })
    }
}

pub struct RouteQueuePermit {
    limiter: Arc<RouteQueueLimiter>,
    route: String,
}

impl Drop for RouteQueuePermit {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.limiter.inflight.lock()
            && let Some(current) = guard.by_route.get_mut(&self.route)
        {
            *current = current.saturating_sub(1);
            if *current == 0 {
                guard.by_route.remove(&self.route);
            }
            guard.total = guard.total.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use super::{RouteQueueLimiter, RouteQueueRejection};

    #[test]
    fn try_acquire_distinguishes_route_cap_from_global_cap() {
        let limiter = Arc::new(RouteQueueLimiter::new(
            2,
            3,
            HashMap::from([(String::from("api"), 1usize)]),
        ));

        let api = limiter.try_acquire("api").expect("api permit");
        assert!(matches!(
            limiter.try_acquire("api"),
            Err(RouteQueueRejection::RouteCap)
        ));

        let other_a = limiter.try_acquire("other-a").expect("other-a permit");
        let other_b = limiter.try_acquire("other-b").expect("other-b permit");
        assert!(matches!(
            limiter.try_acquire("other-c"),
            Err(RouteQueueRejection::GlobalCap)
        ));

        drop(api);
        drop(other_a);
        drop(other_b);
    }

    #[test]
    fn dropping_permit_releases_capacity_for_same_route() {
        let limiter = Arc::new(RouteQueueLimiter::new(1, 1, HashMap::new()));

        let permit = limiter.try_acquire("api").expect("first permit");
        assert!(matches!(
            limiter.try_acquire("api"),
            Err(RouteQueueRejection::GlobalCap)
        ));

        drop(permit);

        assert!(limiter.try_acquire("api").is_ok());
    }
}
