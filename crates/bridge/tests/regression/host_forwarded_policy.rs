//! Canonical host-policy and forwarded-header-policy contracts across h1 and h2.

use std::convert::Infallible;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use quiche::h3::Header;
use spooky_config::{
    backend_endpoint::BackendEndpoint,
    config::{
        ForwardedHeaderPolicy, ForwardedHeaderPolicyMode, UpstreamHostPolicy,
        UpstreamHostPolicyMode,
    },
};

use crate::common::{RequestInputMeta, build_h1_and_h2_requests};

fn assert_same_header(
    h1: &http::Request<BoxBody<Bytes, Infallible>>,
    h2: &http::Request<BoxBody<Bytes, Infallible>>,
    name: &str,
) {
    assert_eq!(
        h1.headers().get(name).and_then(|value| value.to_str().ok()),
        h2.headers().get(name).and_then(|value| value.to_str().ok()),
        "canonical policy application mismatch for header {name}"
    );
}

#[test]
fn pass_through_host_policy_prefers_request_authority_for_h1_and_h2() {
    let endpoint = BackendEndpoint::parse("backend.internal:8443").expect("endpoint");
    let host_policy = UpstreamHostPolicy {
        mode: UpstreamHostPolicyMode::PassThrough,
        host: None,
    };
    let headers = vec![Header::new(b"host", b"spoofed.example.com")];

    let (h1, h2) = build_h1_and_h2_requests(
        &endpoint,
        &host_policy,
        &ForwardedHeaderPolicy::default(),
        "GET",
        "/",
        &headers,
        RequestInputMeta {
            authority: Some("tenant.example.com"),
            content_length: None,
            request_id: 501,
            traceparent: None,
            client_addr: "203.0.113.11:7000".parse().expect("client"),
        },
    )
    .expect("requests");

    for name in [http::header::HOST.as_str(), "x-forwarded-host", "forwarded"] {
        assert_same_header(&h1, &h2, name);
    }

    assert_eq!(
        h1.headers()
            .get(http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        Some("tenant.example.com")
    );
    assert_eq!(
        h1.headers()
            .get("x-forwarded-host")
            .and_then(|value| value.to_str().ok()),
        Some("tenant.example.com")
    );
    assert_eq!(
        h1.headers()
            .get("forwarded")
            .and_then(|value| value.to_str().ok()),
        Some("for=203.0.113.11;proto=https;host=\"tenant.example.com\"")
    );
}

#[test]
fn pass_through_host_policy_falls_back_to_inbound_host_header_for_h1_and_h2() {
    let endpoint = BackendEndpoint::parse("backend.internal:8443").expect("endpoint");
    let host_policy = UpstreamHostPolicy {
        mode: UpstreamHostPolicyMode::PassThrough,
        host: None,
    };
    let headers = vec![Header::new(b"host", b"header-only.example.com")];

    let (h1, h2) = build_h1_and_h2_requests(
        &endpoint,
        &host_policy,
        &ForwardedHeaderPolicy::default(),
        "GET",
        "/fallback",
        &headers,
        RequestInputMeta {
            authority: None,
            content_length: None,
            request_id: 502,
            traceparent: None,
            client_addr: "203.0.113.12:7001".parse().expect("client"),
        },
    )
    .expect("requests");

    for name in [http::header::HOST.as_str(), "x-forwarded-host", "forwarded"] {
        assert_same_header(&h1, &h2, name);
    }

    assert_eq!(
        h1.headers()
            .get(http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        Some("header-only.example.com")
    );
    assert_eq!(
        h1.headers()
            .get("x-forwarded-host")
            .and_then(|value| value.to_str().ok()),
        Some("header-only.example.com")
    );
}

#[test]
fn rewrite_and_upstream_host_policies_apply_identically_for_h1_and_h2() {
    let endpoint = BackendEndpoint::parse("backend.internal:8443").expect("endpoint");
    let headers = vec![Header::new(b"host", b"spoofed.example.com")];

    for policy in [
        UpstreamHostPolicy {
            mode: UpstreamHostPolicyMode::Rewrite,
            host: Some("origin.example.com".to_string()),
        },
        UpstreamHostPolicy {
            mode: UpstreamHostPolicyMode::Upstream,
            host: None,
        },
    ] {
        let (h1, h2) = build_h1_and_h2_requests(
            &endpoint,
            &policy,
            &ForwardedHeaderPolicy::default(),
            "GET",
            "/policy",
            &headers,
            RequestInputMeta {
                authority: Some("tenant.example.com"),
                content_length: None,
                request_id: 503,
                traceparent: None,
                client_addr: "203.0.113.13:7002".parse().expect("client"),
            },
        )
        .expect("requests");

        for name in [http::header::HOST.as_str(), "x-forwarded-host", "forwarded"] {
            assert_same_header(&h1, &h2, name);
        }
    }
}

#[test]
fn forwarded_header_policy_modes_apply_identically_for_h1_and_h2() {
    let endpoint = BackendEndpoint::parse("backend.internal:443").expect("endpoint");
    let host_policy = UpstreamHostPolicy::default();
    let headers = vec![
        Header::new(b"forwarded", b"for=1.2.3.4;proto=http;host=\"old.example\""),
        Header::new(b"x-forwarded-for", b"1.2.3.4"),
        Header::new(b"x-forwarded-proto", b"http"),
        Header::new(b"x-forwarded-host", b"old.example"),
        Header::new(b"connection", b"keep-alive, x-secret"),
        Header::new(b"x-secret", b"drop-me"),
        Header::new(b"x-keep", b"ok"),
    ];

    for policy in [
        ForwardedHeaderPolicy {
            mode: ForwardedHeaderPolicyMode::Overwrite,
        },
        ForwardedHeaderPolicy {
            mode: ForwardedHeaderPolicyMode::Append,
        },
        ForwardedHeaderPolicy {
            mode: ForwardedHeaderPolicyMode::Preserve,
        },
    ] {
        let (h1, h2) = build_h1_and_h2_requests(
            &endpoint,
            &host_policy,
            &policy,
            "GET",
            "/forwarded",
            &headers,
            RequestInputMeta {
                authority: Some("api.example.com"),
                content_length: None,
                request_id: 504,
                traceparent: None,
                client_addr: "203.0.113.14:7003".parse().expect("client"),
            },
        )
        .expect("requests");

        for name in [
            "forwarded",
            "x-forwarded-for",
            "x-forwarded-proto",
            "x-forwarded-host",
            "x-keep",
        ] {
            assert_same_header(&h1, &h2, name);
        }

        assert!(h1.headers().get("x-secret").is_none());
        assert!(h2.headers().get("x-secret").is_none());
        assert_eq!(
            h1.headers().get("x-keep").and_then(|value| value.to_str().ok()),
            Some("ok")
        );
    }
}
