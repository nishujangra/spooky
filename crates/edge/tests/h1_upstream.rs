use std::{
    collections::HashMap,
    net::{SocketAddr, TcpListener as StdTcpListener, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use quiche::h3::NameValue;
use rand::RngCore;
use rcgen::{Certificate, CertificateParams, SanType};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use serial_test::serial;
use spooky_config::{
    config::{
        Backend, ClientAuth, Config, Listen, LoadBalancing, Log, LogFormat, RouteMatch, Security,
        Tls, Upstream, UpstreamTls,
    },
    validator::validate,
};
use spooky_edge::{
    MAX_DATAGRAM_SIZE_BYTES, MAX_UDP_PAYLOAD_BYTES, QUIC_IDLE_TIMEOUT_MS, QUIC_INITIAL_MAX_DATA,
    QUIC_INITIAL_MAX_STREAMS_BIDI, QUIC_INITIAL_MAX_STREAMS_UNI, QUIC_INITIAL_STREAM_DATA,
    REQUEST_TIMEOUT_SECS, UDP_READ_TIMEOUT_MS,
    runtime::listener::QUICListener,
};
use tempfile::{TempDir, tempdir};
use tokio::net::TcpListener;
use tokio_rustls::{TlsAcceptor, rustls::ServerConfig};

fn write_test_certs(dir: &TempDir) -> (String, String) {
    let mut params = CertificateParams::new(vec!["localhost".into()]);
    params
        .subject_alt_names
        .push(SanType::DnsName("localhost".to_string()));
    params.subject_alt_names.push(SanType::IpAddress(
        "127.0.0.1".parse().expect("loopback ip"),
    ));
    let cert = Certificate::from_params(params).expect("failed to build cert");

    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");

    std::fs::write(&cert_path, cert.serialize_pem().expect("serialize cert")).expect("write cert");
    std::fs::write(&key_path, cert.serialize_private_key_pem()).expect("write key");

    (
        cert_path.to_string_lossy().to_string(),
        key_path.to_string_lossy().to_string(),
    )
}

fn read_test_chain(cert_path: &str) -> Vec<CertificateDer<'static>> {
    CertificateDer::pem_file_iter(cert_path)
        .expect("open cert file")
        .collect::<Result<Vec<_>, _>>()
        .expect("parse certs")
}

fn read_test_key(key_path: &str) -> PrivateKeyDer<'static> {
    PrivateKeyDer::from_pem_file(key_path).expect("parse private key")
}

fn make_config(
    cert: String,
    key: String,
    upstreams: HashMap<String, Upstream>,
    upstream_tls: UpstreamTls,
) -> Config {
    let listen_port = reserve_unused_udp_port();
    Config {
        version: 1,
        listen: Listen {
            protocol: "http3".to_string(),
            port: listen_port,
            address: "127.0.0.1".to_string(),
            tls: Tls {
                cert,
                key,
                certificates: Vec::new(),
                client_auth: ClientAuth::default(),
            },
        },
        listeners: Vec::new(),
        upstream: upstreams,
        load_balancing: Some(LoadBalancing {
            lb_type: "round-robin".to_string(),
            key: None,
        }),
        upstream_tls,
        log: Log {
            level: "info".to_string(),
            file: Default::default(),
            format: LogFormat::Plain,
        },
        performance: spooky_config::config::Performance::default(),
        observability: spooky_config::config::Observability::default(),
        resilience: spooky_config::config::Resilience::default(),
        security: Security::default(),
    }
}

fn make_upstream(
    path_prefix: &str,
    backends: Vec<Backend>,
    tls: Option<UpstreamTls>,
    lb_type: &str,
) -> Upstream {
    Upstream {
        load_balancing: LoadBalancing {
            lb_type: lb_type.to_string(),
            key: None,
        },
        auth: Default::default(),
        host_policy: Default::default(),
        forwarded_headers: Default::default(),
        tls,
        route: RouteMatch {
            host: None,
            path_prefix: Some(path_prefix.to_string()),
            method: None,
        },
        backends,
    }
}

