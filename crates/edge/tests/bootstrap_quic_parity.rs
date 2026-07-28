use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming};
use serial_test::serial;
use spooky_config::config::{
    ApiKeyAuth, ExternalAuth, ExternalAuthFailureMode, ExternalAuthRequestHeader, JwtAuth,
    LoadBalancing, RouteAuth, RouteMatch, ScopedRateLimit, ScopedRateLimitScope, Upstream,
};

mod support;

use support::{
    net::local_listener_bind_available,
    parity::{BootstrapQuicParityHarness, ParityRequestSpec, make_backend, make_upstream},
    request_path::{BootstrapRequestSpec, H3RequestSpec, run_bootstrap_request_to, run_request_to},
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

#[test]
fn bootstrap_and_quic_local_api_key_auth_decisions_match() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BootstrapQuicParityHarness::new();
    let backend_addr = harness.start_h1_static_backend(b"api key ok\n");

    let upstreams = HashMap::from([(
        "api".to_string(),
        auth_protected_upstream(
            "/protected",
            vec![make_backend("backend-a", backend_addr.to_string())],
            RouteAuth {
                api_key: Some(ApiKeyAuth {
                    header_name: "x-api-key".to_string(),
                    keys: vec!["edge-key".to_string()],
                }),
                ..RouteAuth::default()
            },
        ),
    )]);

    let config = harness.make_config(upstreams);
    harness.start_listener(config).expect("listener with bootstrap");

    let deny_request = ParityRequestSpec {
        method: "GET",
        authority: "localhost",
        path: "/protected",
        headers: &[],
        body: None,
        user_agent: "spooky-bootstrap-quic-parity-test",
        selected_response_headers: &["www-authenticate"],
        capture_metrics_delta: false,
    };
    let deny_pair = harness.run_parity_pair(deny_request).expect("api key deny");
    assert_eq!(deny_pair.quic.response.status, 401);
    assert_eq!(deny_pair.bootstrap.response.status, 401);
    assert_eq!(
        deny_pair.quic.response.selected_headers,
        vec![(String::from("www-authenticate"), String::from("ApiKey"))]
    );
    assert_eq!(
        deny_pair.bootstrap.response.selected_headers,
        deny_pair.quic.response.selected_headers
    );
    assert_eq!(deny_pair.bootstrap.response.body, deny_pair.quic.response.body);
    assert!(
        String::from_utf8_lossy(&deny_pair.quic.response.body).contains("unauthorized"),
        "expected canonical unauthorized body for api key rejection"
    );

    let allow_request = ParityRequestSpec {
        headers: &[("x-api-key", "edge-key")],
        ..deny_request
    };
    let allow_pair = harness.run_parity_pair(allow_request).expect("api key allow");
    assert_eq!(allow_pair.quic.response.status, 200);
    assert_eq!(allow_pair.bootstrap.response.status, 200);
    assert_eq!(allow_pair.quic.response.body, b"api key ok\n");
    assert_eq!(allow_pair.bootstrap.response.body, allow_pair.quic.response.body);
}

