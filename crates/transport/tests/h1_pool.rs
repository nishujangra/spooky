//! HTTP/1-backed transport facade contract tests.

mod support;

use std::{sync::Arc, time::Duration};

use hyper::{StatusCode, upgrade};
use hyper_util::rt::TokioIo;
use impulse_errors::{PoolError, ProxyError};
use impulse_transport::SharedDnsResolver;
use tokio::io::AsyncWriteExt;

use crate::support::{
    ConcurrencyTracker, TransportTestProtocol, build_single_backend_pool, loopback_bind_restricted,
    request_to_backend, start_h1_upgrade_server, start_shared_backend_pool,
    websocket_upgrade_request_to_backend,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http1_transport_enforces_per_backend_inflight_contract() {
    let tracker = Arc::new(ConcurrencyTracker::new());
    let (backend, pool) = match start_shared_backend_pool(
        TransportTestProtocol::Http1,
        b"ok",
        Duration::from_millis(50),
        Some(Arc::clone(&tracker)),
        1,
        SharedDnsResolver::new(),
    )
    .await
    {
        Ok(fixture) => fixture,
        Err(err) if loopback_bind_restricted(&err) => return,
        Err(err) => panic!("failed to start h1 test server: {err}"),
    };
    let req1 = request_to_backend(&backend);
    let req2 = request_to_backend(&backend);

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
    let pool = build_single_backend_pool(
        "127.0.0.1:12345".to_string(),
        TransportTestProtocol::Http1.runtime_kind(),
        1,
        SharedDnsResolver::new(),
    );
    let req = request_to_backend("127.0.0.1:12345");

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
    let (backend, pool) = match start_shared_backend_pool(
        TransportTestProtocol::Http1,
        b"ok",
        Duration::from_millis(50),
        Some(tracker),
        1,
        SharedDnsResolver::new(),
    )
    .await
    {
        Ok(fixture) => fixture,
        Err(err) if loopback_bind_restricted(&err) => return,
        Err(err) => panic!("failed to start h1 test server: {err}"),
    };

    let req1 = request_to_backend(&backend);
    let req2 = request_to_backend(&backend);

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
    let pool = build_single_backend_pool(
        "127.0.0.1:12345".to_string(),
        TransportTestProtocol::Http1.runtime_kind(),
        1,
        SharedDnsResolver::new(),
    );

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http1_upgrade_requests_flow_through_shared_transport_and_hold_inflight_until_tunnel_ends()
{
    let tracker = Arc::new(ConcurrencyTracker::new());
    let backend = match start_h1_upgrade_server(Some(Arc::clone(&tracker))).await {
        Ok(backend) => backend,
        Err(err) if loopback_bind_restricted(&err) => return,
        Err(err) => panic!("failed to start h1 upgrade test server: {err}"),
    };
    let pool = Arc::new(build_single_backend_pool(
        backend.clone(),
        TransportTestProtocol::Http1.runtime_kind(),
        1,
        SharedDnsResolver::new(),
    ));

    let mut upgrade_response = pool
        .send_http1_upgrade_request(&backend, websocket_upgrade_request_to_backend(&backend))
        .await
        .expect("upgrade response");
    assert_eq!(
        upgrade_response.response().status(),
        StatusCode::SWITCHING_PROTOCOLS
    );

    let on_upgrade = upgrade::on(upgrade_response.response_mut());
    let (_response, lease) = upgrade_response.into_parts();
    let upgraded = tokio::time::timeout(Duration::from_secs(1), on_upgrade)
        .await
        .expect("upgrade timeout")
        .expect("upgrade should succeed");
    let mut upgraded = TokioIo::new(upgraded);

    let overload = pool
        .send_backend_request(&backend, request_to_backend(&backend))
        .await;
    assert!(matches!(
        overload,
        Err(ProxyError::Pool(PoolError::BackendOverloaded(_)))
    ));

    upgraded
        .write_all(b"ping")
        .await
        .expect("write tunnel payload");
    drop(upgraded);
    drop(lease);
    tokio::time::sleep(Duration::from_millis(25)).await;

    let response = pool
        .send_backend_request(&backend, request_to_backend(&backend))
        .await
        .expect("request after tunnel close");
    let body = crate::support::read_body(response).await;
    assert_eq!(body, bytes::Bytes::from_static(b"ok"));
    assert_eq!(tracker.max_observed(), 1);
}