fn make_backend(id: &str, address: String) -> Backend {
    Backend {
        id: id.to_string(),
        address,
        weight: 1,
        health_check: None,
    }
}

fn bind_tcp_listener() -> TcpListener {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind test backend listener");
    listener
        .set_nonblocking(true)
        .expect("set test backend listener nonblocking");
    TcpListener::from_std(listener).expect("register test backend listener")
}

fn local_tcp_bind_available() -> bool {
    match StdTcpListener::bind("127.0.0.1:0") {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(_) => true,
    }
}

async fn start_h1_backend<F>(handler: F) -> SocketAddr
where
    F: Fn(Request<Incoming>) -> Response<Full<Bytes>> + Clone + Send + 'static,
{
    let listener = bind_tcp_listener();
    let addr = listener.local_addr().expect("h1 local addr");

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(value) => value,
                Err(_) => break,
            };
            let handler = handler.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let handler = handler.clone();
                    async move { Ok::<_, hyper::Error>(handler(req)) }
                });

                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    addr
}

async fn start_h1_delayed_backend(body: &'static str, delay: Duration) -> SocketAddr {
    let listener = bind_tcp_listener();
    let addr = listener.local_addr().expect("delayed h1 local addr");

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(value) => value,
                Err(_) => break,
            };
            tokio::spawn(async move {
                let service = service_fn(move |_req: Request<Incoming>| async move {
                    tokio::time::sleep(delay).await;
                    Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from_static(
                        body.as_bytes(),
                    ))))
                });

                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    addr
}

async fn start_h2_tls_backend<F>(cert_path: &str, key_path: &str, handler: F) -> SocketAddr
where
    F: Fn(Request<Incoming>) -> Response<Full<Bytes>> + Clone + Send + 'static,
{
    let mut tls_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(read_test_chain(cert_path), read_test_key(key_path))
        .expect("server tls config");
    tls_config.alpn_protocols = vec![b"h2".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = bind_tcp_listener();
    let addr = listener.local_addr().expect("h2 local addr");

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(value) => value,
                Err(_) => break,
            };
            let acceptor = acceptor.clone();
            let handler = handler.clone();
            tokio::spawn(async move {
                let tls_stream = match acceptor.accept(stream).await {
                    Ok(stream) => stream,
                    Err(_) => return,
                };
                let service = service_fn(move |req: Request<Incoming>| {
                    let handler = handler.clone();
                    async move { Ok::<_, hyper::Error>(handler(req)) }
                });
                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(tls_stream), service)
                    .await;
            });
        }
    });

    addr
}

struct ListenerTaskGuard {
    stop: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl ListenerTaskGuard {
    fn spawn(rt: &tokio::runtime::Runtime, mut listener: QUICListener) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let runtime_handle = rt.handle().clone();
        let handle = rt.spawn_blocking(move || {
            let _enter = runtime_handle.enter();
            while !stop_flag.load(Ordering::Relaxed) {
                listener.poll();
            }
        });
        Self { stop, handle }
    }
}

impl Drop for ListenerTaskGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.abort();
    }
}

struct H3Response {
    status: String,
    body: Vec<u8>,
}

fn quic_read_timeout(conn: &quiche::Connection) -> Duration {
    conn.timeout()
        .filter(|timeout| !timeout.is_zero())
        .unwrap_or(Duration::from_millis(UDP_READ_TIMEOUT_MS))
}