#[test]
fn bootstrap_and_quic_local_jwt_auth_decisions_match() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BootstrapQuicParityHarness::new();
    let backend_addr = harness.start_h1_static_backend(b"jwt ok\n");

    let upstreams = HashMap::from([(
        "api".to_string(),
        auth_protected_upstream(
            "/jwt",
            vec![make_backend("backend-a", backend_addr.to_string())],
            RouteAuth {
                api_key: None,
                jwt: Some(JwtAuth {
                    secret: "jwt-secret".to_string(),
                    issuer: Some("issuer-1".to_string()),
                    audience: Some("aud-1".to_string()),
                    clock_skew_secs: 30,
                }),
                external_auth: None,
                required_scopes: vec!["read:parity".to_string()],
                required_roles: Vec::new(),
            },
        ),
    )]);

    let config = harness.make_config(upstreams);
    harness.start_listener(config).expect("listener with bootstrap");

    let deny_request = ParityRequestSpec {
        method: "GET",
        authority: "localhost",
        path: "/jwt",
        headers: &[],
        body: None,
        user_agent: "spooky-bootstrap-quic-parity-test",
        selected_response_headers: &["www-authenticate"],
        capture_metrics_delta: false,
    };
    let deny_pair = harness.run_parity_pair(deny_request).expect("jwt deny");
    assert_eq!(deny_pair.quic.response.status, 401);
    assert_eq!(deny_pair.bootstrap.response.status, 401);
    assert_eq!(
        deny_pair.quic.response.selected_headers,
        vec![(String::from("www-authenticate"), String::from("Bearer"))]
    );
    assert_eq!(
        deny_pair.bootstrap.response.selected_headers,
        deny_pair.quic.response.selected_headers
    );
    assert_eq!(deny_pair.bootstrap.response.body, deny_pair.quic.response.body);
    assert!(
        String::from_utf8_lossy(&deny_pair.quic.response.body).contains("unauthorized"),
        "expected canonical unauthorized body for jwt rejection"
    );

    let token = encode_test_hs256_jwt(
        "jwt-secret",
        serde_json::json!({
            "sub": "user-1",
            "iss": "issuer-1",
            "aud": "aud-1",
            "exp": 4_000_000_000u64,
            "scope": "read:parity",
        }),
    );
    let authorization = format!("Bearer {token}");
    let allow_request = ParityRequestSpec {
        headers: &[("authorization", authorization.as_str())],
        ..deny_request
    };
    let allow_pair = harness.run_parity_pair(allow_request).expect("jwt allow");
    assert_eq!(allow_pair.quic.response.status, 200);
    assert_eq!(allow_pair.bootstrap.response.status, 200);
    assert_eq!(allow_pair.quic.response.body, b"jwt ok\n");
    assert_eq!(allow_pair.bootstrap.response.body, allow_pair.quic.response.body);
}

#[test]
fn bootstrap_and_quic_external_auth_decisions_match() {
    if !local_listener_bind_available() {
        return;
    }

    let cases = [
        ExternalAuthParityCase {
            name: "allow",
            auth_status: 204,
            auth_headers: &[("x-user-id", "alice")],
            auth_body: b"",
            allowlist: &["x-user-id"],
            expected_status: 200,
            expected_body: b"backend user=alice",
            expected_headers: &[],
        },
        ExternalAuthParityCase {
            name: "deny",
            auth_status: 403,
            auth_headers: &[("x-auth-reason", "policy")],
            auth_body: b"denied by auth",
            allowlist: &["x-auth-reason"],
            expected_status: 403,
            expected_body: b"denied by auth",
            expected_headers: &[("x-auth-reason", "policy")],
        },
        ExternalAuthParityCase {
            name: "challenge",
            auth_status: 401,
            auth_headers: &[
                ("www-authenticate", "Bearer realm=\"spooky\""),
                ("x-auth-reason", "expired"),
            ],
            auth_body: b"token expired",
            allowlist: &["x-auth-reason"],
            expected_status: 401,
            expected_body: b"token expired",
            expected_headers: &[
                ("www-authenticate", "Bearer realm=\"spooky\""),
                ("x-auth-reason", "expired"),
            ],
        },
        ExternalAuthParityCase {
            name: "redirect",
            auth_status: 302,
            auth_headers: &[("location", "https://login.example.com/")],
            auth_body: b"",
            allowlist: &[],
            expected_status: 302,
            expected_body: b"",
            expected_headers: &[("location", "https://login.example.com/")],
        },
    ];

    for case in cases {
        let mut harness = BootstrapQuicParityHarness::new();
        let backend_addr = harness.start_h1_backend(|req: Request<Incoming>| async move {
            let user = req
                .headers()
                .get("x-user-id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("missing");
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(format!(
                "backend user={user}"
            )))))
        });
        let auth_addr = harness.start_h1_backend(move |_req: Request<Incoming>| {
            let case = case;
            async move {
                let mut builder = Response::builder().status(case.auth_status);
                for (name, value) in case.auth_headers {
                    builder = builder.header(*name, *value);
                }
                Ok::<_, Infallible>(
                    builder
                        .body(Full::new(Bytes::from_static(case.auth_body)))
                        .expect("auth response"),
                )
            }
        });

        let config = configure_http_external_auth(
            &harness,
            format!("http://{backend_addr}"),
            format!("http://{auth_addr}/check"),
            250,
            ExternalAuthFailureMode::FailClosed,
            case.allowlist.iter().map(|value| (*value).to_string()).collect(),
        );
        harness.start_listener(config).expect("listener with bootstrap");

        let request = ParityRequestSpec {
            method: "GET",
            authority: "localhost",
            path: "/auth",
            headers: &[],
            body: None,
            user_agent: "spooky-bootstrap-quic-parity-test",
            selected_response_headers: &["www-authenticate", "x-auth-reason", "location"],
            capture_metrics_delta: false,
        };
        let pair = harness.run_parity_pair(request).unwrap_or_else(|err| {
            panic!("external auth parity case `{}` failed: {err}", case.name)
        });

        assert_eq!(
            pair.quic.response.status, case.expected_status,
            "quic external auth status mismatch for case `{}`",
            case.name
        );
        assert_eq!(
            pair.bootstrap.response.status, case.expected_status,
            "bootstrap external auth status mismatch for case `{}`",
            case.name
        );
        assert_eq!(
            pair.quic.response.body,
            case.expected_body,
            "quic external auth body mismatch for case `{}`",
            case.name
        );
        assert_eq!(
            pair.bootstrap.response.body,
            case.expected_body,
            "bootstrap external auth body mismatch for case `{}`",
            case.name
        );
        assert_eq!(
            pair.quic.response.selected_headers,
            expected_selected_headers(case.expected_headers),
            "quic external auth headers mismatch for case `{}`",
            case.name
        );
        assert_eq!(
            pair.bootstrap.response.selected_headers,
            pair.quic.response.selected_headers,
            "bootstrap and quic external auth headers should match for case `{}`",
            case.name
        );
    }
}

