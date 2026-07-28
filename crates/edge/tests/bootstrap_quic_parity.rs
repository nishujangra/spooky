use std::{collections::HashMap, convert::Infallible};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming};
use spooky_config::config::{LoadBalancing, RouteMatch, Upstream};

mod support;

use support::{
    net::local_listener_bind_available,
    parity::{BootstrapQuicParityHarness, ParityRequestSpec, make_backend, make_upstream},
};

#[test]
fn bootstrap_and_quic_parity_harness_collects_canonical_response_shape() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BootstrapQuicParityHarness::new();
    let backend_addr = harness.start_h1_backend(|_req: Request<Incoming>| async move {
        Ok::<_, Infallible>(
            Response::builder()
                .status(200)
                .header("x-parity", "ok")
                .body(Full::new(Bytes::from_static(b"parity ok\n")))
                .expect("response"),
        )
    });

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "api".to_string(),
        make_upstream(
            "/",
            vec![make_backend("backend-a", backend_addr.to_string())],
            None,
            "round-robin",
        ),
    );

    let config = harness.make_config(upstreams);
    harness.start_listener(config).expect("listener with bootstrap");

    let mut request = ParityRequestSpec::get("localhost", "/");
    request.selected_response_headers = &["x-parity"];
    request.capture_metrics_delta = true;

    let pair = harness.run_parity_pair(request).expect("parity pair");

    assert_eq!(pair.quic.response.status, 200);
    assert_eq!(pair.bootstrap.response.status, 200);
    assert_eq!(pair.quic.response.body, pair.bootstrap.response.body);
    assert_eq!(pair.quic.response.body, b"parity ok\n");
    assert_eq!(pair.quic.response.selected_headers, pair.bootstrap.response.selected_headers);
    assert_eq!(
        pair.quic.response.selected_headers,
        vec![(String::from("x-parity"), String::from("ok"))]
    );
    assert!(pair.quic.metrics_delta.is_some());
    assert!(pair.bootstrap.metrics_delta.is_some());
}

#[test]
fn bootstrap_and_quic_route_resolution_choose_the_same_route_and_backend_result() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BootstrapQuicParityHarness::new();
    let payments_get = harness.start_h1_backend(|_req: Request<Incoming>| async move {
        Ok::<_, Infallible>(
            Response::builder()
                .status(200)
                .header("x-backend", "payments-get")
                .body(Full::new(Bytes::from_static(b"payments get\n")))
                .expect("response"),
        )
    });
    let payments_post = harness.start_h1_backend(|_req: Request<Incoming>| async move {
        Ok::<_, Infallible>(
            Response::builder()
                .status(200)
                .header("x-backend", "payments-post")
                .body(Full::new(Bytes::from_static(b"payments post\n")))
                .expect("response"),
        )
    });
    let admin_get = harness.start_h1_backend(|_req: Request<Incoming>| async move {
        Ok::<_, Infallible>(
            Response::builder()
                .status(200)
                .header("x-backend", "admin-get")
                .body(Full::new(Bytes::from_static(b"admin get\n")))
                .expect("response"),
        )
    });

    let upstreams = HashMap::from([
        (
            "payments-get".to_string(),
            routed_upstream(
                Some("api.example.com"),
                "/payments",
                Some("GET"),
                "round-robin",
                vec![make_backend("payments-get-a", payments_get.to_string())],
            ),
        ),
        (
            "payments-post".to_string(),
            routed_upstream(
                Some("api.example.com"),
                "/payments",
                Some("POST"),
                "round-robin",
                vec![make_backend("payments-post-a", payments_post.to_string())],
            ),
        ),
        (
            "admin-get".to_string(),
            routed_upstream(
                Some("admin.example.com"),
                "/admin",
                Some("GET"),
                "round-robin",
                vec![make_backend("admin-get-a", admin_get.to_string())],
            ),
        ),
    ]);

    let config = harness.make_config(upstreams);
    harness.start_listener(config).expect("listener with bootstrap");

    let cases = [
        ("GET", "api.example.com", "/payments/charge", "payments get\n", "payments-get"),
        ("POST", "api.example.com", "/payments/charge", "payments post\n", "payments-post"),
        ("GET", "admin.example.com", "/admin/audit", "admin get\n", "admin-get"),
    ];

    for (method, authority, path, expected_body, expected_backend) in cases {
        let request = ParityRequestSpec {
            method,
            authority,
            path,
            headers: &[],
            body: None,
            user_agent: "spooky-bootstrap-quic-parity-test",
            selected_response_headers: &["x-backend"],
            capture_metrics_delta: false,
        };

        let pair = harness.run_parity_pair(request).expect("parity pair");

        assert_eq!(
            pair.quic.response.status, 200,
            "quic route resolution should succeed for {method} {authority}{path}"
        );
        assert_eq!(
            pair.bootstrap.response.status, 200,
            "bootstrap route resolution should succeed for {method} {authority}{path}"
        );
        assert_eq!(
            pair.quic.response.body,
            expected_body.as_bytes(),
            "quic should hit the expected upstream-visible backend result for {method} {authority}{path}"
        );
        assert_eq!(
            pair.bootstrap.response.body,
            expected_body.as_bytes(),
            "bootstrap should hit the expected upstream-visible backend result for {method} {authority}{path}"
        );
        assert_eq!(
            pair.quic.response.selected_headers,
            vec![(String::from("x-backend"), expected_backend.to_string())],
            "quic should surface the selected backend marker for {method} {authority}{path}"
        );
        assert_eq!(
            pair.bootstrap.response.selected_headers,
            pair.quic.response.selected_headers,
            "bootstrap and quic should expose the same backend-selection result for {method} {authority}{path}"
        );
    }
}

#[test]
fn bootstrap_and_quic_route_resolution_share_observable_unrouted_behavior() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BootstrapQuicParityHarness::new();
    let backend_addr = harness.start_h1_static_backend(b"matched route\n");

    let upstreams = HashMap::from([(
        "api".to_string(),
        routed_upstream(
            Some("api.example.com"),
            "/payments",
            Some("GET"),
            "round-robin",
            vec![make_backend("backend-a", backend_addr.to_string())],
        ),
    )]);

    let config = harness.make_config(upstreams);
    harness.start_listener(config).expect("listener with bootstrap");

    let request = ParityRequestSpec::get("unknown.example.com", "/missing");
    let pair = harness.run_parity_pair(request).expect("unrouted parity pair");

    assert_eq!(pair.quic.response.status, 502);
    assert_eq!(pair.bootstrap.response.status, 502);
    assert_eq!(pair.quic.response.body, b"no route\n");
    assert_eq!(pair.bootstrap.response.body, pair.quic.response.body);
}

fn routed_upstream(
    host: Option<&str>,
    path_prefix: &str,
    method: Option<&str>,
    lb_type: &str,
    backends: Vec<spooky_config::config::Backend>,
) -> Upstream {
    let mut upstream = make_upstream(path_prefix, backends, None, lb_type);
    upstream.load_balancing = LoadBalancing {
        lb_type: lb_type.to_string(),
        key: None,
    };
    upstream.route = RouteMatch {
        host: host.map(str::to_string),
        path_prefix: Some(path_prefix.to_string()),
        method: method.map(str::to_string),
    };
    upstream
}
