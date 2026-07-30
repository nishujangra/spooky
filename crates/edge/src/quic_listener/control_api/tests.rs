use std::{collections::HashMap, ffi::OsString, path::Path, sync::Arc};

use ::http::{Method, Request, header};
use bytes::Bytes;
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
use crate::runtime::activation::{
    ActivationRequest, GenerationEventKind, GenerationOperation, GenerationStatus, PlanningPhase,
    PlanningPhaseStatus, RejectedChangeKind, ReloadCompatibilityClassification, ReloadConfigInput,
    ReloadDiffDisposition, RollbackRequest, RuntimeActivationService, RuntimeRejectionReason,
    plan_runtime_reload,
};

/// Render the typed startup-owned compatibility result into the flat list of
/// operator strings the assertions below were written against.
fn startup_owned_issue_strings(current: &RuntimeBundle, next: &RuntimeBundle) -> Vec<String> {
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

fn write_config_file(path: &Path, config: &SpookyConfigConfig) {
    let backend = config
        .upstream
        .get("api")
        .and_then(|upstream| upstream.backends.first())
        .expect("api upstream backend");
    let upstream = config.upstream.get("api").expect("api upstream");
    let yaml = format!(
        r#"version: {version}
listen:
  protocol: "{protocol}"
  address: "{address}"
  port: {port}
  tls:
    cert: "{cert}"
    key: "{key}"
upstream:
  api:
    load_balancing:
      type: "{lb_type}"
    route:
      path_prefix: "{path_prefix}"
    backends:
      - id: "{backend_id}"
        address: "{backend_address}"
        weight: {backend_weight}
performance:
  control_plane_threads: {control_plane_threads}
log:
  level: "{log_level}"
"#,
        version = config.version,
        protocol = config.listen.protocol,
        address = config.listen.address,
        port = config.listen.port,
        cert = config.listen.tls.cert,
        key = config.listen.tls.key,
        lb_type = upstream.load_balancing.lb_type,
        path_prefix = upstream.route.path_prefix.as_deref().unwrap_or("/"),
        backend_id = backend.id,
        backend_address = backend.address,
        backend_weight = backend.weight,
        control_plane_threads = config.performance.control_plane_threads,
        log_level = config.log.level,
    );
    std::fs::write(path, yaml).expect("write config file");
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

fn control_api_request(method: Method, path: &str, authorization: Option<&str>) -> Request<()> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    builder.body(()).expect("control api request")
}

async fn full_body_bytes(response: Response<http_body_util::Full<Bytes>>) -> Bytes {
    response
        .into_body()
        .collect()
        .await
        .expect("collect response body")
        .to_bytes()
}

async fn json_body(response: Response<http_body_util::Full<Bytes>>) -> serde_json::Value {
    serde_json::from_slice(&full_body_bytes(response).await).expect("json response body")
}

fn default_control_api_state() -> ControlApiState {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let mut startup = test_config(cert.clone(), key.clone());
    startup.observability.control_api.enabled = true;
    startup.observability.control_api.auth_token = Some("secret-token".to_string());
    control_api_state_with_runtime_bundle(&startup, &startup)
}

fn planner_request(expected_generation: u64) -> ActivationRequest {
    ActivationRequest {
        requested_by: Some("test".to_string()),
        trigger_source: Some("unit_test".to_string()),
        reason: Some("planner_contract".to_string()),
        expected_generation: Some(expected_generation),
        requested_at_ms: 1,
    }
}

fn rollback_request(target_generation: u64, expected_active_generation: u64) -> RollbackRequest {
    RollbackRequest {
        target_generation,
        requested_by: Some("test".to_string()),
        trigger_source: Some("unit_test".to_string()),
        reason: Some("rollback_contract".to_string()),
        expected_active_generation: Some(expected_active_generation),
        requested_at_ms: 2,
    }
}

fn assert_structured_resource_preflight_message(
    rejection: &crate::runtime::policy::TransitionRejection,
) {
    let field = rejection
        .field_path
        .as_deref()
        .expect("resource field path");
    let detail = rejection
        .requested_mode
        .as_deref()
        .expect("resource failure detail");
    assert_eq!(
        rejection.to_string(),
        format!(
            "runtime reload rejected: could not prepare {field}: {detail}; active runtime unchanged (no change applied)"
        )
    );
}

#[test]
fn staged_reload_planner_reports_reloadable_candidate_snapshot() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config = test_config(cert, key);
    let config_path = dir.path().join("runtime.yaml");
    write_config_file(&config_path, &config);

    let bundle = runtime_bundle_from_config(config_path.to_string_lossy().as_ref(), &config);
    let current = RuntimeBundleHandle::new(bundle).current_view();
    let plan = plan_runtime_reload(
        &current,
        planner_request(current.generation()),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );

    assert!(plan.can_activate());
    assert_eq!(
        plan.plan.compatibility,
        ReloadCompatibilityClassification::LiveReloadable
    );
    assert_eq!(
        plan.plan.phase_status(PlanningPhase::ReadConfig),
        Some(PlanningPhaseStatus::Accepted)
    );
    assert_eq!(
        plan.plan.phase_status(PlanningPhase::ValidateConfig),
        Some(PlanningPhaseStatus::Accepted)
    );
    assert_eq!(
        plan.plan.phase_status(PlanningPhase::NormalizeRuntime),
        Some(PlanningPhaseStatus::Accepted)
    );
    assert_eq!(
        plan.plan.phase_status(PlanningPhase::EvaluateCompatibility),
        Some(PlanningPhaseStatus::Accepted)
    );

    let snapshot = plan
        .plan
        .candidate_snapshot
        .as_ref()
        .expect("candidate snapshot");
    assert_eq!(snapshot.generation, current.generation() + 1);
    assert_eq!(
        snapshot.config_path,
        config_path.to_string_lossy().to_string()
    );
    assert_eq!(snapshot.upstream_count, 1);
    assert_eq!(snapshot.backend_count, 1);
    assert!(
        plan.plan
            .diff
            .entries
            .iter()
            .any(|entry| entry.domain == "observability_control_plane"
                && entry.disposition == ReloadDiffDisposition::NoOp),
        "expected identical config to produce a no-op observability/control-plane diff"
    );
    assert!(plan.plan.diff.reloadable_entries().is_empty());
    assert!(plan.plan.diff.rejected_startup_owned_entries().is_empty());
}