#[test]
fn bootstrap_and_quic_response_normalization_strip_same_hop_headers() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BootstrapQuicParityHarness::new();
    let backend_addr = harness.start_h1_backend(|_req: Request<Incoming>| async move {
        Ok::<_, Infallible>(
            Response::builder()
                .status(200)
                .header("connection", "x-hop-token")
                .header("x-hop-token", "secret")
                .header("content-length", "9")
                .header("content-type", "text/plain")
                .header("cache-control", "max-age=60")
                .header("etag", "\"etag-1\"")
                .body(Full::new(Bytes::from_static(b"strip hop")))
                .expect("response"),
        )
    });

    let config = harness.make_config(HashMap::from([(
        "api".to_string(),
        make_upstream(
            "/normalize-hop",
            vec![make_backend("backend-a", backend_addr.to_string())],
            None,
            "round-robin",
        ),
    )]));
    harness.start_listener(config).expect("listener with bootstrap");

    let request = ParityRequestSpec {
        method: "GET",
        authority: "localhost",
        path: "/normalize-hop",
        headers: &[],
        body: None,
        user_agent: "spooky-bootstrap-quic-parity-test",
        selected_response_headers: &[
            "cache-control",
            "connection",
            "content-length",
            "content-type",
            "etag",
            "x-hop-token",
        ],
        capture_metrics_delta: false,
    };
    let pair = harness
        .run_parity_pair(request)
        .expect("response normalization parity pair");

    assert_eq!(pair.quic.response.status, 200);
    assert_eq!(pair.bootstrap.response.status, 200);
    assert_eq!(pair.quic.response.body, b"strip hop");
    assert_eq!(pair.bootstrap.response.body, pair.quic.response.body);
    assert_eq!(
        selected_header_value(&pair.quic.response, "cache-control"),
        Some("max-age=60")
    );
    assert_eq!(
        selected_header_value(&pair.bootstrap.response, "cache-control"),
        Some("max-age=60")
    );
    assert_eq!(
        selected_header_value(&pair.quic.response, "etag"),
        Some("\"etag-1\"")
    );
    assert_eq!(
        selected_header_value(&pair.bootstrap.response, "etag"),
        Some("\"etag-1\"")
    );
    for stripped in ["connection", "x-hop-token"] {
        assert_eq!(
            selected_header_value(&pair.quic.response, stripped),
            None,
            "quic should strip hop header `{stripped}`"
        );
        assert_eq!(
            selected_header_value(&pair.bootstrap.response, stripped),
            None,
            "bootstrap should strip hop header `{stripped}`"
        );
    }
    assert_eq!(
        selected_header_value(&pair.quic.response, "content-type"),
        Some("text/plain")
    );
    assert_eq!(
        selected_header_value(&pair.bootstrap.response, "content-type"),
        Some("text/plain")
    );
    assert_eq!(
        selected_header_value(&pair.quic.response, "content-length"),
        None,
        "quic should omit downstream content-length under HTTP/3 framing"
    );
    assert_eq!(
        selected_header_value(&pair.bootstrap.response, "content-length"),
        Some("9"),
        "bootstrap should preserve explicit content-length under HTTP compatibility ingress"
    );
}

