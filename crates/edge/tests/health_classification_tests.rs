use http::StatusCode;
use spooky_config::{
    config::{Backend, Config, HealthCheck, Listen, RouteMatch, Tls, Upstream},
    runtime::RuntimeConfig,
};
use spooky_edge::runtime::health::{HealthClassification, outcome_from_status};
use spooky_lb::{HealthTransition, upstream_pool::UpstreamPool};

fn create_test_upstream_pool() -> UpstreamPool {
    let mut upstreams = std::collections::HashMap::new();
    upstreams.insert(
        "api".to_string(),
        Upstream {
            load_balancing: spooky_config::config::LoadBalancing {
                lb_type: "round-robin".to_string(),
                key: None,
            },
            auth: Default::default(),
            host_policy: Default::default(),
            forwarded_headers: Default::default(),
            tls: None,
            route: RouteMatch {
                path_prefix: Some("/".to_string()),
                ..Default::default()
            },
            backends: vec![Backend {
                id: "bk-1".to_string(),
                address: "127.0.0.1:8001".to_string(),
                weight: 1,
                health_check: Some(HealthCheck {
                    path: "/health".to_string(),
                    interval: 1000,
                    timeout_ms: 5000,
                    failure_threshold: 3,
                    success_threshold: 2,
                    cooldown_ms: 10000,
                }),
            }],
        },
    );

    let runtime = RuntimeConfig::from_config(&Config {
        version: 1,
        listen: Listen {
            protocol: "http1".to_string(),
            tls: Tls {
                cert: "/tmp/test-cert.pem".to_string(),
                key: "/tmp/test-key.pem".to_string(),
                ..Tls::default()
            },
            ..Listen::default()
        },
        listeners: Vec::new(),
        upstream: upstreams,
        load_balancing: None,
        upstream_tls: Default::default(),
        secrets: Default::default(),
        log: Default::default(),
        performance: Default::default(),
        observability: Default::default(),
        resilience: Default::default(),
        security: Default::default(),
    })
    .expect("runtime config");

    UpstreamPool::from_runtime_upstream(runtime.upstreams.get("api").expect("upstream"))
        .expect("pool")
}

fn healthy_backend_indices(pool: &UpstreamPool) -> Vec<usize> {
    pool.healthy_backend_indices_iter().collect()
}

fn assert_status_classification(status: StatusCode, expected: HealthClassification) {
    let actual = outcome_from_status(status);
    assert!(
        std::mem::discriminant(&actual) == std::mem::discriminant(&expected),
        "status {status} should classify as {expected:?}, got {actual:?}"
    );
}

fn assert_backend_is_healthy(pool: &UpstreamPool, backend_index: usize, context: &str) {
    assert!(
        healthy_backend_indices(pool).contains(&backend_index),
        "{context}: backend {backend_index} should be healthy"
    );
}

fn assert_backend_is_unhealthy(pool: &UpstreamPool, backend_index: usize, context: &str) {
    assert!(
        !healthy_backend_indices(pool).contains(&backend_index),
        "{context}: backend {backend_index} should be unhealthy"
    );
}

fn assert_becomes_unhealthy_after_threshold(pool: &mut UpstreamPool, backend_index: usize) {
    for attempt in 1..=3 {
        let transition = pool.mark_backend_failure_from_active_check(backend_index);
        if attempt < 3 {
            assert!(
                transition.is_none(),
                "failure attempt {attempt} should not transition before the threshold"
            );
        } else {
            assert!(
                matches!(transition, Some(HealthTransition::BecameUnhealthy)),
                "failure attempt {attempt} should transition the backend to unhealthy"
            );
        }
    }
}

#[test]
fn client_error_statuses_classify_as_neutral_without_changing_health() {
    let backend_index = 0;
    let test_cases = vec![
        StatusCode::BAD_REQUEST,
        StatusCode::FORBIDDEN,
        StatusCode::NOT_FOUND,
        StatusCode::METHOD_NOT_ALLOWED,
        StatusCode::CONFLICT,
        StatusCode::UNPROCESSABLE_ENTITY,
        StatusCode::TOO_MANY_REQUESTS,
    ];

    for status in test_cases {
        let mut pool = create_test_upstream_pool();
        assert_status_classification(status, HealthClassification::Neutral);
        let transition = pool.mark_backend_healthy(backend_index);
        assert!(
            transition.is_none(),
            "neutral classifications must not produce a health transition on a healthy backend"
        );
        assert_backend_is_healthy(&pool, backend_index, "4xx classifications");
    }
}

