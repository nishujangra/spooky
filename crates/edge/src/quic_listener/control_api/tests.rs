use std::{collections::HashMap, ffi::OsString, path::Path, sync::Arc};

use http_body_util::BodyExt;
use log::LevelFilter;
use spooky_config::{
    config::{
        Backend, ClientAuth, Config as SpookyConfigConfig, Listen, LoadBalancing, Log, LogFormat,
        Observability, Performance, Resilience, RouteMatch, Security, Tls, Upstream, UpstreamTls,
    },
    runtime::RuntimeConfig,
};
use tempfile::tempdir;

use super::{state::ControlApiState, *};

/// Render the typed startup-owned compatibility result into the flat list of
/// operator strings the assertions below were written against.
fn startup_owned_issue_strings(
    current: &RuntimeBundle,
    next: &RuntimeBundle,
) -> Vec<String> {
    match QUICListener::validate_startup_owned_reload_compatibility(current, next) {
        Ok(()) => Vec::new(),
        Err(rejections) => rejections.iter().map(|r| r.to_string()).collect(),
    }
}

fn write_test_cert_for_name(dir: &Path, cert_name: &str, dns_name: &str) -> (String, String) {
    use rcgen::{Certificate, CertificateParams, SanType};

    let mut params = CertificateParams::new(vec![dns_name.to_string()]);
    params
        .subject_alt_names
        .push(SanType::DnsName(dns_name.to_string()));
    let cert = Certificate::from_params(params).expect("failed to build cert");

    let cert_path = dir.join(format!("{cert_name}.pem"));
    let key_path = dir.join(format!("{cert_name}.key.pem"));
    std::fs::write(&cert_path, cert.serialize_pem().expect("serialize cert")).expect("write cert");
    std::fs::write(&key_path, cert.serialize_private_key_pem()).expect("write key");
    (
        cert_path.to_string_lossy().to_string(),
        key_path.to_string_lossy().to_string(),
    )
}

fn test_config(cert: String, key: String) -> SpookyConfigConfig {
    let mut upstreams = HashMap::new();
    upstreams.insert(
        "api".to_string(),
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
                id: "b1".to_string(),
                address: "http://127.0.0.1:7001".to_string(),
                weight: 1,
                health_check: None,
            }],
        },
    );

    SpookyConfigConfig {
        version: 1,
        listen: Listen {
            protocol: "http3".to_string(),
            port: 9889,
            address: "127.0.0.1".to_string(),
            tls: Tls {
                cert,
                key,
                certificates: vec![],
                client_auth: ClientAuth::default(),
            },
        },
        listeners: vec![],
        upstream: upstreams,
        load_balancing: Some(LoadBalancing {
            lb_type: "round-robin".to_string(),
            key: None,
        }),
        upstream_tls: UpstreamTls::default(),
        log: Log::default(),
        performance: Performance::default(),
        observability: Observability::default(),
        resilience: Resilience::default(),
        security: Security::default(),
    }
}

fn runtime_bundle_from_config(config_path: &str, config: &SpookyConfigConfig) -> RuntimeBundle {
    let runtime_config = RuntimeConfig::from_config(config).expect("runtime config");
    QUICListener::build_runtime_bundle(config_path.to_string(), config.log.clone(), &runtime_config)
        .expect("runtime bundle")
}

fn control_api_state_with_runtime_bundle(
    startup: &SpookyConfigConfig,
    reloaded: &SpookyConfigConfig,
) -> ControlApiState {
    let startup_bundle = runtime_bundle_from_config("startup.yaml", startup);
    let reloaded_bundle = runtime_bundle_from_config("reloaded.yaml", reloaded);
    let runtime_ctx =
        crate::quic_listener::runtime_state::ControlPlaneRuntimeCtx::from_runtime_sources(
            &startup_bundle.runtime_config,
            startup_bundle.shared_state.as_ref(),
            Some(Arc::new(RuntimeBundleHandle::new(reloaded_bundle))),
        );

    ControlApiState::new(runtime_ctx)
}

