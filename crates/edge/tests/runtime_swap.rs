use std::collections::HashMap;

use spooky_config::config::{Backend, LoadBalancing, RouteMatch, Upstream};

mod support;

use support::{
    net::local_listener_bind_available,
    runtime_swap::RuntimeSwapHarness,
};

fn single_backend_upstream(backend_addr: std::net::SocketAddr) -> Upstream {
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
            path_prefix: Some("/".to_string()),
            ..Default::default()
        },
        backends: vec![Backend {
            id: "backend-a".to_string(),
            address: format!("http://{backend_addr}"),
            weight: 1,
            health_check: None,
        }],
    }
}

#[test]
fn runtime_swap_harness_exposes_control_api_metrics_and_reload_surface() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = RuntimeSwapHarness::new();
    let backend_addr = harness.start_h1_static_backend(b"ok");
    let config = harness.make_config(HashMap::from([(
        "api".to_string(),
        single_backend_upstream(backend_addr),
    )]));

    harness
        .start_listener(config)
        .expect("start runtime swap listener");

    let startup_snapshot = harness.runtime_snapshot().expect("runtime snapshot");
    assert_eq!(startup_snapshot["runtime"]["generation"], 0);
    assert_eq!(
        startup_snapshot["runtime"]["config_path"],
        harness.config_path().to_string_lossy().to_string()
    );

    let metrics = harness.metrics_text().expect("metrics text");
    assert!(
        metrics.contains("# HELP spooky_requests_total Total requests seen by spooky.\n"),
        "metrics endpoint should expose prometheus request totals"
    );

    harness
        .rewrite_config(|config| {
            config.log.level = "debug".to_string();
            config.performance.new_connections_burst = 2;
        })
        .expect("rewrite config");

    let reload = harness
        .trigger_runtime_reload()
        .expect("trigger runtime reload");
    assert_eq!(reload["reloaded"], true);
    assert_eq!(reload["generation"], 1);

    let reloaded_snapshot = harness.runtime_snapshot().expect("reloaded runtime snapshot");
    assert_eq!(reloaded_snapshot["runtime"]["generation"], 1);
    assert_eq!(
        reloaded_snapshot["runtime"]["config_path"],
        harness.config_path().to_string_lossy().to_string()
    );
}