#[test]
fn staged_reload_planner_classifies_restart_required_changes_without_mutation() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config_path = dir.path().join("runtime.yaml");
    let current_config = test_config(cert.clone(), key.clone());
    write_config_file(&config_path, &current_config);

    let bundle =
        runtime_bundle_from_config(config_path.to_string_lossy().as_ref(), &current_config);
    let current = RuntimeBundleHandle::new(bundle).current_view();

    let mut next_config = test_config(cert, key);
    next_config.performance.control_plane_threads = 4;
    write_config_file(&config_path, &next_config);

    let plan = plan_runtime_reload(
        &current,
        planner_request(current.generation()),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );

    assert!(!plan.can_activate());
    assert_eq!(
        plan.plan.compatibility,
        ReloadCompatibilityClassification::RestartRequired
    );
    assert_eq!(
        plan.plan.phase_status(PlanningPhase::EvaluateCompatibility),
        Some(PlanningPhaseStatus::Rejected)
    );
    assert!(plan.plan.candidate_snapshot.is_some());
    assert!(plan.plan.rejection_summary.is_some());
    assert!(
        plan.plan.rejected_changes.iter().any(|rejection| {
            rejection.kind == RejectedChangeKind::RestartRequired
                && rejection.field_path.as_deref() == Some("performance.control_plane_threads")
        }),
        "expected a startup-owned restart-required rejection"
    );
    assert!(
        plan.plan
            .diff
            .entries
            .iter()
            .any(|entry| entry.domain == "observability_control_plane"
                && entry.disposition == ReloadDiffDisposition::RejectedStartupOwned
                && matches!(
                    entry.change,
                    crate::runtime::activation::ReloadChangeKind::Modified
                )),
        "expected control-plane startup-owned drift to be separated as rejected startup-owned"
    );
}

#[test]
fn staged_reload_planner_marks_log_level_change_as_reloadable_domain_diff() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config_path = dir.path().join("runtime.yaml");
    let current_config = test_config(cert.clone(), key.clone());
    write_config_file(&config_path, &current_config);

    let bundle =
        runtime_bundle_from_config(config_path.to_string_lossy().as_ref(), &current_config);
    let current = RuntimeBundleHandle::new(bundle).current_view();

    let mut next_config = test_config(cert, key);
    next_config.log.level = "debug".to_string();
    write_config_file(&config_path, &next_config);

    let plan = plan_runtime_reload(
        &current,
        planner_request(current.generation()),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );

    assert!(plan.can_activate());
    assert!(
        plan.plan
            .diff
            .entries
            .iter()
            .any(|entry| entry.domain == "observability_control_plane"
                && entry.disposition == ReloadDiffDisposition::Reloadable
                && matches!(
                    entry.change,
                    crate::runtime::activation::ReloadChangeKind::Modified
                )
                && entry.summary.contains("log(level=info")
                && entry.summary.contains("log(level=debug")),
        "expected log.level drift to show up as a reloadable observability/control-plane diff"
    );
    assert_eq!(plan.plan.diff.reloadable_entries().len(), 1);
}

#[test]
fn activation_service_commits_reloadable_candidate_and_advances_generation() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config_path = dir.path().join("runtime.yaml");
    let current_config = test_config(cert.clone(), key.clone());
    write_config_file(&config_path, &current_config);

    let bundle =
        runtime_bundle_from_config(config_path.to_string_lossy().as_ref(), &current_config);
    let handle = RuntimeBundleHandle::new(bundle);
    let generation_before = handle.current_generation();

    let mut next_config = test_config(cert, key);
    next_config.log.level = "debug".to_string();
    write_config_file(&config_path, &next_config);

    let activation = RuntimeActivationService::activate_reload(
        &handle,
        planner_request(generation_before),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );

    assert!(
        activation.succeeded(),
        "activation should succeed: {activation:?}"
    );
    assert_eq!(activation.status, GenerationStatus::Active);
    assert_eq!(activation.active_generation, generation_before + 1);
    assert_eq!(activation.activated_generation, Some(generation_before + 1));
    assert_eq!(handle.current_generation(), generation_before + 1);
    assert_eq!(
        activation.history_entry.operation,
        GenerationOperation::Activate
    );
    assert_eq!(activation.history_entry.status, GenerationStatus::Active);
    assert_eq!(
        activation.history_entry.config_source,
        config_path.to_string_lossy()
    );
    assert_eq!(activation.history_entry.config_version, Some(1));
    assert_eq!(
        activation.history_entry.trigger_source.as_deref(),
        Some("unit_test")
    );
    assert!(
        activation
            .history_entry
            .diff
            .reloadable_entries()
            .iter()
            .any(|entry| entry.domain == "observability_control_plane"),
        "expected activation history to preserve the planned reloadable diff"
    );
    assert_eq!(handle.current_view().startup().log_config.level, "debug");

    let history = handle.generation_change_history();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].operation, GenerationOperation::Activate);
    assert_eq!(history[1].operation, GenerationOperation::Preview);
    assert_eq!(history[2].operation, GenerationOperation::Validate);
    assert_eq!(history[0].config_source, config_path.to_string_lossy());
    assert_eq!(history[0].config_version, Some(1));

    let events = handle.generation_change_events();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].kind, GenerationEventKind::ActivationSucceeded);
    assert_eq!(events[1].kind, GenerationEventKind::Preview);
    assert_eq!(events[2].kind, GenerationEventKind::Validation);
}

#[test]
fn activation_from_alternate_config_path_updates_default_runtime_source() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let primary_path = dir.path().join("primary.yaml");
    let canary_path = dir.path().join("canary.yaml");

    let current_config = test_config(cert.clone(), key.clone());
    write_config_file(&primary_path, &current_config);

    let bundle =
        runtime_bundle_from_config(primary_path.to_string_lossy().as_ref(), &current_config);
    let handle = RuntimeBundleHandle::new(bundle);
    let generation_before = handle.current_generation();

    let mut canary_config = test_config(cert, key);
    canary_config.log.level = "debug".to_string();
    write_config_file(&canary_path, &canary_config);

    let activation = RuntimeActivationService::activate_reload(
        &handle,
        planner_request(generation_before),
        ReloadConfigInput::Path {
            path: canary_path.to_string_lossy().to_string(),
        },
    );

    assert!(activation.succeeded(), "canary activation should succeed");
    assert_eq!(
        handle.current_view().startup().config_path,
        canary_path.to_string_lossy()
    );
    assert_eq!(
        activation.history_entry.config_source,
        canary_path.to_string_lossy()
    );

    let mut followup_config = canary_config.clone();
    followup_config.log.level = "warn".to_string();
    write_config_file(&canary_path, &followup_config);

    let followup = RuntimeActivationService::activate_reload(
        &handle,
        planner_request(handle.current_generation()),
        ReloadConfigInput::Path {
            path: handle.current_view().startup().config_path.clone(),
        },
    );

    assert!(followup.succeeded(), "follow-up activation should succeed");
    assert_eq!(handle.current_view().startup().log_config.level, "warn");
    assert_eq!(
        handle.current_view().startup().config_path,
        canary_path.to_string_lossy()
    );
}