fn runtime_bundle_control_api_state(
    bundle: RuntimeBundle,
) -> (ControlApiState, Arc<RuntimeBundleHandle>) {
    let runtime_handle = Arc::new(RuntimeBundleHandle::new(bundle.clone()));
    let runtime_ctx =
        crate::quic_listener::runtime_state::ControlPlaneRuntimeCtx::from_runtime_sources(
            &bundle.runtime_config,
            bundle.shared_state.as_ref(),
            Some(Arc::clone(&runtime_handle)),
        );
    let state = ControlApiState::new(runtime_ctx);
    (state, runtime_handle)
}

#[test]
fn watchdog_restart_env_keeps_path_when_present() {
    let env = crate::watchdog::service::watchdog_restart_env(
        Some(OsString::from("/usr/bin:/bin")),
        "timeout_spike",
    );
    let map: HashMap<OsString, OsString> = env.into_iter().collect();

    assert_eq!(
        map.get(&OsString::from("PATH")),
        Some(&OsString::from("/usr/bin:/bin"))
    );
    assert_eq!(
        map.get(&OsString::from("SPOOKY_WATCHDOG_REASON")),
        Some(&OsString::from("timeout_spike"))
    );
}

#[test]
fn watchdog_restart_env_omits_path_when_missing() {
    let env = crate::watchdog::service::watchdog_restart_env(None, "poll_stall");
    let map: HashMap<OsString, OsString> = env.into_iter().collect();

    assert!(!map.contains_key(&OsString::from("PATH")));
    assert_eq!(
        map.get(&OsString::from("SPOOKY_WATCHDOG_REASON")),
        Some(&OsString::from("poll_stall"))
    );
}

#[test]
fn bearer_authorization_scheme_is_case_insensitive() {
    assert_eq!(
        QUICListener::bearer_token_from_authorization_header("Bearer token-1"),
        Some("token-1")
    );
    assert_eq!(
        QUICListener::bearer_token_from_authorization_header("bearer token-2"),
        Some("token-2")
    );
    assert_eq!(
        QUICListener::bearer_token_from_authorization_header("BEARER token-3"),
        Some("token-3")
    );
}

#[test]
fn bearer_authorization_rejects_malformed_headers() {
    assert_eq!(
        QUICListener::bearer_token_from_authorization_header("Basic abc"),
        None
    );
    assert_eq!(
        QUICListener::bearer_token_from_authorization_header("Bearer"),
        None
    );
    assert_eq!(
        QUICListener::bearer_token_from_authorization_header("Bearer   "),
        None
    );
}

#[test]
fn control_api_state_prefers_reloaded_paths_and_auth_token() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let mut startup = test_config(cert.clone(), key.clone());
    startup.observability.control_api.enabled = true;
    startup.observability.control_api.health_path = "/health-old".to_string();
    startup.observability.control_api.runtime_path = "/runtime-old".to_string();
    startup.observability.control_api.auth_token = Some("old-token".to_string());

    let mut reloaded = startup.clone();
    reloaded.observability.control_api.health_path = "/health-new".to_string();
    reloaded.observability.control_api.runtime_path = "/runtime-new".to_string();
    reloaded.observability.control_api.auth_token = Some("new-token".to_string());

    let state = control_api_state_with_runtime_bundle(&startup, &reloaded);
    let paths = state.current_paths();

    assert_eq!(paths.health_path, "/health-new");
    assert_eq!(paths.runtime_path, "/runtime-new");
    assert_eq!(
        state.current_control_api().auth_token.as_deref(),
        Some("new-token")
    );
}

#[test]
fn control_api_state_uses_live_primary_listener_label_after_runtime_swap() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let startup = test_config(cert.clone(), key.clone());

    let mut reloaded = startup.clone();
    reloaded.listeners = vec![
        Listen {
            protocol: "http3".to_string(),
            port: 9890,
            address: "127.0.0.1".to_string(),
            tls: Tls {
                cert: cert.clone(),
                key: key.clone(),
                certificates: vec![],
                client_auth: ClientAuth::default(),
            },
        },
        startup.listen.clone(),
    ];

    let state = control_api_state_with_runtime_bundle(&startup, &reloaded);

    assert_eq!(
        state.current_primary_listener_label().as_deref(),
        Some("127.0.0.1:9890")
    );
}

