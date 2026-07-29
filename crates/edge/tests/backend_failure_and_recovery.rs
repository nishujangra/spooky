use std::{
    collections::HashMap,
    convert::Infallible,
    thread,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Response, body::Incoming};
use serial_test::serial;
use spooky_config::config::{Backend, HealthCheck, LoadBalancing, RouteMatch, Upstream};
use spooky_edge::runtime::backend::event::BackendRefreshOutcome;
use spooky_edge::runtime::backend::state::BackendHealthState;

mod support;

use support::{
    backend_lifecycle::{BackendLifecycleHarness, ForcedBackendRefresh, alternate_loopback_backend_addrs},
    net::local_listener_bind_available,
    request_path::{H3RequestSpec, reserve_unused_udp_port},
};

fn start_hostname_swap_backends(
    harness: &mut BackendLifecycleHarness,
) -> Option<(u16, std::net::SocketAddr, std::net::SocketAddr)> {
    for _ in 0..32 {
        let port = reserve_unused_udp_port();
        let (backend_a_bind, backend_b_bind) = alternate_loopback_backend_addrs(port);

        let Ok(backend_a_addr) = harness.try_start_h1_static_backend_at(backend_a_bind, b"backend-a")
        else {
            continue;
        };
        let Ok(backend_b_addr) = harness.try_start_h1_static_backend_at(backend_b_bind, b"backend-b")
        else {
            continue;
        };

        return Some((port, backend_a_addr, backend_b_addr));
    }

    None
}

fn run_keyed_request(
    harness: &BackendLifecycleHarness,
    key: &str,
) -> Result<support::request_path::H3Response, String> {
    let headers = [("x-tenant-id", key)];
    harness.run_request(H3RequestSpec {
        method: "GET",
        authority: "localhost",
        path: "/",
        headers: &headers,
        body: None,
        user_agent: "spooky-request-path-test",
    })
}

fn consistent_hash_upstream(backends: Vec<Backend>) -> Upstream {
    Upstream {
        load_balancing: LoadBalancing {
            lb_type: "consistent-hash".to_string(),
            key: Some("header:x-tenant-id".to_string()),
        },
        auth: Default::default(),
        host_policy: Default::default(),
        forwarded_headers: Default::default(),
        tls: None,
        route: RouteMatch {
            path_prefix: Some("/".to_string()),
            ..Default::default()
        },
        backends,
    }
}

fn find_consistent_hash_key_for_backend(
    harness: &BackendLifecycleHarness,
    expected_body: &str,
) -> Option<String> {
    for idx in 0..128 {
        let key = format!("tenant-{idx}");
        let response = run_keyed_request(harness, &key).ok()?;
        if response.status == 200 && response.body_text() == expected_body {
            return Some(key);
        }
    }
    None
}

fn wait_for_backend_health_state(
    harness: &BackendLifecycleHarness,
    backend_addr: &str,
    expected_healthy_backends: usize,
    expected_health: fn(&BackendHealthState) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let snapshot = harness.backend_snapshot(backend_addr).ok().flatten();
        let summary = harness.upstream_membership_summary("api").ok();

        if let (Some(snapshot), Some(summary)) = (snapshot, summary)
            && summary.healthy_backends == expected_healthy_backends
            && expected_health(&snapshot.health)
        {
            return true;
        }

        thread::sleep(Duration::from_millis(25));
    }

    false
}