#[test]
fn activation_service_rejects_restart_required_changes_without_mutating_active_generation() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config_path = dir.path().join("runtime.yaml");
    let current_config = test_config(cert.clone(), key.clone());
    write_config_file(&config_path, &current_config);

    let bundle =
        runtime_bundle_from_config(config_path.to_string_lossy().as_ref(), &current_config);
    let handle = RuntimeBundleHandle::new(bundle);
    let generation_before = handle.current_generation();

    let mut next_config = test_config(cert, key);
    next_config.performance.control_plane_threads = current_config
        .performance
        .control_plane_threads
        .saturating_add(2);
    write_config_file(&config_path, &next_config);

    let activation = RuntimeActivationService::activate_reload(
        &handle,
        planner_request(generation_before),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );

    assert!(
        !activation.succeeded(),
        "restart-required activation must be rejected"
    );
    assert_eq!(activation.status, GenerationStatus::Rejected);
    assert_eq!(activation.active_generation, generation_before);
    assert_eq!(activation.activated_generation, None);
    assert_eq!(handle.current_generation(), generation_before);
    assert_eq!(
        activation.history_entry.operation,
        GenerationOperation::Activate
    );
    assert_eq!(activation.history_entry.status, GenerationStatus::Rejected);
    assert!(
        activation.rejected_changes.iter().any(|rejection| {
            rejection.kind == RejectedChangeKind::RestartRequired
                && rejection.reason == RuntimeRejectionReason::StartupOwnedChange
                && rejection.field_path.as_deref() == Some("performance.control_plane_threads")
                && !rejection.active_generation_changed
        }),
        "expected a restart-required rejection without live mutation: {:?}",
        activation.rejected_changes
    );

    let events = handle.generation_change_events();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].kind, GenerationEventKind::ActivationFailed);
    assert_eq!(events[0].entry.status, GenerationStatus::Rejected);
    assert_eq!(events[1].kind, GenerationEventKind::Preview);
    assert_eq!(events[2].kind, GenerationEventKind::Validation);
}

#[test]
fn rollback_service_restores_retained_generation_by_id_and_records_rollback_status() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");

    let mut generation_one = test_config(cert.clone(), key.clone());
    generation_one
        .upstream
        .get_mut("api")
        .expect("api upstream")
        .backends[0]
        .address = "http://127.0.0.1:7001".to_string();
    let mut generation_two = test_config(cert.clone(), key.clone());
    generation_two.log.level = "debug".to_string();
    generation_two
        .upstream
        .get_mut("api")
        .expect("api upstream")
        .backends[0]
        .address = "http://127.0.0.1:7002".to_string();
    let mut generation_three = test_config(cert, key);
    generation_three.log.level = "trace".to_string();
    generation_three
        .upstream
        .get_mut("api")
        .expect("api upstream")
        .backends[0]
        .address = "http://127.0.0.1:7003".to_string();

    let mut bundle_one = runtime_bundle_from_config("gen-1.yaml", &generation_one);
    bundle_one.generation = 1;
    let mut bundle_two = runtime_bundle_from_config("gen-2.yaml", &generation_two);
    bundle_two.generation = 2;
    let mut bundle_three = runtime_bundle_from_config("gen-3.yaml", &generation_three);
    bundle_three.generation = 3;

    let handle = RuntimeBundleHandle::new(bundle_one);
    handle.replace(bundle_two).expect("install generation 2");
    handle.replace(bundle_three).expect("install generation 3");

    let rollback = RuntimeActivationService::rollback_generation(
        &handle,
        rollback_request(1, handle.current_generation()),
    );

    assert!(
        rollback.succeeded(),
        "rollback should succeed: {rollback:?}"
    );
    assert_eq!(rollback.status, GenerationStatus::RolledBack);
    assert_eq!(rollback.rolled_back_to, Some(1));
    assert_eq!(rollback.active_generation, 4);
    assert_eq!(handle.current_generation(), 4);
    assert_eq!(
        rollback.history_entry.operation,
        GenerationOperation::Rollback
    );
    assert_eq!(rollback.history_entry.status, GenerationStatus::RolledBack);
    assert_eq!(
        handle
            .current_view()
            .runtime_config()
            .upstreams
            .get("api")
            .expect("active upstream")
            .backends[0]
            .backend
            .address,
        "http://127.0.0.1:7001"
    );

    let history = handle.generation_history();
    assert_eq!(history[1].generation(), 3);
    assert_eq!(
        history[1].status(),
        crate::runtime::bundle::RuntimeGenerationRecordStatus::RolledBack
    );

    let events = handle.generation_change_events();
    assert_eq!(events[0].kind, GenerationEventKind::RollbackSucceeded);
    assert_eq!(events[0].entry.config_source, "gen-1.yaml");
    assert_eq!(events[0].entry.config_version, Some(1));
    assert_eq!(events[0].entry.trigger_source.as_deref(), Some("unit_test"));
}

#[test]
fn rollback_service_rejects_incomplete_or_failed_prepare_targets_without_mutation() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let handle = RuntimeBundleHandle::new(runtime_bundle_from_config(
        "active.yaml",
        &test_config(cert, key),
    ));
    let active_generation = handle.current_generation();
    handle.record_failed_prepare(9, "candidate generation never prepared");

    let rollback = RuntimeActivationService::rollback_generation(
        &handle,
        rollback_request(9, active_generation),
    );

    assert!(
        !rollback.succeeded(),
        "failed-prepare history entries must not be rollbackable"
    );
    assert_eq!(rollback.status, GenerationStatus::Rejected);
    assert_eq!(rollback.rolled_back_to, None);
    assert_eq!(rollback.active_generation, active_generation);
    assert_eq!(handle.current_generation(), active_generation);
    assert!(
        rollback.rejected_changes.iter().any(|rejection| {
            rejection.kind == RejectedChangeKind::RuntimeStateUnavailable
                && rejection.reason == RuntimeRejectionReason::RollbackNotAllowed
                && rejection.field_path.as_deref() == Some("runtime.rollback.target_generation")
        }),
        "expected rollback rejection for incomplete target: {:?}",
        rollback.rejected_changes
    );

    let events = handle.generation_change_events();
    assert_eq!(events[0].kind, GenerationEventKind::RollbackFailed);
    assert_eq!(events[0].entry.status, GenerationStatus::Rejected);
    assert_eq!(events[0].entry.config_source, "generation:9");
}

#[test]
fn validate_plan_does_not_mutate_the_active_generation() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config_path = dir.path().join("runtime.yaml");
    let current_config = test_config(cert.clone(), key.clone());
    write_config_file(&config_path, &current_config);

    let bundle =
        runtime_bundle_from_config(config_path.to_string_lossy().as_ref(), &current_config);
    let handle = RuntimeBundleHandle::new(bundle);
    let generation_before = handle.current_generation();
    let log_level_before = handle.current_view().startup().log_config.level.clone();

    let mut restart_required = test_config(cert, key);
    restart_required.performance.control_plane_threads = current_config
        .performance
        .control_plane_threads
        .saturating_add(1);
    write_config_file(&config_path, &restart_required);

    let plan = plan_runtime_reload(
        &handle.current_view(),
        planner_request(generation_before),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );

    assert!(!plan.can_activate());
    assert_eq!(handle.current_generation(), generation_before);
    assert_eq!(
        handle.current_view().startup().log_config.level,
        log_level_before
    );
    assert_eq!(
        handle
            .current_view()
            .runtime_config()
            .policies
            .transport
            .control_plane_threads,
        current_config.performance.control_plane_threads.max(1)
    );
}