fn run_h3_get(
    addr: SocketAddr,
    authority: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<H3Response, String> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|err| err.to_string())?;
    let local_addr = socket.local_addr().map_err(|err| err.to_string())?;

    let mut config =
        quiche::Config::new(quiche::PROTOCOL_VERSION).map_err(|err| format!("config: {err:?}"))?;
    config.verify_peer(false);
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .map_err(|err| format!("alpn: {err:?}"))?;
    config.set_max_idle_timeout(QUIC_IDLE_TIMEOUT_MS);
    config.set_max_recv_udp_payload_size(MAX_UDP_PAYLOAD_BYTES);
    config.set_max_send_udp_payload_size(MAX_UDP_PAYLOAD_BYTES);
    config.set_initial_max_data(QUIC_INITIAL_MAX_DATA);
    config.set_initial_max_stream_data_bidi_local(QUIC_INITIAL_STREAM_DATA);
    config.set_initial_max_stream_data_bidi_remote(QUIC_INITIAL_STREAM_DATA);
    config.set_initial_max_stream_data_uni(QUIC_INITIAL_STREAM_DATA);
    config.set_initial_max_streams_bidi(QUIC_INITIAL_MAX_STREAMS_BIDI);
    config.set_initial_max_streams_uni(QUIC_INITIAL_MAX_STREAMS_UNI);
    config.set_disable_active_migration(true);

    let mut scid_bytes = [0u8; quiche::MAX_CONN_ID_LEN];
    rand::thread_rng().fill_bytes(&mut scid_bytes);
    let scid = quiche::ConnectionId::from_ref(&scid_bytes);

    let mut conn = quiche::connect(Some("localhost"), &scid, local_addr, addr, &mut config)
        .map_err(|err| format!("connect: {err:?}"))?;
    let h3_config = quiche::h3::Config::new().map_err(|err| format!("h3: {err:?}"))?;
    let mut h3: Option<quiche::h3::Connection> = None;

    let mut out = [0u8; MAX_UDP_PAYLOAD_BYTES];
    let mut buf = [0u8; MAX_DATAGRAM_SIZE_BYTES];
    let mut status = String::new();
    let mut body = Vec::new();
    let mut request_sent = false;
    let start = Instant::now();

    let (write, send_info) = conn
        .send(&mut out)
        .map_err(|err| format!("send: {err:?}"))?;
    socket
        .send_to(&out[..write], send_info.to)
        .map_err(|err| format!("send_to: {err:?}"))?;

    loop {
        loop {
            match conn.send(&mut out) {
                Ok((write, send_info)) => {
                    let _ = socket.send_to(&out[..write], send_info.to);
                }
                Err(quiche::Error::Done) => break,
                Err(err) => return Err(format!("send loop: {err:?}")),
            }
        }

        socket
            .set_read_timeout(Some(quic_read_timeout(&conn)))
            .map_err(|err| format!("timeout: {err:?}"))?;

        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                conn.recv(
                    &mut buf[..len],
                    quiche::RecvInfo {
                        from,
                        to: local_addr,
                    },
                )
                .map_err(|err| format!("recv: {err:?}"))?;
            }
            Err(ref err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                conn.on_timeout();
            }
            Err(err) => return Err(format!("recv: {err:?}")),
        }

        if conn.is_established() && h3.is_none() {
            h3 = Some(
                quiche::h3::Connection::with_transport(&mut conn, &h3_config)
                    .map_err(|err| format!("h3 conn: {err:?}"))?,
            );
        }

        if let Some(h3_conn) = h3.as_mut() {
            if conn.is_established() && !request_sent {
                let mut request_headers = vec![
                    quiche::h3::Header::new(b":method", b"GET"),
                    quiche::h3::Header::new(b":scheme", b"https"),
                    quiche::h3::Header::new(b":authority", authority.as_bytes()),
                    quiche::h3::Header::new(b":path", path.as_bytes()),
                    quiche::h3::Header::new(b"user-agent", b"spooky-h1-regression"),
                ];
                request_headers.extend(headers.iter().map(|(name, value)| {
                    quiche::h3::Header::new(name.as_bytes(), value.as_bytes())
                }));
                h3_conn
                    .send_request(&mut conn, &request_headers, true)
                    .map_err(|err| format!("send_request: {err:?}"))?;
                request_sent = true;
            }

            loop {
                match h3_conn.poll(&mut conn) {
                    Ok((_stream_id, quiche::h3::Event::Headers { list, .. })) => {
                        for header in &list {
                            if header.name() == b":status" {
                                status = String::from_utf8_lossy(header.value()).to_string();
                            }
                        }
                    }
                    Ok((stream_id, quiche::h3::Event::Data)) => loop {
                        match h3_conn.recv_body(&mut conn, stream_id, &mut buf) {
                            Ok(read) => body.extend_from_slice(&buf[..read]),
                            Err(quiche::h3::Error::Done) => break,
                            Err(err) => return Err(format!("recv_body: {err:?}")),
                        }
                    },
                    Ok((_stream_id, quiche::h3::Event::Finished)) => {
                        return Ok(H3Response { status, body });
                    }
                    Ok((_stream_id, quiche::h3::Event::Reset(_))) => {
                        return Err("stream reset".to_string());
                    }
                    Ok((_stream_id, quiche::h3::Event::PriorityUpdate)) => {}
                    Ok((_stream_id, quiche::h3::Event::GoAway)) => {}
                    Err(quiche::h3::Error::Done) => break,
                    Err(err) => return Err(format!("poll: {err:?}")),
                }
            }
        }

        if start.elapsed() > Duration::from_secs(REQUEST_TIMEOUT_SECS) {
            return Err(format!(
                "timeout waiting for response (status='{}', body_len={})",
                status,
                body.len()
            ));
        }
    }
}