#[test]
#[serial]
fn dns_refresh_with_changed_backend_addresses_moves_requests_and_updates_inventory() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BackendLifecycleHarness::new();
    let Some((port, backend_a_addr, backend_b_addr)) = start_hostname_swap_backends(&mut harness)
    else {
        return;
    };

    let route = harness.hostname_backend_route("backend.internal", port);
    let config = harness.make_config(HashMap::from([("api".to_string(), route.upstream())]));
    harness.start_listener(config).expect("listener");

    let seeded_refresh = harness
        .force_hostname_refresh(&route.backend_addr, vec![backend_a_addr])
        .expect("seed hostname refresh to backend A");
    match seeded_refresh {
        ForcedBackendRefresh::Updated {
            result,
            client_rotation,
        } => {
            assert!(client_rotation.rotated(), "seed refresh should rotate the backend client");
            match result.outcome {
                BackendRefreshOutcome::Updated {
                    current_addrs,
                    refresh_generation,
                    ..
                } => {
                    assert_eq!(current_addrs, vec![backend_a_addr]);
                    assert_eq!(refresh_generation, 1);
                }
                other => panic!("expected updated seed refresh outcome, got {other:?}"),
            }
        }
        other => panic!("expected updated seed refresh, got {other:?}"),
    }

    let before = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request before hostname refresh");
    before.assert_status(200);
    before.assert_body_text("backend-a");

    let refreshed = harness
        .force_hostname_refresh(&route.backend_addr, vec![backend_b_addr])
        .expect("refresh hostname resolution to backend B");
    match refreshed {
        ForcedBackendRefresh::Updated {
            result,
            client_rotation,
        } => {
            assert!(client_rotation.rotated(), "address change should rotate the backend client");
            match result.outcome {
                BackendRefreshOutcome::Updated {
                    previous_addrs,
                    current_addrs,
                    refresh_generation,
                    ..
                } => {
                    assert_eq!(previous_addrs, vec![backend_a_addr]);
                    assert_eq!(current_addrs, vec![backend_b_addr]);
                    assert_eq!(refresh_generation, 2);
                }
                other => panic!("expected updated refresh outcome, got {other:?}"),
            }
        }
        other => panic!("expected updated refresh result, got {other:?}"),
    }

    let after = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request after hostname refresh");
    after.assert_status(200);
    after.assert_body_text("backend-b");

    let backend = harness
        .backend_snapshot(&route.backend_addr)
        .expect("backend snapshot lookup")
        .expect("backend lifecycle snapshot");
    assert_eq!(backend.resolution.resolved_addrs, vec![backend_b_addr]);
    assert_eq!(backend.resolution.refresh_generation, 2);

    let inventory = harness.backend_inventory().expect("backend inventory");
    let backend = inventory
        .backends
        .iter()
        .find(|backend| backend.identity.backend_addr == route.backend_addr)
        .expect("backend inventory entry");
    assert_eq!(backend.resolution.resolved_addrs, vec![backend_b_addr]);
    assert_eq!(backend.resolution.refresh_generation, 2);
    assert!(
        backend
            .placements
            .iter()
            .any(|placement| placement.upstream_name == "api"),
        "backend lifecycle inventory should keep the backend placed in the active upstream"
    );
}

#[test]
#[serial]
fn empty_dns_refresh_retains_previous_resolution_and_keeps_requests_flowing() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BackendLifecycleHarness::new();
    let Some((port, backend_a_addr, _backend_b_addr)) = start_hostname_swap_backends(&mut harness)
    else {
        return;
    };

    let route = harness.hostname_backend_route("backend.internal", port);
    let config = harness.make_config(HashMap::from([("api".to_string(), route.upstream())]));
    harness.start_listener(config).expect("listener");

    let seeded_refresh = harness
        .force_hostname_refresh(&route.backend_addr, vec![backend_a_addr])
        .expect("seed hostname refresh to backend A");
    match seeded_refresh {
        ForcedBackendRefresh::Updated { result, .. } => match result.outcome {
            BackendRefreshOutcome::Updated {
                current_addrs,
                refresh_generation,
                ..
            } => {
                assert_eq!(current_addrs, vec![backend_a_addr]);
                assert_eq!(refresh_generation, 1);
            }
            other => panic!("expected updated seed refresh outcome, got {other:?}"),
        },
        other => panic!("expected updated seed refresh, got {other:?}"),
    }

    let before = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request before empty refresh");
    before.assert_status(200);
    before.assert_body_text("backend-a");

    let empty_refresh = harness
        .force_hostname_refresh(&route.backend_addr, Vec::new())
        .expect("empty hostname refresh");
    match empty_refresh {
        ForcedBackendRefresh::EmptyAnswerRetained { retained_addrs } => {
            assert_eq!(retained_addrs, vec![backend_a_addr]);
        }
        other => panic!("expected empty-answer retained refresh, got {other:?}"),
    }

    let after = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request after empty refresh");
    after.assert_status(200);
    after.assert_body_text("backend-a");

    let backend = harness
        .backend_snapshot(&route.backend_addr)
        .expect("backend snapshot lookup")
        .expect("backend lifecycle snapshot");
    assert_eq!(backend.resolution.resolved_addrs, vec![backend_a_addr]);
    assert_eq!(
        backend.resolution.refresh_generation, 1,
        "empty-answer refresh should preserve the existing visible resolution generation"
    );

    let inventory = harness.backend_inventory().expect("backend inventory");
    let backend = inventory
        .backends
        .iter()
        .find(|backend| backend.identity.backend_addr == route.backend_addr)
        .expect("backend inventory entry");
    assert_eq!(backend.resolution.resolved_addrs, vec![backend_a_addr]);
    assert_eq!(backend.resolution.refresh_generation, 1);
}