#[test]
fn server_error_statuses_classify_as_failures_and_trip_the_unhealthy_threshold() {
    let backend_index = 0;
    let test_cases = vec![
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::BAD_GATEWAY,
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::GATEWAY_TIMEOUT,
    ];

    for status in test_cases {
        let mut pool = create_test_upstream_pool();
        assert_status_classification(status, HealthClassification::Failure);
        assert_becomes_unhealthy_after_threshold(&mut pool, backend_index);
        assert_backend_is_unhealthy(&pool, backend_index, "5xx classifications");
    }
}

#[test]
fn success_statuses_keep_healthy_backends_healthy() {
    let test_cases = vec![
        StatusCode::OK,
        StatusCode::CREATED,
        StatusCode::ACCEPTED,
        StatusCode::NO_CONTENT,
        StatusCode::MOVED_PERMANENTLY,
        StatusCode::FOUND,
        StatusCode::NOT_MODIFIED,
    ];

    for status in test_cases {
        let mut pool = create_test_upstream_pool();
        let backend_index = 0;

        assert_status_classification(status, HealthClassification::Success);
        let transition = pool.mark_backend_healthy(backend_index);
        assert!(
            transition.is_none(),
            "success classifications must not transition an already healthy backend"
        );
        assert_backend_is_healthy(&pool, backend_index, "2xx/3xx classifications");
    }
}

#[test]
fn successive_successes_recover_an_unhealthy_backend() {
    let mut pool = create_test_upstream_pool();
    let backend_index = 0;

    assert_becomes_unhealthy_after_threshold(&mut pool, backend_index);
    assert_backend_is_unhealthy(&pool, backend_index, "after failure threshold");

    for i in 0..2 {
        let transition = pool.mark_backend_healthy(backend_index);
        if i < 1 {
            assert!(
                transition.is_none(),
                "the first success should not recover the backend before the success threshold"
            );
        }
    }
}

#[test]
fn bridge_errors_leave_backend_health_unchanged() {
    let mut pool = create_test_upstream_pool();
    let backend_index = 0;

    assert_backend_is_healthy(&pool, backend_index, "fresh backend");

    let transition = pool.mark_backend_healthy(backend_index);
    assert!(
        transition.is_none(),
        "bridge-local failures must not change backend health"
    );
    assert_backend_is_healthy(&pool, backend_index, "bridge errors");
}

#[test]
fn transport_errors_mark_backends_unhealthy_after_the_failure_threshold() {
    let mut pool = create_test_upstream_pool();
    let backend_index = 0;

    assert_becomes_unhealthy_after_threshold(&mut pool, backend_index);
    assert_backend_is_unhealthy(&pool, backend_index, "transport failures");
}

#[test]
fn timeout_failures_mark_backends_unhealthy_after_the_failure_threshold() {
    let mut pool = create_test_upstream_pool();
    let backend_index = 0;

    assert_becomes_unhealthy_after_threshold(&mut pool, backend_index);
    assert_backend_is_unhealthy(&pool, backend_index, "timeout failures");
}

#[test]
fn tls_failures_leave_backend_health_unchanged() {
    let mut pool = create_test_upstream_pool();
    let backend_index = 0;

    assert_backend_is_healthy(&pool, backend_index, "fresh backend");

    let transition = pool.mark_backend_healthy(backend_index);
    assert!(
        transition.is_none(),
        "tls-local failures must not change backend health"
    );
    assert_backend_is_healthy(&pool, backend_index, "tls failures");
}

#[test]
fn mixed_success_failure_and_neutral_observations_preserve_the_expected_health_state() {
    let mut pool = create_test_upstream_pool();
    let backend_index = 0;

    let transition = pool.mark_backend_healthy(backend_index);
    assert!(
        transition.is_none(),
        "a success response must not transition an already healthy backend"
    );

    assert_becomes_unhealthy_after_threshold(&mut pool, backend_index);

    assert_status_classification(StatusCode::BAD_REQUEST, HealthClassification::Neutral);
    assert_backend_is_unhealthy(&pool, backend_index, "mixed observations");
}

#[test]
fn status_code_families_map_to_the_expected_health_classifications() {
    for code in [200, 201, 202, 203, 204, 206] {
        let status = StatusCode::from_u16(code).unwrap();
        assert_status_classification(status, HealthClassification::Success);
    }

    for code in [300, 301, 302, 303, 304, 307, 308] {
        let status = StatusCode::from_u16(code).unwrap();
        assert_status_classification(status, HealthClassification::Success);
    }

    for code in [400, 401, 403, 404, 405, 409, 422, 429] {
        let status = StatusCode::from_u16(code).unwrap();
        assert_status_classification(status, HealthClassification::Neutral);
    }

    for code in [500, 501, 502, 503, 504, 505] {
        let status = StatusCode::from_u16(code).unwrap();
        assert_status_classification(status, HealthClassification::Failure);
    }
}
