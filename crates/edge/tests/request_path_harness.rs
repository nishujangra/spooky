use std::collections::HashMap;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Response, body::Incoming};
use serial_test::serial;
use spooky_config::config::UpstreamTls;

mod support;

use support::{
    net::local_listener_bind_available,
    request_path::{H3RequestSpec, QuicRequestPathHarness, make_backend, make_upstream},
};

fn response_line(body: &str, prefix: &str) -> String {
    body.lines()
        .find_map(|line| line.strip_prefix(prefix).map(str::to_string))
        .unwrap_or_else(|| panic!("missing response line with prefix `{prefix}` in body: {body}"))
}

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

#[test]
#[serial]
fn quic_to_h1_success_path_normalizes_headers_and_keeps_get_bodyless() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = QuicRequestPathHarness::new();
    let backend_addr = harness.start_h1_backend(|req: hyper::Request<Incoming>| async move {
        let header_value = |name: &str| {
            req.headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>")
                .to_string()
        };
        let method = req.method().to_string();
        let path = req.uri().path().to_string();
        let host = header_value("host");
        let forwarded = header_value("forwarded");
        let xff = header_value("x-forwarded-for");
        let xfp = header_value("x-forwarded-proto");
        let xfh = header_value("x-forwarded-host");
        let user_agent = header_value("user-agent");
        let request_id = header_value("x-request-id");
        let content_length = req
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("<missing>")
            .to_string();
        let has_connection = req.headers().contains_key("connection");
        let body = req
            .into_body()
            .collect()
            .await
            .expect("collect request body")
            .to_bytes();
        let body = format!(
            "method={method}\npath={path}\nhost={}\nforwarded={}\nxff={}\nxfp={}\nxfh={}\nuser_agent={}\nx_request_id={}\ncontent_length={content_length}\nbody_len={}\nhas_connection={}\n",
            host,
            forwarded,
            xff,
            xfp,
            xfh,
            user_agent,
            request_id,
            body.len(),
            has_connection,
        );
        Ok::<_, std::convert::Infallible>(Response::new(Full::new(Bytes::from(body))))
    });

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "api".to_string(),
        make_upstream(
            "/headers",
            vec![make_backend("h1-headers", format!("http://{backend_addr}"))],
            None,
            "round-robin",
        ),
    );

    harness
        .start_listener(harness.make_config(upstreams))
        .expect("listener");

    let response = harness
        .run_request(H3RequestSpec {
            method: "GET",
            authority: "public.example.com",
            path: "/headers",
            headers: &[
                ("forwarded", "for=1.2.3.4;proto=http;host=\"evil.example\""),
                ("x-forwarded-for", "1.2.3.4"),
                ("x-forwarded-proto", "http"),
                ("x-forwarded-host", "evil.example"),
                ("connection", "keep-alive, x-secret"),
                ("x-secret", "strip-me"),
            ],
            body: None,
            user_agent: "spooky-success-h1",
        })
        .expect("h3 request");

    response.assert_status(200);
    let body = response.body_text();
    assert_eq!(response_line(&body, "method="), "GET");
    assert_eq!(response_line(&body, "path="), "/headers");
    assert_eq!(response_line(&body, "host="), "public.example.com");
    assert_eq!(
        response_line(&body, "forwarded="),
        "for=127.0.0.1;proto=https;host=\"public.example.com\""
    );
    assert_eq!(response_line(&body, "xff="), "127.0.0.1");
    assert_eq!(response_line(&body, "xfp="), "https");
    assert_eq!(response_line(&body, "xfh="), "public.example.com");
    assert_eq!(response_line(&body, "user_agent="), "spooky-success-h1");
    assert_eq!(response_line(&body, "content_length="), "<missing>");
    assert_eq!(response_line(&body, "body_len="), "0");
    assert_eq!(response_line(&body, "has_connection="), "false");

    let request_id = response_line(&body, "x_request_id=");
    assert!(
        !request_id.is_empty() && request_id.chars().all(|ch| ch.is_ascii_digit()),
        "x-request-id should be a generated numeric request identifier, got `{request_id}`"
    );
}

#[test]
#[serial]
fn quic_to_h1_success_path_streams_response_body_to_completion() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = QuicRequestPathHarness::new();
    let backend_addr = harness.start_h1_chunked_backend(vec![b"chunk-1:", b"chunk-2:", b"chunk-3"]);

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "api".to_string(),
        make_upstream(
            "/stream",
            vec![make_backend("h1-stream", format!("http://{backend_addr}"))],
            None,
            "round-robin",
        ),
    );

    harness
        .start_listener(harness.make_config(upstreams))
        .expect("listener");

    let response = harness
        .run_request(H3RequestSpec::get("stream.example.com", "/stream"))
        .expect("h3 request");
    response.assert_status(200);
    response.assert_body_text("chunk-1:chunk-2:chunk-3");
}
