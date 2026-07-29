use std::collections::HashMap;

use serial_test::serial;
use spooky_edge::runtime::backend::event::BackendRefreshOutcome;

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
