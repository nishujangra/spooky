use std::collections::HashMap;

use serial_test::serial;
use spooky_config::config::UpstreamTls;

mod support;

use support::{
    net::local_listener_bind_available,
    request_path::{H3RequestSpec, QuicRequestPathHarness, make_backend, make_upstream},
};

#[test]
#[serial]
fn request_path_harness_supports_h1_upstream_fixture_round_trip() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = QuicRequestPathHarness::new();
    let backend_addr = harness.start_h1_static_backend(b"h1 harness ok");

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "api".to_string(),
        make_upstream(
            "/",
            vec![make_backend("h1-1", format!("http://{backend_addr}"))],
            None,
            "round-robin",
        ),
    );

    harness
        .start_listener(harness.make_config(upstreams))
        .expect("listener");

    let response = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("h3 request");
    response.assert_status(200);
    response.assert_body_bytes(b"h1 harness ok");
}

#[test]
#[serial]
fn request_path_harness_supports_h2_upstream_fixture_round_trip() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = QuicRequestPathHarness::new();
    let backend_addr = harness.start_h2_static_backend(b"h2 harness ok");

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "api".to_string(),
        make_upstream(
            "/",
            vec![make_backend("h2-1", format!("https://{backend_addr}"))],
            Some(UpstreamTls {
                verify_certificates: false,
                strict_sni: false,
                ..UpstreamTls::default()
            }),
            "round-robin",
        ),
    );

    harness
        .start_listener(harness.make_config(upstreams))
        .expect("listener");

    let response = harness
        .run_request(H3RequestSpec::get("localhost", "/"))
        .expect("h3 request");
    response.assert_status(200);
    response.assert_body_text("h2 harness ok");
}