#[test]
fn bootstrap_and_quic_response_normalization_head_bodyless_contract_matches() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BootstrapQuicParityHarness::new();
    let backend_addr = harness.start_h1_backend(|_req: Request<Incoming>| async move {
        Ok::<_, Infallible>(
            Response::builder()
                .status(200)
                .body(Full::new(Bytes::from_static(b"hidden head body")))
                .expect("response"),
        )
    });

    let config = harness.make_config(HashMap::from([(
        "api".to_string(),
        make_upstream(
            "/head",
            vec![make_backend("backend-a", backend_addr.to_string())],
            None,
            "round-robin",
        ),
    )]));
    harness.start_listener(config).expect("listener with bootstrap");

    let request = ParityRequestSpec {
        method: "HEAD",
        authority: "localhost",
        path: "/head",
        headers: &[],
        body: None,
        user_agent: "spooky-bootstrap-quic-parity-test",
        selected_response_headers: &["content-length", "content-type"],
        capture_metrics_delta: false,
    };
    let pair = harness.run_parity_pair(request).expect("head parity pair");

    assert_eq!(pair.quic.response.status, 200);
    assert_eq!(pair.bootstrap.response.status, 200);
    assert!(pair.quic.response.body.is_empty());
    assert!(pair.bootstrap.response.body.is_empty());
    assert_eq!(
        selected_header_value(&pair.quic.response, "content-type"),
        None
    );
    assert_eq!(
        selected_header_value(&pair.bootstrap.response, "content-type"),
        None
    );
    assert_eq!(
        selected_header_value(&pair.quic.response, "content-length"),
        None
    );
    assert_eq!(
        selected_header_value(&pair.bootstrap.response, "content-length"),
        Some("16")
    );
}

#[test]
fn bootstrap_and_quic_response_normalization_no_content_contract_matches() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = BootstrapQuicParityHarness::new();
    let backend_addr = harness.start_h1_backend(|_req: Request<Incoming>| async move {
        Ok::<_, Infallible>(
            Response::builder()
                .status(204)
                .body(Full::new(Bytes::new()))
                .expect("response"),
        )
    });

    let config = harness.make_config(HashMap::from([(
        "api".to_string(),
        make_upstream(
            "/no-content",
            vec![make_backend("backend-a", backend_addr.to_string())],
            None,
            "round-robin",
        ),
    )]));
    harness.start_listener(config).expect("listener with bootstrap");

    let request = ParityRequestSpec {
        method: "GET",
        authority: "localhost",
        path: "/no-content",
        headers: &[],
        body: None,
        user_agent: "spooky-bootstrap-quic-parity-test",
        selected_response_headers: &["content-length", "content-type"],
        capture_metrics_delta: false,
    };
    let pair = harness
        .run_parity_pair(request)
        .expect("no content parity pair");

    assert_eq!(pair.quic.response.status, 204);
    assert_eq!(pair.bootstrap.response.status, 204);
    assert!(pair.quic.response.body.is_empty());
    assert!(pair.bootstrap.response.body.is_empty());
    assert_eq!(
        selected_header_value(&pair.quic.response, "content-type"),
        None
    );
    assert_eq!(
        selected_header_value(&pair.bootstrap.response, "content-type"),
        None
    );
    assert_eq!(
        selected_header_value(&pair.quic.response, "content-length"),
        None
    );
    assert_eq!(
        selected_header_value(&pair.bootstrap.response, "content-length"),
        None
    );
}