#[test]
fn preview_plan_returns_expected_diff_without_mutating_the_active_generation() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config_path = dir.path().join("runtime.yaml");
    let current_config = test_config(cert.clone(), key.clone());
    write_config_file(&config_path, &current_config);

    let bundle =
        runtime_bundle_from_config(config_path.to_string_lossy().as_ref(), &current_config);
    let handle = RuntimeBundleHandle::new(bundle);
    let generation_before = handle.current_generation();

    let mut next_config = test_config(cert, key);
    next_config.log.level = "debug".to_string();
    write_config_file(&config_path, &next_config);

    let plan = plan_runtime_reload(
        &handle.current_view(),
        planner_request(generation_before),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );

    assert!(plan.can_activate());
    assert_eq!(handle.current_generation(), generation_before);
    let reloadable = plan
        .plan
        .diff
        .reloadable_entries()
        .into_iter()
        .find(|entry| entry.domain == "observability_control_plane")
        .expect("reloadable observability diff");
    assert!(reloadable.summary.contains("log(level=info"));
    assert!(reloadable.summary.contains("log(level=debug"));
}

#[test]
fn invalid_activation_leaves_active_generation_unchanged() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config_path = dir.path().join("runtime.yaml");
    let current_config = test_config(cert, key);
    write_config_file(&config_path, &current_config);

    let bundle =
        runtime_bundle_from_config(config_path.to_string_lossy().as_ref(), &current_config);
    let handle = RuntimeBundleHandle::new(bundle);
    let generation_before = handle.current_generation();

    std::fs::write(&config_path, "version: 1\nlisten:\n  protocol: \"http3\"\n")
        .expect("write invalid config");

    let activation = RuntimeActivationService::activate_reload(
        &handle,
        planner_request(generation_before),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );

    assert!(!activation.succeeded());
    assert_eq!(activation.activated_generation, None);
    assert_eq!(activation.status, GenerationStatus::Rejected);
    assert_eq!(handle.current_generation(), generation_before);
    assert!(
        activation.rejected_changes.iter().any(|rejection| {
            rejection.kind == RejectedChangeKind::InvalidConfiguration
                && rejection.reason == RuntimeRejectionReason::InvalidConfig
        }),
        "expected invalid config rejection: {:?}",
        activation.rejected_changes
    );
}

#[test]
fn runtime_history_records_activate_fail_and_rollback_flow() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config_path = dir.path().join("runtime.yaml");

    let generation_one = test_config(cert.clone(), key.clone());
    write_config_file(&config_path, &generation_one);
    let mut bundle_one = runtime_bundle_from_config("gen-1.yaml", &generation_one);
    bundle_one.generation = 1;
    let handle = RuntimeBundleHandle::new(bundle_one);

    let mut generation_two = test_config(cert.clone(), key.clone());
    generation_two.log.level = "debug".to_string();
    write_config_file(&config_path, &generation_two);
    let activation = RuntimeActivationService::activate_reload(
        &handle,
        planner_request(handle.current_generation()),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );
    assert!(activation.succeeded());

    let mut rejected = test_config(cert, key);
    rejected.performance.control_plane_threads = generation_two
        .performance
        .control_plane_threads
        .saturating_add(1);
    write_config_file(&config_path, &rejected);
    let failed = RuntimeActivationService::activate_reload(
        &handle,
        planner_request(handle.current_generation()),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );
    assert!(!failed.succeeded());

    let rollback = RuntimeActivationService::rollback_generation(
        &handle,
        rollback_request(1, handle.current_generation()),
    );
    assert!(rollback.succeeded());

    let history = handle.generation_change_history();
    let operations = history
        .iter()
        .map(|entry| (entry.operation, entry.status))
        .take(8)
        .collect::<Vec<_>>();
    assert_eq!(
        operations,
        vec![
            (GenerationOperation::Rollback, GenerationStatus::RolledBack),
            (GenerationOperation::Activate, GenerationStatus::Rejected),
            (GenerationOperation::Preview, GenerationStatus::Rejected),
            (GenerationOperation::Validate, GenerationStatus::Rejected),
            (GenerationOperation::Activate, GenerationStatus::Active),
            (GenerationOperation::Preview, GenerationStatus::Staged),
            (GenerationOperation::Validate, GenerationStatus::Staged),
        ]
    );
    assert_eq!(handle.current_generation(), 3);
    assert_eq!(
        handle
            .current_view()
            .runtime_config()
            .upstreams
            .get("api")
            .expect("active upstream")
            .backends[0]
            .backend
            .address,
        "http://127.0.0.1:7001"
    );
}

// Domain: watchdog service state transitions and restart environment handling.
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

// Domain: control API auth and route gating contracts.
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
fn control_api_route_gating_accepts_only_canonical_method_and_path_pairs() {
    let state = default_control_api_state();
    let paths = state.current_paths();

    let cases = [
        (
            Method::GET,
            paths.health_path.clone(),
            Some(super::auth::ControlApiRoute::Health),
        ),
        (
            Method::GET,
            paths.ready_path.clone(),
            Some(super::auth::ControlApiRoute::Ready),
        ),
        (
            Method::GET,
            paths.runtime_path.clone(),
            Some(super::auth::ControlApiRoute::Runtime),
        ),
        (
            Method::GET,
            paths.runtime_history_path(),
            Some(super::auth::ControlApiRoute::RuntimeHistory),
        ),
        (
            Method::GET,
            format!("{}/2", paths.runtime_history_path()),
            Some(super::auth::ControlApiRoute::RuntimeHistoryGeneration(2)),
        ),
        (
            Method::POST,
            paths.reload_certs_path.clone(),
            Some(super::auth::ControlApiRoute::ReloadCerts),
        ),
        (
            Method::POST,
            paths.runtime_validate_path(),
            Some(super::auth::ControlApiRoute::RuntimeValidate),
        ),
        (
            Method::POST,
            paths.runtime_preview_path(),
            Some(super::auth::ControlApiRoute::RuntimePreview),
        ),
        (
            Method::POST,
            paths.runtime_activate_path(),
            Some(super::auth::ControlApiRoute::RuntimeActivate),
        ),
        (
            Method::POST,
            paths.runtime_rollback_path(),
            Some(super::auth::ControlApiRoute::RuntimeRollback),
        ),
        (
            Method::POST,
            paths.reload_path.clone(),
            Some(super::auth::ControlApiRoute::ReloadRuntime),
        ),
        (
            Method::POST,
            paths.restart_path.clone(),
            Some(super::auth::ControlApiRoute::Restart),
        ),
        (Method::POST, paths.runtime_path.clone(), None),
        (Method::POST, paths.runtime_history_path(), None),
        (Method::GET, paths.reload_path.clone(), None),
        (Method::GET, "/missing".to_string(), None),
    ];

    for (method, path, expected) in cases {
        let req = control_api_request(method, &path, None);
        assert_eq!(
            QUICListener::control_api_request_route_for(&req, &state.current_paths()),
            expected,
            "unexpected route decision for {} {}",
            req.method(),
            req.uri().path()
        );
    }
}

