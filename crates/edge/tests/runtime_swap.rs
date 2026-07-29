use std::collections::HashMap;

use serial_test::serial;
use spooky_config::config::{Backend, LoadBalancing, RouteMatch, Upstream};

mod support;

use support::{
    net::local_listener_bind_available,
    request_path::H3RequestSpec,
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

fn lifecycle_backend_addresses(snapshot: &serde_json::Value) -> Vec<String> {
    snapshot["backends"]["lifecycle"]
        .as_array()
        .expect("backend lifecycle array")
        .iter()
        .map(|backend| {
            backend["backend"]
                .as_str()
                .expect("backend lifecycle address")
                .to_string()
        })
        .collect()
}

#[test]
#[serial]
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

#[test]
#[serial]
fn runtime_reload_swaps_generation_owned_backend_targets_without_changing_listener_identity() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = RuntimeSwapHarness::new();
    let old_backend = harness.start_h1_static_backend(b"backend-old");
    let new_backend = harness.start_h1_static_backend(b"backend-new");
    let config = harness.make_config(HashMap::from([(
        "api".to_string(),
        single_backend_upstream(old_backend),
    )]));

    harness
        .start_listener(config)
        .expect("start runtime swap listener");

    let before = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request before reload");
    before.assert_status(200);
    before.assert_body_bytes(b"backend-old");

    let startup_snapshot = harness.runtime_snapshot().expect("startup runtime snapshot");
    assert_eq!(startup_snapshot["runtime"]["generation"], 0);
    let startup_tls = startup_snapshot["tls"]["listeners"]
        .as_object()
        .expect("startup tls listener object")
        .clone();
    assert!(
        !startup_tls.is_empty(),
        "startup runtime snapshot should expose listener TLS inventory"
    );

    harness
        .rewrite_config(|config| {
            let upstream = config
                .upstream
                .get_mut("api")
                .expect("runtime swap test upstream");
            upstream.backends[0].address = format!("http://{new_backend}");
        })
        .expect("rewrite backend target");

    let reload = harness
        .trigger_runtime_reload()
        .expect("trigger runtime reload");
    assert_eq!(reload["reloaded"], true);
    assert_eq!(reload["generation"], 1);

    let after = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request after reload");
    after.assert_status(200);
    after.assert_body_bytes(b"backend-new");

    let reloaded_snapshot = harness.runtime_snapshot().expect("reloaded runtime snapshot");
    assert_eq!(reloaded_snapshot["runtime"]["generation"], 1);
    assert_eq!(
        startup_snapshot["runtime"]["generation"], 0,
        "stale runtime snapshot values should remain stable after bundle replacement"
    );
    assert_eq!(
        startup_snapshot["tls"]["listeners"]
            .as_object()
            .expect("stale startup tls listeners"),
        &startup_tls,
        "stale runtime snapshot should keep its original listener identity view"
    );
    assert_eq!(
        reloaded_snapshot["tls"]["listeners"]
            .as_object()
            .expect("reloaded tls listener object"),
        &startup_tls,
        "startup-owned listener TLS identity should not change across generation-only reloads"
    );
}

#[test]
#[serial]
fn runtime_reload_rejects_listener_bind_change_and_keeps_active_generation_live() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = RuntimeSwapHarness::new();
    let backend_addr = harness.start_h1_static_backend(b"bind-stable");
    let config = harness.make_config(HashMap::from([(
        "api".to_string(),
        single_backend_upstream(backend_addr),
    )]));

    harness
        .start_listener(config)
        .expect("start runtime swap listener");

    let before = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request before bind-change reload");
    before.assert_status(200);
    before.assert_body_bytes(b"bind-stable");

    let startup_snapshot = harness.runtime_snapshot().expect("startup runtime snapshot");
    assert_eq!(startup_snapshot["runtime"]["generation"], 0);

    harness
        .rewrite_config(|config| {
            config.listen.port = config.listen.port.saturating_add(1);
        })
        .expect("rewrite listener bind");

    let rejection = harness
        .trigger_runtime_reload_expect(http::StatusCode::CONFLICT)
        .expect("reload rejection");
    assert_eq!(rejection["reloaded"], false);
    let error = rejection["error"]
        .as_str()
        .expect("reload rejection error string");
    assert!(
        error.contains("restart required"),
        "listener bind change should be restart-required, got: {error}"
    );
    assert!(
        error.contains("listener"),
        "listener bind rejection should mention the listener, got: {error}"
    );

    let after_snapshot = harness.runtime_snapshot().expect("post-rejection runtime snapshot");
    assert_eq!(
        after_snapshot["runtime"]["generation"], 0,
        "startup-owned rejection must leave the active generation unchanged"
    );
    assert_eq!(
        after_snapshot["runtime"]["config_path"],
        startup_snapshot["runtime"]["config_path"],
        "config path should remain the same for a rejected same-path reload attempt"
    );

    let after = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request after bind-change rejection");
    after.assert_status(200);
    after.assert_body_bytes(b"bind-stable");
}