fn reserve_unused_udp_port() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("reserve udp port");
    let port = socket.local_addr().expect("udp local addr").port();
    drop(socket);
    port
}

#[test]
#[serial]
fn http_only_upstream_starts_and_forwards_requests_end_to_end() {
    if !local_tcp_bind_available() {
        return;
    }
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let backend_addr = rt.block_on(start_h1_backend(|_req| {
        Response::new(Full::new(Bytes::from_static(b"http-only ok\n")))
    }));

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "plain".to_string(),
        make_upstream(
            "/",
            vec![make_backend("plain-1", format!("http://{backend_addr}"))],
            Some(UpstreamTls {
                verify_certificates: true,
                strict_sni: true,
                ca_file: Some("/path/does/not/exist.pem".to_string()),
                ca_dir: Some("/path/does/not/exist".to_string()),
            }),
            "round-robin",
        ),
    );
    let config = make_config(
        cert,
        key,
        upstreams,
        UpstreamTls {
            verify_certificates: true,
            strict_sni: true,
            ca_file: Some("/path/does/not/exist-global.pem".to_string()),
            ca_dir: Some("/path/does/not/exist-global".to_string()),
        },
    );

    validate(&config).expect("http-only config should validate");
    let listener = QUICListener::new(config).expect("listener");
    let listen_addr = listener.socket.local_addr().expect("listen addr");
    let _listener_task = ListenerTaskGuard::spawn(&rt, listener);

    let response = run_h3_get(listen_addr, "public.example.com", "/", &[]).expect("h3 request");
    assert_eq!(response.status, "200");
    assert_eq!(String::from_utf8_lossy(&response.body), "http-only ok\n");
}

#[test]
#[serial]
fn http_only_upstream_normalizes_forwarding_headers() {
    if !local_tcp_bind_available() {
        return;
    }
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let backend_addr = rt.block_on(start_h1_backend(|req| {
        let header = |name: &str| {
            req.headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<missing>")
                .to_string()
        };
        let body = format!(
            "host={}\nforwarded={}\nxff={}\nxfp={}\nxfh={}\nhas_connection={}\nx-secret={}\n",
            header("host"),
            header("forwarded"),
            header("x-forwarded-for"),
            header("x-forwarded-proto"),
            header("x-forwarded-host"),
            req.headers().contains_key("connection"),
            header("x-secret"),
        );
        Response::new(Full::new(Bytes::from(body)))
    }));

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "headers".to_string(),
        make_upstream(
            "/headers",
            vec![make_backend("headers-1", format!("http://{backend_addr}"))],
            None,
            "round-robin",
        ),
    );
    let config = make_config(cert, key, upstreams, UpstreamTls::default());

    validate(&config).expect("config should validate");
    let listener = QUICListener::new(config).expect("listener");
    let listen_addr = listener.socket.local_addr().expect("listen addr");
    let _listener_task = ListenerTaskGuard::spawn(&rt, listener);

    let response = run_h3_get(
        listen_addr,
        "public.example.com",
        "/headers",
        &[
            ("forwarded", "for=1.2.3.4;proto=http;host=\"evil.example\""),
            ("x-forwarded-for", "1.2.3.4"),
            ("x-forwarded-proto", "http"),
            ("x-forwarded-host", "evil.example"),
            ("connection", "keep-alive, x-secret"),
            ("x-secret", "should-strip"),
        ],
    )
    .expect("h3 request");
    let body = String::from_utf8_lossy(&response.body);

    assert_eq!(response.status, "200");
    assert!(body.contains("host=public.example.com"));
    assert!(body.contains("forwarded=for=127.0.0.1;proto=https;host=\"public.example.com\""));
    assert!(body.contains("xff=127.0.0.1"));
    assert!(body.contains("xfp=https"));
    assert!(body.contains("xfh=public.example.com"));
    assert!(body.contains("has_connection=false"));
    assert!(body.contains("x-secret=<missing>"));
}