#[test]
fn control_api_route_gating_leaves_health_and_ready_ungated_by_auth() {
    let state = default_control_api_state();
    let paths = state.current_paths();

    for path in [&paths.health_path, &paths.ready_path] {
        let req = control_api_request(Method::GET, path, None);
        let route = QUICListener::gate_control_api_request_for(&req, &state)
            .expect("health and ready routes should bypass auth");
        assert!(matches!(
            route,
            super::auth::ControlApiRoute::Health | super::auth::ControlApiRoute::Ready
        ));
    }
}

#[test]
fn control_api_authorization_uses_token_matching_independent_of_request_body_type() {
    let state = default_control_api_state();
    let runtime_path = state.current_paths().runtime_path;
    let authorized = control_api_request(Method::GET, &runtime_path, Some("Bearer secret-token"));
    let malformed = control_api_request(Method::GET, &runtime_path, Some("Bearer"));
    let missing = control_api_request(Method::GET, &runtime_path, None);

    assert!(QUICListener::control_api_is_authorized_for(
        &authorized,
        &state.current_control_api()
    ));
    assert!(!QUICListener::control_api_is_authorized_for(
        &malformed,
        &state.current_control_api()
    ));
    assert!(!QUICListener::control_api_is_authorized_for(
        &missing,
        &state.current_control_api()
    ));
}

#[tokio::test]
async fn control_api_gate_returns_canonical_unauthorized_payloads_per_route() {
    let state = default_control_api_state();
    let paths = state.current_paths();

    let cases = [
        (
            Method::GET,
            paths.runtime_path.clone(),
            serde_json::json!({ "error": "unauthorized" }),
        ),
        (
            Method::POST,
            paths.runtime_validate_path(),
            serde_json::json!({ "error": "unauthorized" }),
        ),
        (
            Method::POST,
            paths.runtime_preview_path(),
            serde_json::json!({ "error": "unauthorized" }),
        ),
        (
            Method::POST,
            paths.runtime_activate_path(),
            serde_json::json!({ "error": "unauthorized" }),
        ),
        (
            Method::POST,
            paths.runtime_rollback_path(),
            serde_json::json!({ "error": "unauthorized" }),
        ),
        (
            Method::GET,
            paths.runtime_history_path(),
            serde_json::json!({ "error": "unauthorized" }),
        ),
        (
            Method::POST,
            paths.reload_certs_path.clone(),
            serde_json::json!({ "reloaded": false, "error": "unauthorized" }),
        ),
        (
            Method::POST,
            paths.reload_path.clone(),
            serde_json::json!({ "reloaded": false, "error": "unauthorized" }),
        ),
        (
            Method::POST,
            paths.restart_path.clone(),
            serde_json::json!({ "accepted": false, "error": "unauthorized" }),
        ),
    ];

    for (method, path, expected_body) in cases {
        let req = control_api_request(method, &path, None);
        let response = QUICListener::gate_control_api_request_for(&req, &state)
            .expect_err("protected route should reject missing auth");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = full_body_bytes(*response).await;
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("unauthorized payload");
        assert_eq!(payload, expected_body);
    }
}

#[tokio::test]
async fn control_api_gate_returns_not_found_for_invalid_routes_without_transport_coupling() {
    let state = default_control_api_state();
    let req = control_api_request(
        Method::DELETE,
        &state.current_paths().runtime_path,
        Some("Bearer secret-token"),
    );

    let response = QUICListener::gate_control_api_request_for(&req, &state)
        .expect_err("invalid route should map to not found");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        full_body_bytes(*response).await,
        Bytes::from_static(b"not found\n")
    );
}

// Domain: runtime snapshot rendering and live runtime-view selection.
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

#[tokio::test]
async fn control_api_runtime_snapshot_uses_live_primary_listener_label_after_bundle_replace() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let mut startup = test_config(cert.clone(), key.clone());
    startup.observability.control_api.enabled = true;

    let startup_bundle = runtime_bundle_from_config("startup.yaml", &startup);
    let (state, runtime_handle) = runtime_bundle_control_api_state(startup_bundle);

    let startup_payload =
        json_body(QUICListener::render_control_api_runtime_snapshot(&state)).await;
    let startup_listeners = startup_payload["tls"]["listeners"]
        .as_object()
        .expect("startup listeners object");
    assert!(
        startup_listeners.contains_key("127.0.0.1:9889"),
        "startup snapshot should expose the startup primary listener label"
    );

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

    let mut reloaded_bundle = runtime_bundle_from_config("reloaded.yaml", &reloaded);
    reloaded_bundle.generation = 1;
    runtime_handle
        .replace(reloaded_bundle)
        .expect("replace runtime bundle");

    assert_eq!(
        state.current_primary_listener_label().as_deref(),
        Some("127.0.0.1:9890"),
        "control api state must prefer the live generation's primary listener label"
    );

    let live_payload = json_body(QUICListener::render_control_api_runtime_snapshot(&state)).await;
    let live_listeners = live_payload["tls"]["listeners"]
        .as_object()
        .expect("live listeners object");

    assert_eq!(live_payload["runtime"]["generation"], 1);
    assert!(
        live_listeners.contains_key("127.0.0.1:9890"),
        "runtime snapshot must render the live generation's primary listener label after bundle replacement"
    );
    assert!(
        live_listeners.contains_key("127.0.0.1:9889"),
        "runtime snapshot should keep the remaining listener inventory from the live generation"
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
fn control_api_gating_uses_live_generation_paths_and_auth_after_bundle_replace() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let mut startup = test_config(cert.clone(), key.clone());
    startup.observability.control_api.enabled = true;
    startup.observability.control_api.runtime_path = "/runtime-startup".to_string();
    startup.observability.control_api.auth_token = Some("startup-token".to_string());

    let startup_bundle = runtime_bundle_from_config("startup.yaml", &startup);
    let (state, runtime_handle) = runtime_bundle_control_api_state(startup_bundle);

    let mut reloaded = startup.clone();
    reloaded.observability.control_api.runtime_path = "/runtime-reloaded".to_string();
    reloaded.observability.control_api.auth_token = Some("reloaded-token".to_string());
    let mut reloaded_bundle = runtime_bundle_from_config("reloaded.yaml", &reloaded);
    reloaded_bundle.generation = 1;
    runtime_handle
        .replace(reloaded_bundle)
        .expect("replace runtime bundle");

    let startup_path = control_api_request(
        Method::GET,
        "/runtime-startup",
        Some("Bearer startup-token"),
    );
    let startup_err = QUICListener::gate_control_api_request_for(&startup_path, &state)
        .expect_err("stale runtime path should be rejected after replacement");
    assert_eq!(startup_err.status(), StatusCode::NOT_FOUND);

    let stale_token = control_api_request(
        Method::GET,
        "/runtime-reloaded",
        Some("Bearer startup-token"),
    );
    let stale_token_err = QUICListener::gate_control_api_request_for(&stale_token, &state)
        .expect_err("stale token should be rejected after replacement");
    assert_eq!(stale_token_err.status(), StatusCode::UNAUTHORIZED);

    let live = control_api_request(
        Method::GET,
        "/runtime-reloaded",
        Some("Bearer reloaded-token"),
    );
    let route = QUICListener::gate_control_api_request_for(&live, &state)
        .expect("live control api path and auth should be accepted");
    assert!(matches!(route, super::auth::ControlApiRoute::Runtime));
}