#[test]
#[serial]
fn bootstrap_and_quic_admission_rate_limit_rejections_match() {
    if !local_listener_bind_available() {
        return;
    }

    let quic = run_scoped_rate_limit_scenario(IngressKind::Quic);
    let bootstrap = run_scoped_rate_limit_scenario(IngressKind::Bootstrap);

    assert_eq!(quic.status, 429);
    assert_eq!(bootstrap.status, 429);
    assert_eq!(bootstrap.status, quic.status);
    assert_eq!(bootstrap.body, quic.body);
    assert!(
        quic.body.contains("request rate limited"),
        "expected canonical rate-limit rejection body, got `{}`",
        quic.body
    );
    assert_eq!(bootstrap.headers, quic.headers);
    assert_eq!(
        quic.headers,
        vec![(String::from("retry-after"), String::from("1"))]
    );
    assert_eq!(
        quic.upstream_calls, 1,
        "quic rate-limit path should reject before the second upstream dispatch"
    );
    assert_eq!(
        bootstrap.upstream_calls, 1,
        "bootstrap rate-limit path should reject before the second upstream dispatch"
    );
}

#[test]
#[serial]
#[ignore = "bootstrap path does not yet share post-auth admission overload semantics"]
fn bootstrap_and_quic_admission_overload_shed_contracts_match() {
    if !local_listener_bind_available() {
        return;
    }

    let quic = run_overload_shed_scenario(IngressKind::Quic);
    let bootstrap = run_overload_shed_scenario(IngressKind::Bootstrap);

    assert_eq!(quic.status, 503);
    assert_eq!(bootstrap.status, 503);
    assert_eq!(bootstrap.body, quic.body);
    assert!(
        quic.body.contains("route queue cap exceeded"),
        "expected canonical overload body, got `{}`",
        quic.body
    );
    assert_eq!(bootstrap.headers, quic.headers);
    assert_eq!(
        quic.headers,
        vec![(String::from("retry-after"), String::from("1"))]
    );
    assert_eq!(
        quic.upstream_calls, 1,
        "quic overload shedding should stop the second request before backend dispatch"
    );
    assert_eq!(
        bootstrap.upstream_calls, 1,
        "bootstrap overload shedding should stop the second request before backend dispatch"
    );
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

fn auth_protected_upstream(
    path_prefix: &str,
    backends: Vec<spooky_config::config::Backend>,
    auth: RouteAuth,
) -> Upstream {
    let mut upstream = make_upstream(path_prefix, backends, None, "round-robin");
    upstream.auth = auth;
    upstream
}

fn configure_http_external_auth(
    harness: &BootstrapQuicParityHarness,
    backend_address: String,
    auth_endpoint: String,
    timeout_ms: u64,
    failure_mode: ExternalAuthFailureMode,
    response_header_allowlist: Vec<String>,
) -> spooky_config::config::Config {
    let mut upstream = make_upstream(
        "/auth",
        vec![make_backend("auth-backend", backend_address)],
        None,
        "round-robin",
    );
    upstream.auth.external_auth = Some(ExternalAuth::Http {
        endpoint: auth_endpoint,
        request_headers: vec![ExternalAuthRequestHeader {
            name: "x-auth-static".to_string(),
            value: "1".to_string(),
        }],
        response_header_allowlist,
        timeout_ms,
        failure_mode,
    });

    let mut upstreams = HashMap::new();
    upstreams.insert("api".to_string(), upstream);
    harness.make_config(upstreams)
}

fn encode_test_hs256_jwt(secret: &str, claims: serde_json::Value) -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let header = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({ "alg": "HS256", "typ": "JWT" }))
            .expect("serialize header"),
    );
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("serialize claims"));
    let signing_input = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("mac");
    mac.update(signing_input.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{signing_input}.{signature}")
}

