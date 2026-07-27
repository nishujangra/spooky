//! Backend endpoint and health-check canonicalization.

use std::time::Duration;

use spooky_config::config::HealthCheck;

use crate::common::{api_runtime_upstream, api_upstream_mut, runtime_config, sample_config};

#[test]
fn runtime_config_lowers_backend_endpoint_and_health_check_contract() {
    let mut config = sample_config();
    api_upstream_mut(&mut config).backends[0].health_check = Some(HealthCheck {
        path: String::new(),
        interval: 2_000,
        timeout_ms: 250,
        failure_threshold: 4,
        success_threshold: 3,
        cooldown_ms: 5_000,
    });

    let runtime = runtime_config(&config);
    let backend = &api_runtime_upstream(&runtime).backends[0];
    let health = backend.health_check.as_ref().expect("health check");

    assert_eq!(backend.endpoint.origin, "https://api.internal:8443");
    assert_eq!(health.path, "/");
    assert_eq!(health.interval, Duration::from_millis(2_000));
    assert_eq!(health.timeout, Duration::from_millis(250));
    assert_eq!(health.failure_threshold, 4);
    assert_eq!(health.success_threshold, 3);
}