#[test]
fn control_api_backend_inventory_and_summary_share_one_canonical_snapshot_contract() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config = test_config(cert, key);
    let bundle = runtime_bundle_from_config("runtime.yaml", &config);
    let (state, _) = runtime_bundle_control_api_state(bundle);

    let service_state = state.current_service_state();
    let inventory = service_state.snapshot_backend_inventory();
    let summary = service_state.snapshot_backend_health();

    assert_eq!(summary, inventory.summary());
    assert_eq!(inventory.backends.len(), 1);

    let backend = &inventory.backends[0];
    assert_eq!(backend.identity.backend_addr, "http://127.0.0.1:7001");
    assert_eq!(backend.resolution.authority_host, "127.0.0.1");
    assert_eq!(backend.resolution.authority_port, 7001);
    assert_eq!(backend.resolution.refresh_generation, 0);
    assert_eq!(
        backend.membership,
        crate::runtime::backend::state::BackendMembershipState::Active
    );
    assert!(matches!(
        backend.health,
        crate::runtime::backend::state::BackendHealthState::Healthy
    ));
    assert_eq!(backend.placements.len(), 1);
    assert_eq!(backend.placements[0].upstream_name, "api");
    assert!(backend.placements[0].healthy);
    assert_eq!(summary.total_backends, 1);
    assert_eq!(summary.healthy_backends, 1);
}

#[tokio::test]
async fn control_api_runtime_history_renders_recorded_generation_changes() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config_path = dir.path().join("runtime.yaml");
    let current_config = test_config(cert.clone(), key.clone());
    write_config_file(&config_path, &current_config);

    let bundle =
        runtime_bundle_from_config(config_path.to_string_lossy().as_ref(), &current_config);
    let (state, runtime_handle) = runtime_bundle_control_api_state(bundle);

    let mut next_config = test_config(cert, key);
    next_config.log.level = "debug".to_string();
    write_config_file(&config_path, &next_config);

    let activation = RuntimeActivationService::activate_reload(
        runtime_handle.as_ref(),
        planner_request(runtime_handle.current_generation()),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );
    assert!(activation.succeeded());

    let payload = json_body(QUICListener::render_control_api_runtime_history(&state)).await;
    assert_eq!(payload["active_generation"], 1);
    assert_eq!(payload["entries"].as_array().map(Vec::len), Some(3));
    assert_eq!(payload["entries"][0]["operation"], "activate");
    assert_eq!(payload["entries"][1]["operation"], "preview");
    assert_eq!(payload["entries"][2]["operation"], "validate");
}

#[tokio::test]
async fn control_api_runtime_history_generation_filters_to_requested_generation() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config_path = dir.path().join("runtime.yaml");
    let current_config = test_config(cert.clone(), key.clone());
    write_config_file(&config_path, &current_config);

    let bundle =
        runtime_bundle_from_config(config_path.to_string_lossy().as_ref(), &current_config);
    let (state, runtime_handle) = runtime_bundle_control_api_state(bundle);

    let mut next_config = test_config(cert, key);
    next_config.log.level = "debug".to_string();
    write_config_file(&config_path, &next_config);

    let activation = RuntimeActivationService::activate_reload(
        runtime_handle.as_ref(),
        planner_request(runtime_handle.current_generation()),
        ReloadConfigInput::Path {
            path: config_path.to_string_lossy().to_string(),
        },
    );
    assert!(activation.succeeded());

    let payload = json_body(QUICListener::render_control_api_runtime_history_generation(
        &state, 1,
    ))
    .await;
    assert_eq!(payload["generation"], 1);
    assert_eq!(payload["entries"].as_array().map(Vec::len), Some(3));

    let missing = QUICListener::render_control_api_runtime_history_generation(&state, 99);
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn control_api_runtime_snapshot_renders_live_generation_listener_and_backend_contract() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let mut startup = test_config(cert.clone(), key.clone());
    startup.observability.control_api.enabled = true;

    let startup_bundle = runtime_bundle_from_config("startup.yaml", &startup);
    let (state, runtime_handle) = runtime_bundle_control_api_state(startup_bundle);

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
                client_auth: ClientAuth {
                    enabled: true,
                    ca_file: Some(cert.clone()),
                    require_client_cert: true,
                },
            },
        },
        startup.listen.clone(),
    ];
    reloaded.observability.metrics.path = "/metrics-live".to_string();

    let mut reloaded_bundle = runtime_bundle_from_config("reloaded.yaml", &reloaded);
    reloaded_bundle.generation = 1;
    runtime_handle
        .replace(reloaded_bundle)
        .expect("replace runtime bundle");

    let live = runtime_handle.current_view();
    live.shared_services()
        .metrics
        .requests_total
        .store(11, std::sync::atomic::Ordering::Relaxed);
    live.shared_services()
        .metrics
        .requests_success
        .store(7, std::sync::atomic::Ordering::Relaxed);
    live.shared_services()
        .metrics
        .requests_failure
        .store(4, std::sync::atomic::Ordering::Relaxed);
    live.shared_services()
        .metrics
        .active_connections
        .store(3, std::sync::atomic::Ordering::Relaxed);

    let payload = json_body(QUICListener::render_control_api_runtime_snapshot(&state)).await;

    assert_eq!(payload["runtime"]["generation"], 1);
    assert_eq!(payload["runtime"]["config_path"], "reloaded.yaml");
    assert_eq!(payload["metrics"]["requests_total"], 11);
    assert_eq!(payload["metrics"]["requests_success"], 7);
    assert_eq!(payload["metrics"]["requests_failure"], 4);
    assert_eq!(payload["metrics"]["active_connections"], 3);

    let listeners = payload["tls"]["listeners"]
        .as_object()
        .expect("listeners object");
    assert!(
        listeners.contains_key("127.0.0.1:9890"),
        "runtime snapshot should render listener inventory from the active generation"
    );
    assert!(
        listeners.contains_key("127.0.0.1:9889"),
        "runtime snapshot should keep the secondary listener visible"
    );
    assert_eq!(
        listeners["127.0.0.1:9890"]["client_auth_enabled"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        listeners["127.0.0.1:9890"]["require_client_cert"],
        serde_json::Value::Bool(true)
    );

    assert_eq!(payload["backends"]["healthy"], 1);
    assert_eq!(payload["backends"]["total"], 1);
    let lifecycle = payload["backends"]["lifecycle"]
        .as_array()
        .expect("backend lifecycle array");
    assert_eq!(lifecycle.len(), 1);
    let backend = &lifecycle[0];
    assert_eq!(backend["backend"], "http://127.0.0.1:7001");
    assert_eq!(backend["health"], "healthy");
    assert_eq!(backend["membership"], "active");
    assert_eq!(backend["authority_host"], "127.0.0.1");
    assert_eq!(backend["authority_port"], 7001);
    assert_eq!(backend["resolution_generation"], 0);
    assert!(
        backend.get("health_reason").is_none(),
        "healthy rendered backends must omit optional health_reason"
    );
    assert_eq!(
        backend["placements"]
            .as_array()
            .expect("placement array")
            .len(),
        1
    );
}

