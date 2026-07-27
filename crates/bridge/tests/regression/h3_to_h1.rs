//! h3→h1 request building: hop-header stripping, TE: trailers, content-length, parity.

use http::{
    HeaderMap, HeaderValue,
    header::{CONTENT_LENGTH, HOST, TE},
};
use quiche::h3::Header;
use spooky_bridge::request::build_h1_request;
use spooky_config::{
    backend_endpoint::BackendEndpoint,
    config::{ForwardedHeaderPolicy, UpstreamHostPolicy},
};

use crate::common::{RequestInputMeta, bridge_headers, request_input, request_target};

#[test]
fn plain_requests_strip_hop_headers_and_add_te_trailers() {
    let endpoint = BackendEndpoint::parse("backend.internal:443").expect("endpoint");
    let headers = vec![
        Header::new(b"host", b"spoofed.example.com"),
        Header::new(b"forwarded", b"for=1.2.3.4"),
        Header::new(b"x-forwarded-for", b"1.2.3.4"),
        Header::new(b"connection", b"keep-alive, x-secret"),
        Header::new(b"x-secret", b"drop-me"),
        Header::new(b"x-keep", b"ok"),
        Header::new(b"content-length", b"999"),
    ];

    let req = build_h1_request(
        request_target(
            &endpoint,
            &UpstreamHostPolicy::default(),
            &ForwardedHeaderPolicy::default(),
        ),
        request_input(
            "POST",
            "/submit",
            &headers,
            RequestInputMeta {
                authority: Some("api.example.com"),
                content_length: Some(12),
                request_id: 42,
                traceparent: Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
                client_addr: "203.0.113.10:44321".parse().expect("client"),
            },
        ),
    )
    .expect("request");

    assert_eq!(req.uri().to_string(), "https://backend.internal:443/submit");
    assert_eq!(
        req.headers()
            .get(HOST)
            .and_then(|value| value.to_str().ok()),
        Some("api.example.com")
    );
    assert_eq!(
        req.headers().get(TE).and_then(|value| value.to_str().ok()),
        Some("trailers")
    );
    assert_eq!(
        req.headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some("12")
    );
    assert_eq!(
        req.headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
        Some("203.0.113.10")
    );
    assert_eq!(
        req.headers()
            .get("forwarded")
            .and_then(|value| value.to_str().ok()),
        Some("for=203.0.113.10;proto=https;host=\"api.example.com\"")
    );
    assert_eq!(
        req.headers()
            .get("traceparent")
            .and_then(|value| value.to_str().ok()),
        Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
    );
    assert_eq!(
        req.headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok()),
        Some("42")
    );
    assert_eq!(
        req.headers()
            .get("x-keep")
            .and_then(|value| value.to_str().ok()),
        Some("ok")
    );
    assert!(req.headers().get("x-secret").is_none());
}

#[test]
fn legacy_websocket_requests_preserve_upgrade_headers() {
    let endpoint = BackendEndpoint::parse("http://backend.internal:8080").expect("endpoint");
    let headers = vec![
        Header::new(b"host", b"socket.example.com"),
        Header::new(b"connection", b"upgrade"),
        Header::new(b"upgrade", b"websocket"),
        Header::new(b"sec-websocket-key", b"dGhlIHNhbXBsZSBub25jZQ=="),
    ];

    let req = build_h1_request(
        request_target(
            &endpoint,
            &UpstreamHostPolicy::default(),
            &ForwardedHeaderPolicy::default(),
        ),
        request_input(
            "GET",
            "/ws",
            &headers,
            RequestInputMeta {
                authority: Some("socket.example.com"),
                content_length: None,
                request_id: 7,
                traceparent: None,
                client_addr: "198.51.100.12:5555".parse().expect("client"),
            },
        ),
    )
    .expect("request");

    assert_eq!(req.method(), http::Method::GET);
    assert_eq!(req.uri().to_string(), "http://backend.internal:8080/ws");
    assert_eq!(
        req.headers()
            .get("connection")
            .and_then(|value| value.to_str().ok()),
        Some("upgrade")
    );
    assert_eq!(
        req.headers()
            .get("upgrade")
            .and_then(|value| value.to_str().ok()),
        Some("websocket")
    );
    assert_eq!(
        req.headers()
            .get("sec-websocket-key")
            .and_then(|value| value.to_str().ok()),
        Some("dGhlIHNhbXBsZSBub25jZQ==")
    );
    assert!(req.headers().get(TE).is_none());
    assert!(req.headers().get(CONTENT_LENGTH).is_none());
}

#[test]
fn bootstrap_and_forwarding_header_shapes_match_for_h1() {
    let endpoint = BackendEndpoint::parse("backend.internal:443").expect("endpoint");
    let forwarding_headers = vec![
        Header::new(b"host", b"api.example.com"),
        Header::new(b"x-custom", b"ok"),
        Header::new(b"x-forwarded-for", b"1.2.3.4"),
    ];

    let mut bootstrap_headers_map = HeaderMap::new();
    bootstrap_headers_map.insert(HOST, HeaderValue::from_static("api.example.com"));
    bootstrap_headers_map.insert("x-custom", HeaderValue::from_static("ok"));
    bootstrap_headers_map.insert("x-forwarded-for", HeaderValue::from_static("1.2.3.4"));
    let bootstrap_headers = bridge_headers(&bootstrap_headers_map);

    let build = |headers: &[Header]| {
        build_h1_request(
            request_target(
                &endpoint,
                &UpstreamHostPolicy::default(),
                &ForwardedHeaderPolicy::default(),
            ),
            request_input(
                "GET",
                "/",
                headers,
                RequestInputMeta {
                    authority: Some("api.example.com"),
                    content_length: None,
                    request_id: 99,
                    traceparent: Some("00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01"),
                    client_addr: "203.0.113.88:8080".parse().expect("client"),
                },
            ),
        )
        .expect("request")
    };

    let forwarding_req = build(&forwarding_headers);
    let bootstrap_req = build(&bootstrap_headers);

    for name in [
        HOST.as_str(),
        "x-custom",
        "x-forwarded-for",
        "x-forwarded-proto",
        "x-forwarded-host",
        "forwarded",
        "x-request-id",
        "traceparent",
    ] {
        assert_eq!(
            forwarding_req
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok()),
            bootstrap_req
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok()),
            "header mismatch for {name}"
        );
    }
}
