//! Reload-boundary contracts for runtime lowering.

use std::time::Duration;

use spooky_config::{
    config::{Listen, LogFormat, Tls},
    runtime::RuntimeListenerSource,
};

use crate::common::{primary_listener_runtime_config, runtime_config, sample_config};

#[test]
fn runtime_config_keeps_generation_policies_normalized_and_listener_inputs_raw() {
    let mut config = sample_config();
    config.performance.backend_timeout_ms = 2_400;
    config.performance.backend_connect_timeout_ms = 600;
    config.performance.backend_body_idle_timeout_ms = 3_000;
    config.performance.backend_body_total_timeout_ms = 4_000;
    config.performance.backend_total_request_timeout_ms = 5_000;
    config.performance.worker_threads = 7;
    config.performance.packet_shards_per_worker = 3;
    config.performance.reuseport = true;
    config.performance.quic_initial_max_data = 2_000_000;
    config.observability.tracing.enabled = true;
    config.observability.tracing.service_name = "spooky-edge-prod".to_string();
    config.observability.tracing.sample_ratio = 0.42;
    config.observability.tracing.otlp_endpoint =
        Some("http://otel-collector.internal:4317".to_string());

    let runtime = runtime_config(&config);
    let policies = runtime.policies();
    let listener = primary_listener_runtime_config(&runtime);

    assert_eq!(
        policies.timeouts.backend_request,
        Duration::from_millis(2_400)
    );
    assert_eq!(
        policies.timeouts.backend_connect,
        Duration::from_millis(600)
    );
    assert_eq!(policies.transport.quic_initial_max_data, 2_000_000);
    assert_eq!(listener.policies.timeouts, policies.timeouts);
    assert_eq!(listener.policies.transport, policies.transport);

    assert_eq!(listener.performance.backend_timeout_ms, 2_400);
    assert_eq!(listener.performance.backend_connect_timeout_ms, 600);
    assert_eq!(listener.performance.worker_threads, 7);
    assert_eq!(listener.performance.packet_shards_per_worker, 3);
    assert!(listener.performance.reuseport);

    assert!(listener.observability.tracing.enabled);
    assert_eq!(
        listener.observability.tracing.service_name,
        "spooky-edge-prod"
    );
    assert_eq!(listener.observability.tracing.sample_ratio, 0.42);
    assert_eq!(
        listener.observability.tracing.otlp_endpoint.as_deref(),
        Some("http://otel-collector.internal:4317")
    );
}

#[test]
fn runtime_config_keeps_explicit_listener_topology_and_tls_identities_listener_owned() {
    let mut config = sample_config();
    config.performance.backend_timeout_ms = 1_750;
    config.performance.backend_connect_timeout_ms = 500;
    config.observability.control_api.enabled = true;
    config.observability.control_api.address = "127.0.0.1".to_string();
    config.observability.control_api.port = 9891;
    config.observability.metrics.enabled = true;
    config.observability.metrics.address = "127.0.0.1".to_string();
    config.observability.metrics.port = 9890;
    config.listeners = vec![
        Listen {
            protocol: "http3".to_string(),
            port: 8443,
            address: "127.0.0.1".to_string(),
            tls: Tls {
                cert: "/tmp/tls/edge-a.pem".to_string(),
                key: "/tmp/tls/edge-a.key".to_string(),
                certificates: Vec::new(),
                client_auth: Default::default(),
            },
        },
        Listen {
            protocol: "http3".to_string(),
            port: 9443,
            address: "127.0.0.2".to_string(),
            tls: Tls {
                cert: "/tmp/tls/edge-b.pem".to_string(),
                key: "/tmp/tls/edge-b.key".to_string(),
                certificates: Vec::new(),
                client_auth: Default::default(),
            },
        },
    ];

    let runtime = runtime_config(&config);
    let listeners = runtime.listener_runtime_configs();

    assert_eq!(listeners.len(), 2);
    assert_eq!(
        listeners[0].listen.source,
        RuntimeListenerSource::ExplicitListeners
    );
    assert_eq!(
        listeners[1].listen.source,
        RuntimeListenerSource::ExplicitListeners
    );
    assert_eq!(listeners[0].listen.listen.address, "127.0.0.1");
    assert_eq!(listeners[0].listen.listen.port, 8443);
    assert_eq!(listeners[1].listen.listen.address, "127.0.0.2");
    assert_eq!(listeners[1].listen.listen.port, 9443);
    assert_eq!(
        listeners[0].listen.tls.default_identity.cert_path,
        "/tmp/tls/edge-a.pem"
    );
    assert_eq!(
        listeners[1].listen.tls.default_identity.cert_path,
        "/tmp/tls/edge-b.pem"
    );

    assert_eq!(
        listeners[0].policies.timeouts,
        listeners[1].policies.timeouts
    );
    assert_eq!(
        listeners[0].policies.transport,
        listeners[1].policies.transport
    );

    assert!(listeners[0].observability.control_api.enabled);
    assert_eq!(listeners[0].observability.control_api.port, 9891);
    assert!(listeners[1].observability.metrics.enabled);
    assert_eq!(listeners[1].observability.metrics.port, 9890);
}

#[test]
fn runtime_config_excludes_log_sink_shape_from_generation_owned_runtime_state() {
    let mut plain = sample_config();
    plain.log.level = "warn".to_string();
    plain.log.file.enabled = false;
    plain.log.format = LogFormat::Plain;

    let mut json = plain.clone();
    json.log.level = "debug".to_string();
    json.log.file.enabled = true;
    json.log.file.path = "/var/log/spooky/edge.json".to_string();
    json.log.format = LogFormat::Json;

    let plain_runtime = runtime_config(&plain);
    let json_runtime = runtime_config(&json);

    assert_eq!(
        plain_runtime.policies().timeouts,
        json_runtime.policies().timeouts
    );
    assert_eq!(
        plain_runtime.policies().transport,
        json_runtime.policies().transport
    );
    assert_eq!(plain_runtime.listeners.len(), json_runtime.listeners.len());
    assert_eq!(
        plain_runtime.listeners[0].listen.address,
        json_runtime.listeners[0].listen.address
    );
    assert_eq!(
        plain_runtime.listeners[0].listen.port,
        json_runtime.listeners[0].listen.port
    );

    let plain_listener = primary_listener_runtime_config(&plain_runtime);
    let json_listener = primary_listener_runtime_config(&json_runtime);
    assert_eq!(
        plain_listener.policies.timeouts,
        json_listener.policies.timeouts
    );
    assert_eq!(
        plain_listener.policies.transport,
        json_listener.policies.transport
    );
}
