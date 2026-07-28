use std::{
    collections::HashMap,
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Response, body::Incoming};
use serial_test::serial;
use spooky_config::config::{
    ApiKeyAuth, RouteAuth, ScopedRateLimit, ScopedRateLimitScope, UpstreamTls,
};

mod support;

use support::{
    net::local_listener_bind_available,
    request_path::{
        H3RequestSpec, QuicRequestPathHarness, make_backend, make_upstream, run_request_to,
    },
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

#[test]
#[serial]
fn quic_to_h2_success_path_normalizes_headers_and_keeps_get_bodyless() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = QuicRequestPathHarness::new();
    let backend_addr = harness.start_h2_backend(|req: hyper::Request<Incoming>| async move {
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
            "/headers-h2",
            vec![make_backend(
                "h2-headers",
                format!("https://{backend_addr}"),
            )],
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
        .run_request(H3RequestSpec {
            method: "GET",
            authority: "public.example.com",
            path: "/headers-h2",
            headers: &[
                ("forwarded", "for=1.2.3.4;proto=http;host=\"evil.example\""),
                ("x-forwarded-for", "1.2.3.4"),
                ("x-forwarded-proto", "http"),
                ("x-forwarded-host", "evil.example"),
                ("connection", "keep-alive, x-secret"),
                ("x-secret", "strip-me"),
            ],
            body: None,
            user_agent: "spooky-success-h2",
        })
        .expect("h3 request");

    response.assert_status(200);
    let body = response.body_text();
    assert_eq!(response_line(&body, "method="), "GET");
    assert_eq!(response_line(&body, "path="), "/headers-h2");
    assert_eq!(response_line(&body, "host="), "public.example.com");
    assert_eq!(
        response_line(&body, "forwarded="),
        "for=127.0.0.1;proto=https;host=\"public.example.com\""
    );
    assert_eq!(response_line(&body, "xff="), "127.0.0.1");
    assert_eq!(response_line(&body, "xfp="), "https");
    assert_eq!(response_line(&body, "xfh="), "public.example.com");
    assert_eq!(response_line(&body, "user_agent="), "spooky-success-h2");
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
fn quic_to_h2_success_path_streams_response_body_to_completion() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = QuicRequestPathHarness::new();
    let backend_addr =
        harness.start_h2_streaming_backend(vec![b"h2-chunk-1:", b"h2-chunk-2:", b"h2-chunk-3"]);

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "api".to_string(),
        make_upstream(
            "/stream-h2",
            vec![make_backend("h2-stream", format!("https://{backend_addr}"))],
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
        .run_request(H3RequestSpec::get("stream.example.com", "/stream-h2"))
        .expect("h3 request");
    response.assert_status(200);
    response.assert_body_text("h2-chunk-1:h2-chunk-2:h2-chunk-3");
}

#[test]
#[serial]
fn quic_request_path_auth_deny_rejects_before_upstream_dispatch() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = QuicRequestPathHarness::new();
    let upstream_calls = Arc::new(AtomicUsize::new(0));
    let backend_observed = Arc::clone(&upstream_calls);
    let backend_addr = harness.start_h1_backend(move |_req: hyper::Request<Incoming>| {
        let backend_observed = Arc::clone(&backend_observed);
        async move {
            backend_observed.fetch_add(1, Ordering::Relaxed);
            Ok::<_, std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(
                b"unexpected upstream call",
            ))))
        }
    });

    let mut upstream = make_upstream(
        "/protected",
        vec![make_backend(
            "h1-protected",
            format!("http://{backend_addr}"),
        )],
        None,
        "round-robin",
    );
    upstream.auth = RouteAuth {
        api_key: Some(ApiKeyAuth {
            header_name: "x-api-key".to_string(),
            keys: vec!["edge-key".to_string()],
        }),
        ..RouteAuth::default()
    };

    let mut upstreams = HashMap::new();
    upstreams.insert("api".to_string(), upstream);

    harness
        .start_listener(harness.make_config(upstreams))
        .expect("listener");

    let response = harness
        .run_request(H3RequestSpec::get("localhost", "/protected"))
        .expect("unauthorized request should complete");

    response.assert_status(401);
    assert_eq!(response.header("www-authenticate"), Some("ApiKey"));
    assert!(
        response.body_text().contains("unauthorized"),
        "expected canonical unauthorized response body"
    );
    assert_eq!(
        upstream_calls.load(Ordering::Relaxed),
        0,
        "pre-dispatch auth rejection must not contact upstream"
    );
}