#[test]
fn control_api_state_sees_the_active_runtime_generation_after_bundle_replace() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let mut startup = test_config(cert.clone(), key.clone());
    startup.observability.control_api.enabled = true;
    startup.observability.control_api.runtime_path = "/runtime-startup".to_string();

    let startup_bundle = runtime_bundle_from_config("startup.yaml", &startup);
    let (state, runtime_handle) = runtime_bundle_control_api_state(startup_bundle);

    let current = state.current_generation().expect("current generation");
    assert_eq!(current.generation(), 0);
    assert_eq!(
        state.current_paths().runtime_path,
        "/runtime-startup".to_string()
    );

    let mut reloaded = startup.clone();
    reloaded.observability.control_api.runtime_path = "/runtime-reloaded".to_string();
    reloaded.observability.metrics.path = "/metrics-reloaded".to_string();

    let mut reloaded_bundle = runtime_bundle_from_config("reloaded.yaml", &reloaded);
    reloaded_bundle.generation = 1;
    runtime_handle
        .replace(reloaded_bundle)
        .expect("replace runtime bundle");

    let current = state.current_generation().expect("reloaded generation");
    assert_eq!(current.generation(), 1);
    assert_eq!(
        current.runtime_config().observability.metrics.path,
        "/metrics-reloaded"
    );
    assert_eq!(
        state.current_paths().runtime_path,
        "/runtime-reloaded".to_string()
    );
}

#[test]
fn current_recovers_from_poisoned_bundle_lock_without_panicking() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config = test_config(cert, key);
    let bundle = runtime_bundle_from_config("current.yaml", &config);
    let expected_generation = bundle.generation;

    let handle = RuntimeBundleHandle::new(bundle);
    handle.poison_for_test();

    // Phase 5: a poisoned bundle lock must not panic the hot read path; it
    // recovers the last consistently-published generation.
    let recovered = handle.current();
    assert_eq!(recovered.generation, expected_generation);
    // And it keeps working on subsequent reads.
    assert_eq!(handle.current_generation(), expected_generation);
}

#[test]
fn validate_control_api_reload_compatibility_allows_bind_change_when_socket_is_free() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let mut current = test_config(cert.clone(), key.clone());
    current.observability.control_api.enabled = true;
    current.observability.control_api.address = "127.0.0.1".to_string();
    current.observability.control_api.port = 9443;

    let mut next = current.clone();
    next.observability.control_api.port = 0;

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);
    let result = QUICListener::validate_control_api_reload_compatibility(&current_bundle, &next_bundle)
        .map(|rejection| rejection.to_string());
    if result
        .as_deref()
        .is_some_and(|err| err.contains("Operation not permitted"))
    {
        return;
    }
    assert!(
        result.is_none(),
        "expected compatible reload, got: {result:?}"
    );
}

#[test]
fn validate_metrics_reload_compatibility_allows_bind_change_when_socket_is_free() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let mut current = test_config(cert.clone(), key.clone());
    current.observability.metrics.enabled = true;
    current.observability.metrics.address = "127.0.0.1".to_string();
    current.observability.metrics.port = 9100;

    let mut next = current.clone();
    next.observability.metrics.port = 0;

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);
    let result = QUICListener::validate_metrics_reload_compatibility(&current_bundle, &next_bundle)
        .map(|rejection| rejection.to_string());
    if result
        .as_deref()
        .is_some_and(|err| err.contains("Operation not permitted"))
    {
        return;
    }
    assert!(
        result.is_none(),
        "expected compatible reload, got: {result:?}"
    );
}