#[test]
#[serial]
fn http_only_upstream_retries_bodyless_requests_on_alternate_backend() {
    if !local_tcp_bind_available() {
        return;
    }
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let stalled_backend = rt.block_on(start_h1_delayed_backend(
        "too slow\n",
        Duration::from_secs(2),
    ));
    let healthy_backend = rt.block_on(start_h1_backend(|_req| {
        Response::new(Full::new(Bytes::from_static(b"retry ok\n")))
    }));

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "retry".to_string(),
        make_upstream(
            "/retry",
            vec![
                make_backend("stalled", format!("http://{stalled_backend}")),
                make_backend("healthy", format!("http://{healthy_backend}")),
            ],
            None,
            "round-robin",
        ),
    );
    let mut config = make_config(cert, key, upstreams, UpstreamTls::default());
    config.performance.backend_connect_timeout_ms = 50;
    config.performance.backend_timeout_ms = 100;

    validate(&config).expect("config should validate");
    let listener = QUICListener::new(config).expect("listener");
    let listen_addr = listener.socket.local_addr().expect("listen addr");
    let _listener_task = ListenerTaskGuard::spawn(&rt, listener);

    let response =
        run_h3_get(listen_addr, "retry.example.com", "/retry", &[]).expect("retry request");
    assert_eq!(response.status, "200");
    assert_eq!(String::from_utf8_lossy(&response.body), "retry ok\n");
}

#[test]
#[serial]
fn mixed_http_and_https_upstreams_route_by_scheme() {
    if !local_tcp_bind_available() {
        return;
    }
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(&dir);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let plain_backend = rt.block_on(start_h1_backend(|_req| {
        Response::new(Full::new(Bytes::from_static(b"plain backend\n")))
    }));
    let secure_backend = rt.block_on(start_h2_tls_backend(&cert, &key, |_req| {
        Response::new(Full::new(Bytes::from_static(b"secure backend\n")))
    }));

    let mut upstreams = HashMap::new();
    upstreams.insert(
        "plain".to_string(),
        make_upstream(
            "/plain",
            vec![make_backend("plain-1", format!("http://{plain_backend}"))],
            None,
            "round-robin",
        ),
    );
    upstreams.insert(
        "secure".to_string(),
        make_upstream(
            "/secure",
            vec![make_backend(
                "secure-1",
                format!("https://{secure_backend}"),
            )],
            Some(UpstreamTls {
                verify_certificates: false,
                strict_sni: true,
                ca_file: None,
                ca_dir: None,
            }),
            "round-robin",
        ),
    );
    let config = make_config(cert, key, upstreams, UpstreamTls::default());

    validate(&config).expect("mixed config should validate");
    let listener = QUICListener::new(config).expect("listener");
    let listen_addr = listener.socket.local_addr().expect("listen addr");
    let _listener_task = ListenerTaskGuard::spawn(&rt, listener);

    let plain = run_h3_get(listen_addr, "mixed.example.com", "/plain", &[]).expect("plain request");
    let secure =
        run_h3_get(listen_addr, "mixed.example.com", "/secure", &[]).expect("secure request");

    assert_eq!(plain.status, "200");
    assert_eq!(String::from_utf8_lossy(&plain.body), "plain backend\n");
    assert_eq!(secure.status, "200");
    assert_eq!(String::from_utf8_lossy(&secure.body), "secure backend\n");
}