#[test]
#[serial]
fn backend_refresh_rotates_transport_clients_without_leaving_stale_routing() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BackendLifecycleHarness::new();
    let Some((port, backend_a_bind, backend_b_bind)) = ({
        let port = reserve_unused_udp_port();
        let (a, b) = alternate_loopback_backend_addrs(port);
        Some((port, a, b))
    }) else {
        return;
    };

    let backend_a_requests = Arc::new(AtomicUsize::new(0));
    let backend_b_requests = Arc::new(AtomicUsize::new(0));

    let backend_a_counter = Arc::clone(&backend_a_requests);
    let backend_a_addr = harness
        .try_start_h1_backend_at(backend_a_bind, move |_req: hyper::Request<Incoming>| {
            let backend_a_counter = Arc::clone(&backend_a_counter);
            async move {
                backend_a_counter.fetch_add(1, Ordering::Relaxed);
                Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(
                    b"backend-a",
                ))))
            }
        })
        .ok();
    let Some(backend_a_addr) = backend_a_addr else {
        return;
    };

    let backend_b_counter = Arc::clone(&backend_b_requests);
    let backend_b_addr = harness
        .try_start_h1_backend_at(backend_b_bind, move |_req: hyper::Request<Incoming>| {
            let backend_b_counter = Arc::clone(&backend_b_counter);
            async move {
                backend_b_counter.fetch_add(1, Ordering::Relaxed);
                Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(
                    b"backend-b",
                ))))
            }
        })
        .ok();
    let Some(backend_b_addr) = backend_b_addr else {
        return;
    };

    let route = harness.hostname_backend_route("backend.internal", port);
    let config = harness.make_config(HashMap::from([("api".to_string(), route.upstream())]));
    harness.start_listener(config).expect("listener");

    harness
        .force_hostname_refresh(&route.backend_addr, vec![backend_a_addr])
        .expect("seed hostname refresh to backend A");

    let before = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request before refresh");
    before.assert_status(200);
    before.assert_body_text("backend-a");
    assert_eq!(
        backend_a_requests.load(Ordering::Relaxed),
        1,
        "initial request should be served by the original backend address"
    );
    assert_eq!(
        backend_b_requests.load(Ordering::Relaxed),
        0,
        "replacement backend should not receive traffic before refresh"
    );

    let refreshed = harness
        .force_hostname_refresh(&route.backend_addr, vec![backend_b_addr])
        .expect("refresh hostname resolution to backend B");
    match refreshed {
        ForcedBackendRefresh::Updated { result, .. } => match result.outcome {
            BackendRefreshOutcome::Updated {
                previous_addrs,
                current_addrs,
                refresh_generation,
                ..
            } => {
                assert_eq!(previous_addrs, vec![backend_a_addr]);
                assert_eq!(current_addrs, vec![backend_b_addr]);
                assert_eq!(refresh_generation, 2);
            }
            other => panic!("expected updated refresh outcome, got {other:?}"),
        },
        other => panic!("expected updated refresh result, got {other:?}"),
    }

    for _ in 0..3 {
        let response = harness
            .run_request(H3RequestSpec::get("localhost", "/"))
            .expect("request after refresh");
        response.assert_status(200);
        response.assert_body_text("backend-b");
    }

    assert_eq!(
        backend_a_requests.load(Ordering::Relaxed),
        1,
        "old backend address should stop serving traffic after refresh settles"
    );
    assert_eq!(
        backend_b_requests.load(Ordering::Relaxed),
        3,
        "new backend address should begin serving all post-refresh traffic"
    );

    let backend = harness
        .backend_snapshot(&route.backend_addr)
        .expect("backend snapshot lookup")
        .expect("backend lifecycle snapshot");
    assert_eq!(backend.resolution.resolved_addrs, vec![backend_b_addr]);
    assert_eq!(backend.resolution.refresh_generation, 2);
}