#[test]
fn validate_startup_owned_reload_compatibility_allows_log_level_change() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert.clone(), key.clone());

    let mut next = current.clone();
    next.log.level = "debug".to_string();

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);
    let issues = startup_owned_issue_strings(&current_bundle, &next_bundle);

    assert!(
        issues.iter().all(|issue| !issue.contains("log.level")),
        "expected log.level to be live-reloadable, got: {issues:?}"
    );
}

#[test]
fn validate_startup_owned_reload_compatibility_rejects_log_format_change() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert.clone(), key.clone());

    let mut next = current.clone();
    next.log.format = LogFormat::Json;

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);
    let issues = startup_owned_issue_strings(&current_bundle, &next_bundle);

    assert!(
        issues
            .iter()
            .any(|issue| issue.contains("log.format") && issue.contains("restart required"))
    );
}

#[test]
fn validate_startup_owned_reload_compatibility_allows_worker_topology_change() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert.clone(), key.clone());

    let mut next = current.clone();
    next.performance.worker_threads = 4;
    next.performance.packet_shards_per_worker = 2;

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);
    let issues = startup_owned_issue_strings(&current_bundle, &next_bundle);

    assert!(issues.is_empty());
}

#[test]
fn validate_runtime_reload_compatibility_allows_listener_addition_when_binds_are_free() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert.clone(), key.clone());

    let mut next = current.clone();
    let mut extra_listener = next.listen.clone();
    extra_listener.port = 0;
    next.listeners = vec![next.listen.clone(), extra_listener];

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);

    let result = QUICListener::validate_runtime_reload_compatibility(&current_bundle, &next_bundle)
        .map(|rejection| rejection.to_string());
    if result
        .as_deref()
        .is_some_and(|err| err.contains("Operation not permitted"))
    {
        return;
    }
    assert!(
        result.is_none(),
        "expected compatible reload, got: {result:?}"
    );
}

#[test]
fn validate_runtime_reload_compatibility_rejects_listener_removal() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert.clone(), key.clone());

    let mut next = current.clone();
    next.listeners = vec![{
        let mut l = next.listen.clone();
        l.port = 9892;
        l
    }];

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);

    let err = QUICListener::validate_runtime_reload_compatibility(&current_bundle, &next_bundle)
        .map(|rejection| rejection.to_string());
    assert!(
        err.as_deref()
            .is_some_and(|e| e.contains("restart required")),
        "expected rejection, got: {:?}",
        err
    );
}

#[test]
fn validate_runtime_reload_compatibility_rejects_listener_bind_change() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert.clone(), key.clone());

    let mut next = current.clone();
    next.listen.port = 9893;

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);

    let err = QUICListener::validate_runtime_reload_compatibility(&current_bundle, &next_bundle)
        .map(|rejection| rejection.to_string());
    assert!(
        err.as_deref()
            .is_some_and(|e| e.contains("restart required")),
        "expected rejection, got: {:?}",
        err
    );
}

#[test]
fn validate_startup_owned_reload_compatibility_rejects_control_plane_thread_change() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert.clone(), key.clone());

    let mut next = current.clone();
    next.performance.control_plane_threads = 7;

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);
    let issues = startup_owned_issue_strings(&current_bundle, &next_bundle);

    assert!(
        issues
            .iter()
            .any(|issue| issue.contains("performance.control_plane_threads"))
    );
}

#[test]
fn apply_live_log_level_reload_updates_global_filter() {
    spooky_utils::logger::set_log_level("info").expect("set initial level");

    let changed = QUICListener::apply_live_log_level_reload("info", "haunt")
        .expect("apply live log level reload");
    assert!(changed);
    assert_eq!(log::max_level(), LevelFilter::Debug);

    let changed = QUICListener::apply_live_log_level_reload("haunt", "haunt")
        .expect("same-level reload should succeed");
    assert!(!changed);
    assert_eq!(log::max_level(), LevelFilter::Debug);
}