fn expected_selected_headers(headers: &[(&str, &str)]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn selected_header_value<'a>(
    response: &'a support::parity::ParityResponseSnapshot,
    name: &str,
) -> Option<&'a str> {
    response
        .selected_headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[derive(Clone, Copy)]
struct ExternalAuthParityCase<'a> {
    name: &'a str,
    auth_status: u16,
    auth_headers: &'a [(&'a str, &'a str)],
    auth_body: &'a [u8],
    allowlist: &'a [&'a str],
    expected_status: u16,
    expected_body: &'a [u8],
    expected_headers: &'a [(&'a str, &'a str)],
}

#[derive(Clone, Copy)]
enum IngressKind {
    Quic,
    Bootstrap,
}

struct RejectionObservation {
    status: u16,
    body: String,
    headers: Vec<(String, String)>,
    upstream_calls: usize,
}

fn run_scoped_rate_limit_scenario(ingress: IngressKind) -> RejectionObservation {
    let mut harness = BootstrapQuicParityHarness::new();
    let upstream_calls = Arc::new(AtomicUsize::new(0));
    let backend_observed = Arc::clone(&upstream_calls);
    let backend_addr = harness.start_h1_backend(move |_req: Request<Incoming>| {
        let backend_observed = Arc::clone(&backend_observed);
        async move {
            backend_observed.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(
                b"rate-limit ok",
            ))))
        }
    });

    let mut config = harness.make_config(HashMap::from([(
        "api".to_string(),
        make_upstream(
            "/limited",
            vec![make_backend("backend-a", backend_addr.to_string())],
            None,
            "round-robin",
        ),
    )]));
    config.resilience.scoped_rate_limits = vec![ScopedRateLimit {
        name: "route-cap".to_string(),
        scope: ScopedRateLimitScope::Route,
        requests_per_sec: 1,
        burst: 1,
        key: None,
        route_allowlist: vec!["api".to_string()],
        idle_ttl_secs: 300,
    }];

    harness.start_listener(config).expect("listener with bootstrap");

    let success = run_parity_ingress_request(
        ingress,
        &harness,
        ParityRequestSpec {
            method: "GET",
            authority: "localhost",
            path: "/limited",
            headers: &[],
            body: None,
            user_agent: "spooky-bootstrap-quic-parity-test",
            selected_response_headers: &[],
            capture_metrics_delta: false,
        },
    )
    .expect("first request should complete");
    assert_eq!(success.status, 200);

    let rejection = run_parity_ingress_request(
        ingress,
        &harness,
        ParityRequestSpec {
            method: "GET",
            authority: "localhost",
            path: "/limited",
            headers: &[],
            body: None,
            user_agent: "spooky-bootstrap-quic-parity-test",
            selected_response_headers: &["retry-after", "www-authenticate"],
            capture_metrics_delta: false,
        },
    )
    .expect("second request should complete");

    RejectionObservation {
        status: rejection.status,
        body: String::from_utf8_lossy(&rejection.body).into_owned(),
        headers: rejection.selected_headers,
        upstream_calls: upstream_calls.load(Ordering::Relaxed),
    }
}

