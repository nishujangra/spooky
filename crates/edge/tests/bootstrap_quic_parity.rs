use std::{collections::HashMap, convert::Infallible};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming};
use spooky_config::config::{
    ApiKeyAuth, ExternalAuth, ExternalAuthFailureMode, ExternalAuthRequestHeader, JwtAuth,
    LoadBalancing, RouteAuth, RouteMatch, Upstream,
};

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