// Domain: watchdog/runtime ownership alignment across reload boundaries.
#[test]
fn reload_preserves_process_scoped_watchdog_and_dns_resolver() {
    // Regression: process-shared services must be carried across a reload, not
    // rebuilt. Rebuilding the watchdog silently discards an in-flight
    // restart/drain; rebuilding the DNS resolver drops its cache.
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let mut config = test_config(cert, key);
    // Enable the watchdog so request_restart takes effect.
    config.resilience.watchdog.enabled = true;
    let runtime_config = RuntimeConfig::from_config(&config).expect("runtime config");

    let current = QUICListener::build_shared_state(&runtime_config).expect("current shared state");

    // Simulate an in-flight watchdog restart on the active generation.
    assert!(
        current.shared_services().watchdog.request_restart("test"),
        "watchdog restart should be accepted when enabled"
    );
    assert!(current.shared_services().watchdog.restart_requested());

    // Reload: carry the process-scoped services forward.
    let carried = crate::runtime::generation::CarriedProcessSharedServices::from_active(
        current.shared_services(),
    );
    let next = QUICListener::build_shared_state_with_carried(&runtime_config, Some(carried))
        .expect("reloaded shared state");

    // Same watchdog instance, and its in-flight restart survived the swap.
    assert!(
        Arc::ptr_eq(
            &current.shared_services().watchdog,
            &next.shared_services().watchdog
        ),
        "watchdog must be the same instance across a reload"
    );
    assert!(
        next.shared_services().watchdog.restart_requested(),
        "in-flight watchdog restart must survive a reload"
    );

    // The carried services expose the same DNS resolver handle, so its cache is
    // preserved rather than rebuilt cold on every reload.
    let carried_again = crate::runtime::generation::CarriedProcessSharedServices::from_active(
        current.shared_services(),
    );
    let seeded = carried_again
        .backend_dns_resolver
        .cached_addrs("never-resolved.example");
    assert_eq!(
        seeded,
        next.shared_services()
            .backend_dns_resolver
            .cached_addrs("never-resolved.example"),
        "carried DNS resolver must be the same cache view as the reloaded generation"
    );
}

// Domain: reload compatibility classification and lifecycle gatekeeping.
#[test]
fn live_reloadable_upstream_change_is_accepted() {
    // Phase 9 (#1): a generation-owned change (upstream/route table) must pass
    // reload compatibility — it is live-reloadable, not restart-required.
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert, key);

    let mut next = current.clone();
    next.upstream
        .get_mut("api")
        .expect("api upstream")
        .backends
        .push(Backend {
            id: "b2".to_string(),
            address: "http://127.0.0.1:7002".to_string(),
            weight: 1,
            health_check: None,
        });

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);
    let current_active = RuntimeBundleHandle::new(current_bundle).current_view();

    let result = QUICListener::evaluate_runtime_reload_compatibility(&current_active, &next_bundle);
    assert!(
        result.is_ok(),
        "generation-owned upstream change must be live-reloadable, got: {result:?}"
    );
}

#[test]
fn reload_compatibility_classifies_generation_owned_changes_as_live_reloadable() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert, key);

    let mut next = current.clone();
    next.upstream
        .get_mut("api")
        .expect("api upstream")
        .route
        .path_prefix = Some("/live".to_string());

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);
    let active = RuntimeBundleHandle::new(current_bundle).current_view();

    assert!(
        QUICListener::evaluate_runtime_reload_compatibility(&active, &next_bundle).is_ok(),
        "generation-owned routing changes should stay reloadable"
    );
}

#[test]
fn restart_required_change_rejects_before_touching_active_generation() {
    // Phase 9 (#4): a failed validation must leave the active generation unchanged.
    // A restart-required change (control_plane_threads) is rejected by the
    // compatibility evaluation, which runs entirely on the candidate bundle and
    // never mutates the live handle.
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert, key);

    let mut next = current.clone();
    next.performance.control_plane_threads = current.performance.control_plane_threads + 3;

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);

    let handle = RuntimeBundleHandle::new(current_bundle);
    let generation_before = handle.current_generation();
    let active = handle.current_view();

    let result = QUICListener::evaluate_runtime_reload_compatibility(&active, &next_bundle);
    let rejections = result.expect_err("restart-required change must be rejected");
    assert!(
        rejections.iter().any(
            |r| r.to_string().contains("performance.control_plane_threads") && r.requires_restart()
        ),
        "expected a restart-required rejection, got: {rejections:?}"
    );
    // The active generation is untouched by the rejected evaluation.
    assert_eq!(handle.current_generation(), generation_before);
}

#[test]
fn reload_compatibility_classifies_startup_owned_changes_as_restart_required() {
    use crate::runtime::policy::TransitionRejectionKind;

    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert, key);

    let mut next = current.clone();
    let current_threads = current.performance.control_plane_threads;
    let next_threads = current_threads.saturating_add(2);
    next.performance.control_plane_threads = next_threads;

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);
    let active = RuntimeBundleHandle::new(current_bundle).current_view();

    let rejections = QUICListener::evaluate_runtime_reload_compatibility(&active, &next_bundle)
        .expect_err("startup-owned change must reject the reload");
    assert_eq!(rejections.len(), 1);
    let rejection = &rejections[0];
    assert_eq!(rejection.kind, TransitionRejectionKind::RestartRequired);
    assert!(rejection.requires_restart());
    assert_eq!(
        rejection.field_path.as_deref(),
        Some("performance.control_plane_threads")
    );
    assert_eq!(
        rejection.to_string(),
        format!(
            "runtime reload rejected: performance.control_plane_threads changed from {current_threads} to {next_threads}; restart required"
        )
    );
}