#[tokio::test]
async fn runtime_bundle_cert_reload_ignores_unrelated_config_drift_and_bundle_swap() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let mut live = test_config(cert.clone(), key.clone());
    live.observability.metrics.enabled = true;
    live.observability.metrics.path = "/metrics-live".to_string();

    let live_bundle = runtime_bundle_from_config("live.yaml", &live);
    let (state, runtime_handle) = runtime_bundle_control_api_state(live_bundle);

    let mut drifted = live.clone();
    drifted.observability.metrics.path = "/metrics-drifted".to_string();
    drifted.performance.control_plane_threads =
        live.performance.control_plane_threads.saturating_add(1);
    let drifted_bundle = runtime_bundle_from_config("drifted.yaml", &drifted);
    let current_runtime = runtime_handle.current_view();
    let full_reload_issues =
        startup_owned_issue_strings(current_runtime.bundle(), &drifted_bundle);
    assert!(
        full_reload_issues
            .iter()
            .any(|issue| issue.contains("performance.control_plane_threads")),
        "expected a full reload blocker from on-disk drift, got: {full_reload_issues:?}"
    );

    let generation_before = runtime_handle.current_generation();
    let live_runtime = runtime_handle.current_view();
    let primary_listener_label = state
        .current_primary_listener_label()
        .expect("primary listener label");
    let tls_generation_before = live_runtime
        .shared_services()
        .listener_tls_store
        .generation(&primary_listener_label)
        .unwrap_or(0);

    let response = QUICListener::reload_listener_certs(
        live_runtime.state().listener_runtime_configs.as_ref(),
        live_runtime.shared_services().listener_tls_store.as_ref(),
        live_runtime.shared_services().metrics.as_ref(),
    );
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect response body")
        .to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("response json");
    assert_eq!(payload["reloaded"], serde_json::Value::Bool(true));

    let current_runtime = runtime_handle.current_view();
    assert_eq!(current_runtime.generation(), generation_before);
    assert_eq!(
        current_runtime.runtime_config().observability.metrics.path,
        "/metrics-live"
    );
    assert!(
        current_runtime
            .shared_services()
            .listener_tls_store
            .generation(&primary_listener_label)
            .unwrap_or(0)
            > tls_generation_before,
        "expected cert reload to rotate the live listener TLS generation"
    );
}

#[tokio::test]
async fn reload_listener_certs_is_atomic_when_any_listener_reload_fails() {
    let dir = tempdir().expect("tempdir");
    let (cert1, key1) = write_test_cert_for_name(dir.path(), "server-one", "api.example.com");
    let (cert2, key2) = write_test_cert_for_name(dir.path(), "server-two", "admin.example.com");
    let mut config = test_config(cert1.clone(), key1.clone());
    config.listeners = vec![
        Listen {
            protocol: "http3".to_string(),
            port: 9889,
            address: "127.0.0.1".to_string(),
            tls: Tls {
                cert: cert1,
                key: key1,
                certificates: vec![],
                client_auth: ClientAuth::default(),
            },
        },
        Listen {
            protocol: "http3".to_string(),
            port: 9890,
            address: "127.0.0.1".to_string(),
            tls: Tls {
                cert: cert2.clone(),
                key: key2,
                certificates: vec![],
                client_auth: ClientAuth::default(),
            },
        },
    ];

    let bundle = runtime_bundle_from_config("current.yaml", &config);
    let generations_before = bundle
        .shared_state
        .shared_services()
        .listener_tls_store
        .generations();

    std::fs::write(&cert2, "not a valid certificate").expect("corrupt cert");

    let response = QUICListener::reload_listener_certs(
        bundle
            .shared_state
            .generation_state()
            .listener_runtime_configs
            .as_ref(),
        bundle
            .shared_state
            .shared_services()
            .listener_tls_store
            .as_ref(),
        bundle.shared_state.shared_services().metrics.as_ref(),
    );
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect response body")
        .to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("response json");
    assert_eq!(payload["reloaded"], serde_json::Value::Bool(false));

    assert_eq!(
        bundle
            .shared_state
            .shared_services()
            .listener_tls_store
            .generations(),
        generations_before
    );
}
