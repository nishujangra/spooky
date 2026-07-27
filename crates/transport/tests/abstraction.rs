//! Transport facade contract tests.
//!
//! These assertions lock the edge-facing behavior of the transport layer while
//! keeping H1/H2 implementation details hidden behind the canonical facade.

mod support;

use std::{collections::HashMap, sync::Arc, time::Duration};

use bytes::Bytes;
use spooky_config::{
    config::{
        ClientAuth, Config, Listen, LoadBalancing, Log, Observability, Performance, Resilience,
        RouteMatch, Security, Tls, Upstream, UpstreamTls,
    },
    runtime::{RuntimeBackendTransportKind, RuntimeConfig},
};
use spooky_errors::{PoolError, ProxyError};
use spooky_transport::{SharedDnsResolver, UpstreamTransportPool};

use crate::support::{
    ConcurrencyTracker, build_pool, build_pool_with_policy, connection_policy,
    loopback_bind_restricted, read_body, request, reserve_unused_port, start_h1_server,
    start_h2_server,
};

fn transport_test_config(http_backend: &str, https_backend: &str) -> Config {
    let mut config = Config {
        version: 1,
        listen: Listen {
            protocol: "http3".to_string(),
            port: 443,
            address: "0.0.0.0".to_string(),
            tls: Tls {
                cert: "/tmp/tls/default.pem".to_string(),
                key: "/tmp/tls/default.key".to_string(),
                certificates: Vec::new(),
                client_auth: ClientAuth::default(),
            },
        },
        listeners: Vec::new(),
        upstream: HashMap::new(),
        load_balancing: None,
        upstream_tls: UpstreamTls::default(),
        log: Log::default(),
        performance: Performance::default(),
        observability: Observability::default(),
        resilience: Resilience::default(),
        security: Security::default(),
    };

    for (name, route_host, backend) in [
        ("plain", "plain.example.com", http_backend),
        ("secure", "secure.example.com", https_backend),
    ] {
        config.upstream.insert(
            name.to_string(),
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
                    host: Some(route_host.to_string()),
                    path_prefix: Some("/".to_string()),
                    method: None,
                },
                backends: vec![spooky_config::config::Backend {
                    id: format!("{name}-1"),
                    address: backend.to_string(),
                    weight: 100,
                    health_check: None,
                }],
            },
        );
    }

    config
}

