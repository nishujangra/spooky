use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use spooky_errors::RetryPolicyDenialReason;

pub struct RetryBudget {
    enabled: bool,
    global_ratio_percent: u8,
    per_route_ratio_percent: HashMap<String, u8>,
    global_primary: AtomicU64,
    global_retries: AtomicU64,
    route_stats: Mutex<HashMap<String, (u64, u64)>>,
}

impl RetryBudget {
    pub fn new(
        enabled: bool,
        global_ratio_percent: u8,
        per_route_ratio_percent: HashMap<String, u8>,
    ) -> Self {
        Self {
            enabled,
            global_ratio_percent,
            per_route_ratio_percent,
            global_primary: AtomicU64::new(0),
            global_retries: AtomicU64::new(0),
            route_stats: Mutex::new(HashMap::new()),
        }
    }

    pub fn mark_primary(&self, route: &str) {
        self.global_primary.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut stats) = self.route_stats.lock() {
            let entry = stats.entry(route.to_string()).or_insert((0, 0));
            entry.0 = entry.0.saturating_add(1);
        }
    }

    pub fn allow_retry(&self, route: &str) -> Result<(), RetryPolicyDenialReason> {
        if !self.enabled {
            return Ok(());
        }

        let ratio = self
            .per_route_ratio_percent
            .get(route)
            .copied()
            .unwrap_or(self.global_ratio_percent);

        let primary = self.global_primary.load(Ordering::Relaxed);
        let retries = self.global_retries.load(Ordering::Relaxed);
        let global_limit = ((primary * ratio as u64) / 100).saturating_add(1);
        if retries >= global_limit {
            return Err(RetryPolicyDenialReason::BudgetDenied);
        }

        let mut route_allowed = true;
        if let Ok(mut stats) = self.route_stats.lock() {
            let entry = stats.entry(route.to_string()).or_insert((0, 0));
            let route_limit = ((entry.0 * ratio as u64) / 100).saturating_add(1);
            if entry.1 >= route_limit {
                route_allowed = false;
            } else {
                entry.1 = entry.1.saturating_add(1);
            }
        }
        if !route_allowed {
            return Err(RetryPolicyDenialReason::BudgetDenied);
        }

        self.global_retries.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use spooky_errors::RetryPolicyDenialReason;

    use super::RetryBudget;

    #[test]
    fn allow_retry_denial_does_not_consume_retry_budget_counters() {
        let budget = RetryBudget::new(true, 100, HashMap::new());

        assert_eq!(budget.allow_retry("api"), Ok(()));
        assert_eq!(
            budget.allow_retry("api"),
            Err(RetryPolicyDenialReason::BudgetDenied)
        );
        assert_eq!(
            budget
                .global_primary
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            budget
                .global_retries
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert_eq!(
            budget.route_stats.lock().expect("route stats").get("api"),
            Some(&(0, 1))
        );

        budget.mark_primary("api");
        assert_eq!(budget.allow_retry("api"), Ok(()));
        assert_eq!(
            budget.allow_retry("api"),
            Err(RetryPolicyDenialReason::BudgetDenied)
        );
        assert_eq!(
            budget
                .global_primary
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert_eq!(
            budget
                .global_retries
                .load(std::sync::atomic::Ordering::Relaxed),
            2
        );
        assert_eq!(
            budget.route_stats.lock().expect("route stats").get("api"),
            Some(&(1, 2))
        );
    }

    #[test]
    fn route_specific_budget_denial_leaves_other_routes_retryable() {
        let budget = RetryBudget::new(
            true,
            100,
            HashMap::from([(String::from("api"), 0), (String::from("other"), 100)]),
        );
        budget.mark_primary("api");
        budget.mark_primary("other");

        assert_eq!(budget.allow_retry("api"), Ok(()));
        assert_eq!(
            budget.allow_retry("api"),
            Err(RetryPolicyDenialReason::BudgetDenied)
        );
        assert_eq!(budget.allow_retry("other"), Ok(()));
        assert_eq!(
            budget
                .global_retries
                .load(std::sync::atomic::Ordering::Relaxed),
            2
        );
        assert_eq!(
            budget.route_stats.lock().expect("route stats").get("api"),
            Some(&(1, 1))
        );
        assert_eq!(
            budget.route_stats.lock().expect("route stats").get("other"),
            Some(&(1, 1))
        );
    }
}