fn run_overload_shed_scenario(ingress: IngressKind) -> RejectionObservation {
    let mut harness = BootstrapQuicParityHarness::new();
    let upstream_calls = Arc::new(AtomicUsize::new(0));
    let backend_observed = Arc::clone(&upstream_calls);
    let backend_addr = harness.start_h1_backend(move |_req: Request<Incoming>| {
        let backend_observed = Arc::clone(&backend_observed);
        async move {
            backend_observed.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"slow ok"))))
        }
    });

    let mut config = harness.make_config(HashMap::from([(
        "api".to_string(),
        make_upstream(
            "/slow",
            vec![make_backend("backend-a", backend_addr.to_string())],
            None,
            "round-robin",
        ),
    )]));
    config.performance.global_inflight_limit = 64;
    config.resilience.route_queue.default_cap = 1;
    config.resilience.route_queue.global_cap = 64;
    harness.start_listener(config).expect("listener with bootstrap");
    let listen_addr = harness.listen_addr();
    let cert_path = harness.cert_path().to_string();

    let first_request = thread::spawn({
        let cert_path = cert_path.clone();
        move || {
            execute_ingress_request(
                ingress,
                listen_addr,
                &cert_path,
                ParityRequestSpec::get("localhost", "/slow"),
            )
        }
    });

    for _ in 0..50 {
        if upstream_calls.load(Ordering::Relaxed) > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let rejection = execute_ingress_request(
        ingress,
        listen_addr,
        &cert_path,
        ParityRequestSpec {
            method: "GET",
            authority: "localhost",
            path: "/slow",
            headers: &[],
            body: None,
            user_agent: "spooky-bootstrap-quic-parity-test",
            selected_response_headers: &["retry-after", "www-authenticate"],
            capture_metrics_delta: false,
        },
    )
    .expect("second request should complete");
    let first_response = first_request
        .join()
        .expect("first request thread")
        .expect("first request should complete");
    assert_eq!(first_response.status, 200);

    RejectionObservation {
        status: rejection.status,
        body: String::from_utf8_lossy(&rejection.body).into_owned(),
        headers: rejection.selected_headers,
        upstream_calls: upstream_calls.load(Ordering::Relaxed),
    }
}

fn run_parity_ingress_request(
    ingress: IngressKind,
    harness: &BootstrapQuicParityHarness,
    request: ParityRequestSpec<'_>,
) -> Result<support::parity::ParityResponseSnapshot, String> {
    let observation = match ingress {
        IngressKind::Quic => harness.run_quic(request)?,
        IngressKind::Bootstrap => harness.run_bootstrap(request)?,
    };
    Ok(observation.response)
}

fn execute_ingress_request(
    ingress: IngressKind,
    listen_addr: std::net::SocketAddr,
    cert_path: &str,
    request: ParityRequestSpec<'_>,
) -> Result<support::parity::ParityResponseSnapshot, String> {
    match ingress {
        IngressKind::Quic => {
            let response = run_request_to(
                listen_addr,
                H3RequestSpec {
                    method: request.method,
                    authority: request.authority,
                    path: request.path,
                    headers: request.headers,
                    body: request.body,
                    user_agent: request.user_agent,
                },
            )?;
            Ok(parity_snapshot_from_response(
                response.status,
                response.body,
                response.headers,
                request.selected_response_headers,
            ))
        }
        IngressKind::Bootstrap => {
            let response = run_bootstrap_request_to(
                listen_addr,
                cert_path,
                BootstrapRequestSpec {
                    method: request.method,
                    authority: request.authority,
                    path: request.path,
                    headers: request.headers,
                    body: request.body,
                    user_agent: request.user_agent,
                },
            )?;
            Ok(parity_snapshot_from_response(
                response.status,
                response.body,
                response.headers,
                request.selected_response_headers,
            ))
        }
    }
}

fn parity_snapshot_from_response(
    status: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    selected_response_headers: &[&str],
) -> support::parity::ParityResponseSnapshot {
    let mut selected_headers = headers
        .into_iter()
        .filter(|(name, _)| {
            selected_response_headers
                .iter()
                .any(|selected| name.eq_ignore_ascii_case(selected))
        })
        .collect::<Vec<_>>();
    selected_headers.sort_by(|left, right| {
        left.0
            .to_ascii_lowercase()
            .cmp(&right.0.to_ascii_lowercase())
            .then_with(|| left.1.cmp(&right.1))
    });

    support::parity::ParityResponseSnapshot {
        status,
        body,
        selected_headers,
    }
}