#[test]
#[serial]
fn passive_request_failures_mark_backend_unhealthy_and_shift_selection_to_healthy_alternative() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BackendLifecycleHarness::new();
    let port = reserve_unused_udp_port();
    let (backend_a_bind, backend_b_bind) = alternate_loopback_backend_addrs(port);

    let backend_a_requests = Arc::new(AtomicUsize::new(0));
    let backend_b_requests = Arc::new(AtomicUsize::new(0));
    let backend_a_failing = Arc::new(AtomicBool::new(false));

    let backend_a_counter = Arc::clone(&backend_a_requests);
    let backend_a_failure = Arc::clone(&backend_a_failing);
    let backend_a_addr = harness
        .try_start_h1_backend_at(
            backend_a_bind,
            move |_req: hyper::Request<Incoming>| {
                let backend_a_counter = Arc::clone(&backend_a_counter);
                let backend_a_failure = Arc::clone(&backend_a_failure);
                async move {
                    backend_a_counter.fetch_add(1, Ordering::Relaxed);
                    let mut response = Response::builder();
                    if backend_a_failure.load(Ordering::Relaxed) {
                        response = response.status(503);
                    }
                    Ok::<_, Infallible>(
                        response
                            .body(Full::new(Bytes::from_static(b"backend-a")))
                            .expect("backend a response"),
                    )
                }
            },
        )
        .ok();
    let Some(backend_a_addr) = backend_a_addr else {
        return;
    };

    let backend_b_counter = Arc::clone(&backend_b_requests);
    let backend_b_addr = harness
        .try_start_h1_backend_at(
            backend_b_bind,
            move |_req: hyper::Request<Incoming>| {
                let backend_b_counter = Arc::clone(&backend_b_counter);
                async move {
                    backend_b_counter.fetch_add(1, Ordering::Relaxed);
                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(
                        b"backend-b",
                    ))))
                }
            },
        )
        .ok();
    let Some(backend_b_addr) = backend_b_addr else {
        return;
    };

    let config = harness.make_config(HashMap::from([(
        "api".to_string(),
        consistent_hash_upstream(vec![
            Backend {
                id: "backend-a".to_string(),
                address: format!("http://{backend_a_addr}"),
                weight: 1,
                health_check: None,
            },
            Backend {
                id: "backend-b".to_string(),
                address: format!("http://{backend_b_addr}"),
                weight: 1,
                health_check: None,
            },
        ]),
    )]));
    harness.start_listener(config).expect("listener");

    let backend_a_key = find_consistent_hash_key_for_backend(&harness, "backend-a")
        .expect("consistent-hash key should map to backend A");
    let healthy_summary = harness
        .upstream_membership_summary("api")
        .expect("membership summary");
    assert_eq!(healthy_summary.total_backends, 2);
    assert_eq!(healthy_summary.healthy_backends, 2);

    let baseline_a = backend_a_requests.load(Ordering::Relaxed);
    let baseline_b = backend_b_requests.load(Ordering::Relaxed);
    backend_a_failing.store(true, Ordering::Relaxed);

    for attempt in 1..=3 {
        let response = run_keyed_request(&harness, &backend_a_key).expect("failing keyed request");
        response.assert_status(503);
        response.assert_body_text("backend-a");

        let backend_state = harness
            .backend_snapshot(&format!("http://{backend_a_addr}"))
            .expect("backend snapshot lookup")
            .expect("backend lifecycle snapshot");
        if attempt < 3 {
            assert!(
                !matches!(
                    backend_state.health,
                    spooky_edge::runtime::backend::state::BackendHealthState::Unhealthy { .. }
                ),
                "backend should stay healthy until the passive failure threshold is crossed"
            );
        }
    }

    let backend_a_state = harness
        .backend_snapshot(&format!("http://{backend_a_addr}"))
        .expect("backend A snapshot lookup")
        .expect("backend A lifecycle snapshot");
    assert!(
        matches!(
            backend_a_state.health,
            spooky_edge::runtime::backend::state::BackendHealthState::Unhealthy { .. }
        ),
        "passive request failures should mark backend A unhealthy after the threshold"
    );

    let runtime_state = harness
        .upstream_backend_runtime_state("api", &format!("http://{backend_a_addr}"))
        .expect("backend runtime state lookup")
        .expect("backend runtime state");
    assert!(
        !runtime_state.healthy,
        "upstream pool runtime state should reflect the passive unhealthy transition"
    );

    let rerouted = run_keyed_request(&harness, &backend_a_key).expect("rerouted keyed request");
    rerouted.assert_status(200);
    rerouted.assert_body_text("backend-b");

    assert_eq!(
        backend_a_requests.load(Ordering::Relaxed) - baseline_a,
        3,
        "unhealthy backend should stop receiving the keyed traffic once passive ejection occurs"
    );
    assert_eq!(
        backend_b_requests.load(Ordering::Relaxed) - baseline_b,
        1,
        "healthy alternative should take over the keyed traffic after passive ejection"
    );

    let summary = harness
        .upstream_membership_summary("api")
        .expect("membership summary after failure");
    assert_eq!(summary.total_backends, 2);
    assert_eq!(summary.healthy_backends, 1);
}