#[test]
#[serial]
fn runtime_reload_rejects_startup_owned_log_sink_change_and_keeps_request_behavior() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = RuntimeSwapHarness::new();
    let backend_addr = harness.start_h1_static_backend(b"log-sink-stable");
    let config = harness.make_config(HashMap::from([(
        "api".to_string(),
        single_backend_upstream(backend_addr),
    )]));

    harness
        .start_listener(config)
        .expect("start runtime swap listener");

    let before = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request before log-sink reload");
    before.assert_status(200);
    before.assert_body_bytes(b"log-sink-stable");

    let startup_snapshot = harness.runtime_snapshot().expect("startup runtime snapshot");
    assert_eq!(startup_snapshot["runtime"]["generation"], 0);

    harness
        .rewrite_config(|config| {
            config.log.file.enabled = true;
            config.log.file.path = "/tmp/spooky-runtime-swap.log".to_string();
        })
        .expect("rewrite log sink shape");

    let rejection = harness
        .trigger_runtime_reload_expect(http::StatusCode::CONFLICT)
        .expect("reload rejection");
    assert_eq!(rejection["reloaded"], false);
    let error = rejection["error"]
        .as_str()
        .expect("reload rejection error string");
    assert!(
        error.contains("restart required"),
        "log sink shape change should be restart-required, got: {error}"
    );
    assert!(
        error.contains("log.file.enabled") || error.contains("log.file.path"),
        "log sink rejection should point at startup-owned log file fields, got: {error}"
    );

    let after_snapshot = harness.runtime_snapshot().expect("post-rejection runtime snapshot");
    assert_eq!(
        after_snapshot["runtime"]["generation"], 0,
        "startup-owned log sink rejection must keep the active generation unchanged"
    );

    let after = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request after log-sink rejection");
    after.assert_status(200);
    after.assert_body_bytes(b"log-sink-stable");
}

#[test]
#[serial]
fn control_api_runtime_snapshot_tracks_active_generation_listener_labels_and_backend_inventory() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = RuntimeSwapHarness::new();
    let old_backend = harness.start_h1_static_backend(b"snapshot-old");
    let new_backend = harness.start_h1_static_backend(b"snapshot-new");
    let config = harness.make_config(HashMap::from([(
        "api".to_string(),
        single_backend_upstream(old_backend),
    )]));
    let listener_label = format!("127.0.0.1:{}", config.listen.port);

    harness
        .start_listener(config)
        .expect("start runtime swap listener");

    let startup_snapshot = harness.runtime_snapshot().expect("startup runtime snapshot");
    assert_eq!(startup_snapshot["runtime"]["generation"], 0);
    let startup_listeners = startup_snapshot["tls"]["listeners"]
        .as_object()
        .expect("startup tls listeners");
    assert!(
        startup_listeners.contains_key(&listener_label),
        "runtime snapshot should render the current active listener label"
    );
    assert_eq!(
        lifecycle_backend_addresses(&startup_snapshot),
        vec![format!("http://{old_backend}")],
        "startup runtime snapshot should expose only the active generation backend inventory"
    );

    harness
        .rewrite_config(|config| {
            let upstream = config
                .upstream
                .get_mut("api")
                .expect("runtime swap test upstream");
            upstream.backends[0].address = format!("http://{new_backend}");
            config.observability.control_api.runtime_path = "/runtime-live".to_string();
        })
        .expect("rewrite active generation fields");

    let reload = harness
        .trigger_runtime_reload()
        .expect("trigger runtime reload");
    assert_eq!(reload["reloaded"], true);
    assert_eq!(reload["generation"], 1);

    let live_snapshot = harness.runtime_snapshot().expect("live runtime snapshot");
    assert_eq!(live_snapshot["runtime"]["generation"], 1);
    let live_listeners = live_snapshot["tls"]["listeners"]
        .as_object()
        .expect("live tls listeners");
    assert!(
        live_listeners.contains_key(&listener_label),
        "runtime snapshot should keep rendering the current live listener label after swap"
    );
    assert_eq!(
        live_listeners.len(),
        1,
        "runtime snapshot should render only the active generation listener inventory"
    );

    let live_backends = lifecycle_backend_addresses(&live_snapshot);
    assert_eq!(
        live_backends,
        vec![format!("http://{new_backend}")],
        "runtime snapshot should expose backend lifecycle inventory from the active generation only"
    );
    assert!(
        !live_backends.iter().any(|backend| backend == &format!("http://{old_backend}")),
        "runtime snapshot must not leak stale-generation backend inventory"
    );
}

#[test]
#[serial]
fn metrics_endpoint_tracks_active_generation_path_and_metric_surface_after_reload() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = RuntimeSwapHarness::new();
    let backend_addr = harness.start_h1_static_backend(b"metrics-live");
    let config = harness.make_config(HashMap::from([(
        "api".to_string(),
        single_backend_upstream(backend_addr),
    )]));

    harness
        .start_listener(config)
        .expect("start runtime swap listener");

    let startup_metrics = harness.metrics_text().expect("startup metrics text");
    assert!(
        startup_metrics.contains("spooky_route_requests_total{route=\"api\"} 0\n"),
        "startup metrics should render the startup generation route label"
    );
    let old_path = "/metrics".to_string();

    harness
        .rewrite_config(|config| {
            let upstream = config.upstream.remove("api").expect("startup upstream");
            config.upstream.insert("api-reloaded".to_string(), upstream);
            config.observability.metrics.path = "/metrics-live".to_string();
        })
        .expect("rewrite metrics path and route labels");

    let reload = harness
        .trigger_runtime_reload()
        .expect("trigger runtime reload");
    assert_eq!(reload["reloaded"], true);
    assert_eq!(reload["generation"], 1);

    let live_metrics = harness.metrics_text().expect("live metrics text");
    assert!(
        live_metrics.contains("spooky_route_requests_total{route=\"api-reloaded\"} 0\n"),
        "reloaded metrics should render the active generation route label"
    );
    assert!(
        !live_metrics.contains("spooky_route_requests_total{route=\"api\"}"),
        "reloaded metrics must not fall back to the startup metrics surface after reload"
    );

    let old_path_status = harness
        .metrics_status_at(&old_path)
        .expect("old metrics path status");
    assert_eq!(
        old_path_status,
        http::StatusCode::NOT_FOUND,
        "old metrics path should no longer be treated as the active metrics endpoint after reload"
    );
}