#[test]
fn accepted_reload_advances_the_active_generation() {
    // Phase 9 (#1/#4 positive path): a valid reload committed through the handle
    // advances the active generation atomically.
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert, key);
    let next = current.clone();

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let mut next_bundle = runtime_bundle_from_config("next.yaml", &next);
    next_bundle.generation = current_bundle.generation + 1;

    let handle = RuntimeBundleHandle::new(current_bundle);
    let before = handle.current_generation();

    let committed = handle.replace(next_bundle).expect("valid reload commits");
    assert_eq!(committed, before + 1);
    assert_eq!(handle.current_generation(), before + 1);
}

#[test]
fn handle_drives_full_lifecycle_transition_table() {
    use crate::runtime::policy::{LifecycleTransitionResult, RuntimeLifecyclePhase};

    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config = test_config(cert, key);
    let bundle = runtime_bundle_from_config("current.yaml", &config);
    let handle = RuntimeBundleHandle::new(bundle);

    // The handle publishes into `Running`.
    assert_eq!(handle.lifecycle().phase(), RuntimeLifecyclePhase::Running);

    // Running -> Draining (as the supervisor does on a watchdog restart request).
    assert!(matches!(
        handle.lifecycle().begin_drain(),
        LifecycleTransitionResult::Applied {
            to: RuntimeLifecyclePhase::Draining,
            ..
        }
    ));

    // Draining -> ShuttingDown (as the shutdown-signal handler does).
    assert!(matches!(
        handle.lifecycle().begin_shutdown(),
        LifecycleTransitionResult::Applied {
            to: RuntimeLifecyclePhase::ShuttingDown,
            ..
        }
    ));
    // Idempotent second call (the app calls begin_shutdown again before draining).
    assert!(matches!(
        handle.lifecycle().begin_shutdown(),
        LifecycleTransitionResult::NoOp { .. }
    ));

    // ShuttingDown -> Terminated (after workers are drained and joined).
    assert!(matches!(
        handle.lifecycle().finish_shutdown(),
        LifecycleTransitionResult::Applied {
            to: RuntimeLifecyclePhase::Terminated,
            ..
        }
    ));
    assert!(handle.lifecycle().phase().is_terminal());
}

#[test]
fn reload_commit_is_rejected_after_shutdown_begins() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let config = test_config(cert, key);
    let bundle = runtime_bundle_from_config("current.yaml", &config);
    let next = runtime_bundle_from_config("next.yaml", &config);
    let generation_before = bundle.generation;

    let handle = RuntimeBundleHandle::new(bundle);
    // Phase 6: once shutdown has begun, a reload commit must be rejected and the
    // active generation left untouched.
    handle.lifecycle().begin_shutdown();

    let result = handle.replace(next);
    assert!(result.is_err(), "reload during shutdown must be rejected");
    assert_eq!(
        handle.current_generation(),
        generation_before,
        "active generation must be unchanged after a rejected reload"
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
    let result =
        QUICListener::validate_control_api_reload_compatibility(&current_bundle, &next_bundle)
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
fn validate_runtime_reload_compatibility_classifies_listener_bind_conflict_as_preflight_failure() {
    use crate::runtime::policy::TransitionRejectionKind;

    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert.clone(), key.clone());

    let occupied = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind occupied udp");
    let occupied_port = occupied.local_addr().expect("occupied addr").port();

    let mut next = current.clone();
    let mut extra_listener = next.listen.clone();
    extra_listener.port = occupied_port;
    next.listeners = vec![next.listen.clone(), extra_listener];

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);

    let rejection =
        QUICListener::validate_runtime_reload_compatibility(&current_bundle, &next_bundle)
            .expect("listener bind conflict must reject");
    let expected_field = format!("QUIC listener '127.0.0.1:{occupied_port}'");
    assert_eq!(
        rejection.kind,
        TransitionRejectionKind::ResourcePreparationFailed
    );
    assert!(!rejection.requires_restart());
    assert_eq!(
        rejection.field_path.as_deref(),
        Some(expected_field.as_str())
    );
    assert_structured_resource_preflight_message(&rejection);
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
fn validate_control_api_reload_compatibility_classifies_bind_conflict_as_preflight_failure() {
    use crate::runtime::policy::TransitionRejectionKind;

    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert.clone(), key.clone());

    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied tcp");
    let occupied_port = occupied.local_addr().expect("occupied addr").port();

    let mut next = current.clone();
    next.observability.control_api.enabled = true;
    next.observability.control_api.address = "127.0.0.1".to_string();
    next.observability.control_api.port = occupied_port;

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);

    let rejection =
        QUICListener::validate_control_api_reload_compatibility(&current_bundle, &next_bundle)
            .expect("control api bind conflict must reject");
    let expected_field = format!("control API endpoint '127.0.0.1:{occupied_port}'");
    assert_eq!(
        rejection.kind,
        TransitionRejectionKind::ResourcePreparationFailed
    );
    assert_eq!(
        rejection.field_path.as_deref(),
        Some(expected_field.as_str())
    );
    assert!(!rejection.requires_restart());
    assert_structured_resource_preflight_message(&rejection);
}

#[test]
fn validate_metrics_reload_compatibility_classifies_bind_conflict_as_preflight_failure() {
    use crate::runtime::policy::TransitionRejectionKind;

    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_cert_for_name(dir.path(), "server", "api.example.com");
    let current = test_config(cert.clone(), key.clone());

    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind occupied tcp");
    let occupied_port = occupied.local_addr().expect("occupied addr").port();

    let mut next = current.clone();
    next.observability.metrics.enabled = true;
    next.observability.metrics.address = "127.0.0.1".to_string();
    next.observability.metrics.port = occupied_port;

    let current_bundle = runtime_bundle_from_config("current.yaml", &current);
    let next_bundle = runtime_bundle_from_config("next.yaml", &next);

    let rejection =
        QUICListener::validate_metrics_reload_compatibility(&current_bundle, &next_bundle)
            .expect("metrics bind conflict must reject");
    let expected_field = format!("metrics endpoint '127.0.0.1:{occupied_port}'");
    assert_eq!(
        rejection.kind,
        TransitionRejectionKind::ResourcePreparationFailed
    );
    assert_eq!(
        rejection.field_path.as_deref(),
        Some(expected_field.as_str())
    );
    assert!(!rejection.requires_restart());
    assert_structured_resource_preflight_message(&rejection);
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

// Domain: control-plane certificate reload and atomic service update behavior.
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
    let full_reload_issues = startup_owned_issue_strings(current_runtime.bundle(), &drifted_bundle);
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