#[test]
#[serial]
fn active_health_recovery_restores_backend_availability() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BackendLifecycleHarness::new();
    let (backend_addr, backend_fixture) = harness.start_h1_fail_then_recover_backend(
        b"backend-a",
        http::StatusCode::SERVICE_UNAVAILABLE,
        b"backend-a",
    );

    let config = harness.make_config(HashMap::from([(
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
                id: "backend-a".to_string(),
                address: format!("http://{backend_addr}"),
                weight: 1,
                health_check: Some(HealthCheck {
                    path: "/".to_string(),
                    interval: 50,
                    timeout_ms: 100,
                    failure_threshold: 1,
                    success_threshold: 1,
                    cooldown_ms: 1,
                }),
            }],
        },
    )]));
    harness.start_listener(config).expect("listener");

    let backend_identity = format!("http://{backend_addr}");
    assert!(
        wait_for_backend_health_state(
            &harness,
            &backend_identity,
            1,
            |health| !matches!(health, BackendHealthState::Unhealthy { .. }),
        ),
        "backend should start available before active health transitions run"
    );

    let initial = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("initial request");
    initial.assert_status(200);
    initial.assert_body_text("backend-a");

    backend_fixture.set_failing(true);
    assert!(
        wait_for_backend_health_state(
            &harness,
            &backend_identity,
            0,
            |_| true,
        ),
        "active health checks should mark the backend unhealthy after it begins failing"
    );

    let unhealthy_state = harness
        .upstream_backend_runtime_state("api", &backend_identity)
        .expect("backend runtime state lookup")
        .expect("backend runtime state");
    assert!(
        !unhealthy_state.healthy,
        "membership state should reflect the active-health unhealthy transition"
    );

    backend_fixture.set_failing(false);
    assert!(
        wait_for_backend_health_state(
            &harness,
            &backend_identity,
            1,
            |health| !matches!(health, BackendHealthState::Unhealthy { .. }),
        ),
        "active health checks should restore backend availability after recovery"
    );

    let recovered_state = harness
        .upstream_backend_runtime_state("api", &backend_identity)
        .expect("backend runtime state lookup")
        .expect("backend runtime state");
    assert!(
        recovered_state.healthy,
        "membership state should return to healthy after active recovery"
    );

    let recovered_snapshot = harness
        .backend_snapshot(&backend_identity)
        .expect("backend snapshot lookup")
        .expect("backend lifecycle snapshot");
    assert!(
        !matches!(recovered_snapshot.health, BackendHealthState::Unhealthy { .. }),
        "lifecycle snapshot should no longer report the backend as unhealthy after recovery"
    );

    let recovered = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("request after recovery");
    recovered.assert_status(200);
    recovered.assert_body_text("backend-a");
}