#[test]
fn runtime_upstream_interpretation_selects_protocol_without_caller_branching() {
    let config = transport_test_config("http://127.0.0.1:8080", "https://127.0.0.1:8443");
    let runtime = RuntimeConfig::from_config(&config).expect("runtime config");

    let pool = UpstreamTransportPool::from_runtime_upstreams(
        runtime.upstreams.values(),
        &runtime.policies.transport.backend_connections,
        SharedDnsResolver::new(),
        None,
    )
    .expect("transport pool");

    let h1_rotation = pool
        .rotate_backend_client("http://127.0.0.1:8080")
        .expect("h1 rotation");
    assert!(h1_rotation.rotated());
    assert_eq!(h1_rotation.generations(), None);

    let h2_rotation = pool
        .rotate_backend_client("https://127.0.0.1:8443")
        .expect("h2 rotation");
    assert!(h2_rotation.rotated());
    assert_eq!(h2_rotation.generations(), Some((0, 1)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_facade_executes_requests_without_protocol_specific_callers() {
    let h1_port = match start_h1_server(b"h1", Duration::ZERO, None).await {
        Ok(port) => port,
        Err(err) if loopback_bind_restricted(&err) => return,
        Err(err) => panic!("failed to start h1 server: {err}"),
    };
    let h2_port = match start_h2_server(b"h2", Duration::ZERO, None).await {
        Ok(port) => port,
        Err(err) if loopback_bind_restricted(&err) => return,
        Err(err) => panic!("failed to start h2 server: {err}"),
    };

    let h1_backend = "h1-backend".to_string();
    let h2_backend = "h2-backend".to_string();
    let pool = build_pool(
        [
            (h1_backend.clone(), RuntimeBackendTransportKind::Http1),
            (h2_backend.clone(), RuntimeBackendTransportKind::H2),
        ],
        4,
        SharedDnsResolver::new(),
    );

    let h1_response = pool
        .send_backend_request(
            &h1_backend,
            request(&format!("http://127.0.0.1:{h1_port}/")),
        )
        .await
        .expect("h1 response");
    assert_eq!(read_body(h1_response).await, Bytes::from_static(b"h1"));

    let h2_response = pool
        .send_backend_request(
            &h2_backend,
            request(&format!("http://127.0.0.1:{h2_port}/")),
        )
        .await
        .expect("h2 response");
    assert_eq!(read_body(h2_response).await, Bytes::from_static(b"h2"));
}

#[test]
fn transport_facade_exposes_stable_rotation_contract_across_protocols() {
    let pool = build_pool(
        [
            ("h1-backend".to_string(), RuntimeBackendTransportKind::Http1),
            ("h2-backend".to_string(), RuntimeBackendTransportKind::H2),
        ],
        4,
        SharedDnsResolver::new(),
    );

    let h1_rotation = pool
        .rotate_backend_client("h1-backend")
        .expect("h1 rotation");
    assert!(h1_rotation.rotated());
    assert_eq!(h1_rotation.generations(), None);

    let h2_rotation = pool
        .rotate_backend_client("h2-backend")
        .expect("h2 rotation");
    assert!(h2_rotation.rotated());
    assert_eq!(h2_rotation.generations(), Some((0, 1)));

    let h2_second_rotation = pool
        .rotate_backend_client("h2-backend")
        .expect("second h2 rotation");
    assert!(h2_second_rotation.rotated());
    assert_eq!(h2_second_rotation.generations(), Some((1, 2)));

    let missing_rotation = pool
        .rotate_backend_client("missing")
        .expect("missing rotation should not error");
    assert!(!missing_rotation.rotated());
    assert_eq!(missing_rotation.generations(), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_facade_maps_unknown_backend_send_failure_and_overload_consistently() {
    let unknown_pool = build_pool(
        [("known".to_string(), RuntimeBackendTransportKind::Http1)],
        1,
        SharedDnsResolver::new(),
    );
    let unknown_err = unknown_pool
        .send_backend_request("missing", request("http://127.0.0.1:1/"))
        .await
        .expect_err("missing backend should fail");
    assert!(matches!(
        unknown_err,
        ProxyError::Pool(PoolError::UnknownBackend(name)) if name == "missing"
    ));

    let failing_h1_pool = build_pool(
        [("h1-fail".to_string(), RuntimeBackendTransportKind::Http1)],
        1,
        SharedDnsResolver::new(),
    );
    let h1_port = reserve_unused_port();
    let h1_err = failing_h1_pool
        .send_backend_request("h1-fail", request(&format!("http://127.0.0.1:{h1_port}/")))
        .await
        .expect_err("h1 send should fail");
    assert!(matches!(h1_err, ProxyError::Pool(PoolError::Send(_))));

    let failing_h2_pool = build_pool(
        [("h2-fail".to_string(), RuntimeBackendTransportKind::H2)],
        1,
        SharedDnsResolver::new(),
    );
    let h2_port = reserve_unused_port();
    let h2_err = failing_h2_pool
        .send_backend_request("h2-fail", request(&format!("http://127.0.0.1:{h2_port}/")))
        .await
        .expect_err("h2 send should fail");
    assert!(matches!(h2_err, ProxyError::Pool(PoolError::Send(_))));

    let h1_tracker = Arc::new(ConcurrencyTracker::new());
    let h1_overload_port = match start_h1_server(
        b"h1",
        Duration::from_millis(50),
        Some(Arc::clone(&h1_tracker)),
    )
    .await
    {
        Ok(port) => port,
        Err(err) if loopback_bind_restricted(&err) => return,
        Err(err) => panic!("failed to start h1 overload server: {err}"),
    };
    let h1_overload_pool = Arc::new(build_pool(
        [(
            "h1-overload".to_string(),
            RuntimeBackendTransportKind::Http1,
        )],
        1,
        SharedDnsResolver::new(),
    ));
    let h1_task_pool = Arc::clone(&h1_overload_pool);
    let h1_task = tokio::spawn(async move {
        h1_task_pool
            .send_backend_request(
                "h1-overload",
                request(&format!("http://127.0.0.1:{h1_overload_port}/")),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let h1_overload = h1_overload_pool
        .send_backend_request(
            "h1-overload",
            request(&format!("http://127.0.0.1:{h1_overload_port}/")),
        )
        .await;
    assert!(matches!(
        h1_overload,
        Err(ProxyError::Pool(PoolError::BackendOverloaded(_)))
    ));
    let _ = h1_task.await.expect("h1 task join");
    assert_eq!(
        h1_tracker.max_observed(),
        1,
        "tracked h1 overload fixture should observe only one admitted request"
    );

    let h2_tracker = Arc::new(ConcurrencyTracker::new());
    let h2_overload_port = match start_h2_server(
        b"h2",
        Duration::from_millis(50),
        Some(Arc::clone(&h2_tracker)),
    )
    .await
    {
        Ok(port) => port,
        Err(err) if loopback_bind_restricted(&err) => return,
        Err(err) => panic!("failed to start h2 overload server: {err}"),
    };
    let h2_overload_pool = Arc::new(build_pool(
        [("h2-overload".to_string(), RuntimeBackendTransportKind::H2)],
        1,
        SharedDnsResolver::new(),
    ));
    let h2_task_pool = Arc::clone(&h2_overload_pool);
    let h2_task = tokio::spawn(async move {
        h2_task_pool
            .send_backend_request(
                "h2-overload",
                request(&format!("http://127.0.0.1:{h2_overload_port}/")),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let h2_overload = h2_overload_pool
        .send_backend_request(
            "h2-overload",
            request(&format!("http://127.0.0.1:{h2_overload_port}/")),
        )
        .await;
    assert!(matches!(
        h2_overload,
        Err(ProxyError::Pool(PoolError::BackendOverloaded(_)))
    ));
    let _ = h2_task.await.expect("h2 task join");
    assert_eq!(
        h2_tracker.max_observed(),
        1,
        "tracked h2 overload fixture should observe only one admitted request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_facade_maps_execution_timeouts_to_proxy_timeout_across_protocols() {
    let h1_port = match start_h1_server(b"h1", Duration::from_millis(50), None).await {
        Ok(port) => port,
        Err(err) if loopback_bind_restricted(&err) => return,
        Err(err) => panic!("failed to start h1 timeout server: {err}"),
    };
    let h2_port = match start_h2_server(b"h2", Duration::from_millis(50), None).await {
        Ok(port) => port,
        Err(err) if loopback_bind_restricted(&err) => return,
        Err(err) => panic!("failed to start h2 timeout server: {err}"),
    };

    let timeout_policy = spooky_config::runtime::RuntimeBackendConnectionPolicy {
        execution_timeout: Duration::from_millis(5),
        ..connection_policy(4)
    };

    let pool = build_pool_with_policy(
        [
            ("h1-timeout".to_string(), RuntimeBackendTransportKind::Http1),
            ("h2-timeout".to_string(), RuntimeBackendTransportKind::H2),
        ],
        timeout_policy,
        SharedDnsResolver::new(),
    );

    let h1_err = pool
        .send_backend_request(
            "h1-timeout",
            request(&format!("http://127.0.0.1:{h1_port}/")),
        )
        .await
        .expect_err("h1 execution should time out");
    assert!(matches!(h1_err, ProxyError::Timeout));

    let h2_err = pool
        .send_backend_request(
            "h2-timeout",
            request(&format!("http://127.0.0.1:{h2_port}/")),
        )
        .await
        .expect_err("h2 execution should time out");
    assert!(matches!(h2_err, ProxyError::Timeout));
}

#[test]
fn transport_facade_preserves_dns_refresh_rotation_contract() {
    let resolver = SharedDnsResolver::new();
    let backend = "https://api.example.com:443".to_string();
    let pool = build_pool(
        [(backend.clone(), RuntimeBackendTransportKind::H2)],
        4,
        resolver.clone(),
    );

    resolver.set_host_addrs(
        "api.example.com",
        [std::net::SocketAddr::from(([127, 0, 0, 10], 443))],
    );
    let first_rotation = pool
        .rotate_backend_client(&backend)
        .expect("first rotation");
    assert!(first_rotation.rotated());
    assert_eq!(first_rotation.generations(), Some((0, 1)));

    resolver.set_host_addrs(
        "api.example.com",
        [std::net::SocketAddr::from(([127, 0, 0, 11], 443))],
    );
    let second_rotation = pool
        .rotate_backend_client(&backend)
        .expect("second rotation");
    assert!(second_rotation.rotated());
    assert_eq!(second_rotation.generations(), Some((1, 2)));

    assert_eq!(
        resolver.cached_addrs("api.example.com"),
        Some(vec![std::net::SocketAddr::from(([127, 0, 0, 11], 443))])
    );
}
