//! HTTP/1-backed transport facade contract tests.

mod support;

use std::{sync::Arc, time::Duration};

use spooky_config::runtime::RuntimeBackendTransportKind;
use spooky_errors::{PoolError, ProxyError};
use spooky_transport::{SharedDnsResolver, UpstreamTransportPool};

use crate::support::{
    ConcurrencyTracker, connection_policy, loopback_bind_restricted, request, start_h1_server,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http1_transport_enforces_per_backend_inflight_contract() {
    let tracker = Arc::new(ConcurrencyTracker::new());
    let port =
        match start_h1_server(b"ok", Duration::from_millis(50), Some(Arc::clone(&tracker))).await {
            Ok(port) => port,
            Err(err) if loopback_bind_restricted(&err) => return,
            Err(err) => panic!("failed to start h1 test server: {err}"),
        };
    let backend = format!("127.0.0.1:{port}");

    let pool = Arc::new(
        UpstreamTransportPool::new_from_runtime_backends(
            [(backend.clone(), RuntimeBackendTransportKind::Http1)],
            std::collections::HashMap::new(),
            connection_policy(1),
            SharedDnsResolver::new(),
        )
        .expect("pool"),
    );
    let req1 = request(&format!("http://{backend}/"));
    let req2 = request(&format!("http://{backend}/"));

    let pool1 = pool.clone();
    let backend1 = backend.clone();
    let r1 = tokio::spawn(async move { pool1.send_backend_request(&backend1, req1).await });

    let pool2 = pool.clone();
    let backend2 = backend.clone();
    let r2 = tokio::spawn(async move { pool2.send_backend_request(&backend2, req2).await });

    let (r1, r2) = tokio::join!(r1, r2);
    let r1 = r1.expect("first request join");
    let r2 = r2.expect("second request join");
    assert!(
        r1.is_ok() || r2.is_ok(),
        "at least one HTTP/1 request should be admitted"
    );
    assert!(
        matches!(r1, Err(ProxyError::Pool(PoolError::BackendOverloaded(_))))
            || matches!(r2, Err(ProxyError::Pool(PoolError::BackendOverloaded(_)))),
        "one HTTP/1 request should be rejected by per-backend inflight admission"
    );
    assert_eq!(
        tracker.max_observed(),
        1,
        "only one HTTP/1 request should reach the backend at a time"
    );
}

#[tokio::test]
async fn http1_transport_rejects_unknown_backend_at_facade_boundary() {
    let pool = UpstreamTransportPool::new_from_runtime_backends(
        [(
            "127.0.0.1:12345".to_string(),
            RuntimeBackendTransportKind::Http1,
        )],
        std::collections::HashMap::new(),
        connection_policy(1),
        SharedDnsResolver::new(),
    )
    .expect("pool");
    let req = request("http://127.0.0.1:12345/");

    let err = pool
        .send_backend_request("127.0.0.1:9999", req)
        .await
        .expect_err("unknown backend should fail");
    match err {
        ProxyError::Pool(PoolError::UnknownBackend(name)) => {
            assert_eq!(name, "127.0.0.1:9999")
        }
        _ => panic!("unexpected error contract for missing HTTP/1 backend"),
    }
}

#[tokio::test]
async fn http1_transport_reports_backend_overload_when_inflight_is_exhausted() {
    let tracker = Arc::new(ConcurrencyTracker::new());
    let port = match start_h1_server(b"ok", Duration::from_millis(50), Some(tracker)).await {
        Ok(port) => port,
        Err(err) if loopback_bind_restricted(&err) => return,
        Err(err) => panic!("failed to start h1 test server: {err}"),
    };
    let backend = format!("127.0.0.1:{port}");
    let pool = Arc::new(
        UpstreamTransportPool::new_from_runtime_backends(
            [(backend.clone(), RuntimeBackendTransportKind::Http1)],
            std::collections::HashMap::new(),
            connection_policy(1),
            SharedDnsResolver::new(),
        )
        .expect("pool"),
    );

    let req1 = request(&format!("http://{backend}/"));
    let req2 = request(&format!("http://{backend}/"));

    let pool_task = Arc::clone(&pool);
    let backend_task = backend.clone();
    let handle =
        tokio::spawn(async move { pool_task.send_backend_request(&backend_task, req1).await });

    tokio::time::sleep(Duration::from_millis(10)).await;
    let overload = pool.send_backend_request(&backend, req2).await;
    assert!(matches!(
        overload,
        Err(ProxyError::Pool(PoolError::BackendOverloaded(_)))
    ));

    let _ = handle.await.expect("request task join");
}

#[test]
fn http1_transport_rotation_contract_is_effective_for_known_backends_and_noop_for_missing_ones() {
    let pool = UpstreamTransportPool::new_from_runtime_backends(
        [(
            "127.0.0.1:12345".to_string(),
            RuntimeBackendTransportKind::Http1,
        )],
        std::collections::HashMap::new(),
        connection_policy(1),
        SharedDnsResolver::new(),
    )
    .expect("pool");

    assert!(
        pool.rotate_backend_client("127.0.0.1:12345")
            .expect("known backend")
            .rotated()
    );
    assert!(
        !pool
            .rotate_backend_client("127.0.0.1:9999")
            .expect("unknown backend should be ignored")
            .rotated()
    );
}
