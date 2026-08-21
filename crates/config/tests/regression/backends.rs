//! Backend endpoint and health-check canonicalization.

use std::time::Duration;

use impulse_config::config::HealthCheck;

use crate::common::{api_backend_mut, api_runtime_upstream, sample_runtime_config_with};

#[test]
fn runtime_config_lowers_backend_endpoint_and_health_check_contract() {
    let runtime = sample_runtime_config_with(|config| {
        api_backend_mut(config).health_check = Some(HealthCheck {
            path: String::new(),
            interval: 2_000,
            timeout_ms: 250,
            failure_threshold: 4,
            success_threshold: 3,
            cooldown_ms: 5_000,
        });
    });
    let backend = &api_runtime_upstream(&runtime).backends[0];
    let health = backend.health_check.as_ref().expect("health check");

    assert_eq!(backend.endpoint.origin, "https://api.internal:8443");
    assert_eq!(health.path, "/");
    assert_eq!(health.interval, Duration::from_millis(2_000));
    assert_eq!(health.timeout, Duration::from_millis(250));
    assert_eq!(health.failure_threshold, 4);
    assert_eq!(health.success_threshold, 3);
}