#[test]
#[serial]
fn quic_request_path_rate_limit_deny_rejects_before_second_upstream_dispatch() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = QuicRequestPathHarness::new();
    let upstream_calls = Arc::new(AtomicUsize::new(0));
    let backend_observed = Arc::clone(&upstream_calls);
    let backend_addr = harness.start_h1_backend(move |_req: hyper::Request<Incoming>| {
        let backend_observed = Arc::clone(&backend_observed);
        async move {
            backend_observed.fetch_add(1, Ordering::Relaxed);
            Ok::<_, std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(
                b"rate-limit ok",
            ))))
        }
    });

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "api".to_string(),
        make_upstream(
            "/limited",
            vec![make_backend("h1-limited", format!("http://{backend_addr}"))],
            None,
            "round-robin",
        ),
    );

    let mut config = harness.make_config(upstreams);
    config.resilience.scoped_rate_limits = vec![ScopedRateLimit {
        name: "route-cap".to_string(),
        scope: ScopedRateLimitScope::Route,
        requests_per_sec: 1,
        burst: 1,
        key: None,
        route_allowlist: vec!["api".to_string()],
        idle_ttl_secs: 300,
    }];

    harness.start_listener(config).expect("listener");

    let first = harness
        .run_request(H3RequestSpec::get("localhost", "/limited"))
        .expect("first request should complete");
    first.assert_status(200);
    first.assert_body_text("rate-limit ok");

    let second = harness
        .run_request(H3RequestSpec::get("localhost", "/limited"))
        .expect("second request should complete");
    second.assert_status(429);
    assert!(
        second.body_text().contains("request rate limited"),
        "expected canonical rate-limit rejection body"
    );
    assert_eq!(
        upstream_calls.load(Ordering::Relaxed),
        1,
        "rate-limit rejection must stop before contacting upstream on the second request"
    );
}

#[test]
#[serial]
fn quic_request_path_upstream_overload_sheds_before_second_upstream_dispatch() {
    if !local_listener_bind_available() {
        return;
    }

    let mut harness = QuicRequestPathHarness::new();
    let upstream_calls = Arc::new(AtomicUsize::new(0));
    let backend_observed = Arc::clone(&upstream_calls);
    let backend_addr = harness.start_h1_backend(move |_req: hyper::Request<Incoming>| {
        let backend_observed = Arc::clone(&backend_observed);
        async move {
            backend_observed.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok::<_, std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(
                b"slow ok",
            ))))
        }
    });

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "api".to_string(),
        make_upstream(
            "/slow",
            vec![make_backend("h1-slow", format!("http://{backend_addr}"))],
            None,
            "round-robin",
        ),
    );

    let mut config = harness.make_config(upstreams);
    config.performance.global_inflight_limit = 64;
    config.performance.per_upstream_inflight_limit = 1;
    let listen_addr = harness.start_listener(config).expect("listener");
    let barrier = Arc::new(Barrier::new(2));

    let responses = thread::scope(|scope| {
        let first_barrier = Arc::clone(&barrier);
        let first = scope.spawn(move || {
            first_barrier.wait();
            run_request_to(listen_addr, H3RequestSpec::get("localhost", "/slow"))
        });

        let second_barrier = Arc::clone(&barrier);
        let second = scope.spawn(move || {
            second_barrier.wait();
            run_request_to(listen_addr, H3RequestSpec::get("localhost", "/slow"))
        });

        [
            first.join().expect("first request thread"),
            second.join().expect("second request thread"),
        ]
    });

    let mut status_200 = 0usize;
    let mut status_503 = 0usize;
    let mut shed_body = String::new();
    for response in responses {
        let response = response.expect("concurrent request should complete");
        match response.status {
            200 => status_200 += 1,
            503 => {
                status_503 += 1;
                shed_body = response.body_text();
            }
            other => panic!("unexpected status in overload test: {other}"),
        }
    }

    assert_eq!(status_200, 1, "expected one successful request");
    assert_eq!(status_503, 1, "expected one shed request");
    assert!(
        shed_body.contains("upstream overloaded"),
        "expected canonical overload response body, got `{shed_body}`"
    );
    assert_eq!(
        upstream_calls.load(Ordering::Relaxed),
        1,
        "upstream overload rejection must happen before the second request reaches the backend"
    );
}
