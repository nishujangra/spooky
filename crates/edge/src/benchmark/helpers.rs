use impulse_config::config::{Backend, HealthCheck, LoadBalancing, RouteMatch, Upstream};

pub fn default_health_check() -> HealthCheck {
    HealthCheck {
        path: "/health".to_string(),
        interval: 1_000,
        timeout_ms: 1_000,
        failure_threshold: 3,
        success_threshold: 2,
        cooldown_ms: 5_000,
    }
}

pub fn build_benchmark_upstream(host: Option<String>, path_prefix: String) -> Upstream {
    Upstream {
        load_balancing: LoadBalancing {
            lb_type: "round-robin".to_string(),
            key: None,
        },
        auth: Default::default(),
        host_policy: Default::default(),
        forwarded_headers: Default::default(),
        tls: None,
        route: RouteMatch {
            host,
            path_prefix: Some(path_prefix),
            method: None,
        },
        // Routing benchmark does not touch backend connectivity.
        backends: vec![Backend {
            id: "placeholder".to_string(),
            address: "127.0.0.1:1".to_string(),
            weight: 1,
            health_check: Some(default_health_check()),
        }],
    }
}
