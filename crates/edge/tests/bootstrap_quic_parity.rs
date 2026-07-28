use std::{collections::HashMap, convert::Infallible};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming};

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
