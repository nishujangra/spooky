#![allow(dead_code)]

use std::{
    collections::HashMap,
    convert::Infallible,
    fmt::Write as _,
    future::Future,
    net::{SocketAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use http::{Method, StatusCode};
use http_body_util::{BodyExt, Empty, Full};
use hyper::{
    Request, Response,
    body::Incoming,
    client::conn::http1,
};
use hyper_util::rt::TokioIo;
use rustls_pki_types::{CertificateDer, pem::PemObject};
use serde_json::Value as JsonValue;
use spooky_config::{
    config::{
        ClientAuth, Config, ControlApi, Listen, LoadBalancing, Log, LogFormat, MetricsEndpoint,
        Observability, Security, Tls, Upstream, UpstreamTls,
    },
    runtime::RuntimeConfig,
    validator::validate,
};
use spooky_edge::{
    MAX_DATAGRAM_SIZE_BYTES, MAX_UDP_PAYLOAD_BYTES, QUIC_IDLE_TIMEOUT_MS, QUIC_INITIAL_MAX_DATA,
    QUIC_INITIAL_MAX_STREAMS_BIDI, QUIC_INITIAL_MAX_STREAMS_UNI, QUIC_INITIAL_STREAM_DATA,
    runtime::{
        bundle::RuntimeBundleHandle,
        listener::QUICListener as RuntimeListener,
        policy::{LifecycleTransitionResult, RuntimeLifecyclePhase},
    },
};
use tempfile::{TempDir, tempdir};
use tokio::net::TcpStream;
use tokio_rustls::{
    TlsConnector,
    rustls::{ClientConfig, RootCertStore, pki_types::ServerName},
};

use super::request_path::{
    BackendFixture, H3RequestSpec, H3Response, ListenerTaskGuard, TestTlsMaterial,
    run_request_to, start_h1_backend,
};

pub struct RuntimeSwapHarness {
    backends: Vec<BackendFixture>,
    listener_task: Option<ListenerTaskGuard>,
    rt: tokio::runtime::Runtime,
    tls: TestTlsMaterial,
    config_dir: TempDir,
    config_path: PathBuf,
    listen_port: u16,
    metrics_port: u16,
    control_api_port: u16,
    current_config: Option<Config>,
    runtime_bundle: Option<Arc<RuntimeBundleHandle>>,
    listen_addr: Option<SocketAddr>,
}

impl RuntimeSwapHarness {
    pub fn new() -> Self {
        let config_dir = tempdir().expect("runtime swap tempdir");
        Self {
            backends: Vec::new(),
            listener_task: None,
            rt: tokio::runtime::Runtime::new().expect("runtime"),
            tls: TestTlsMaterial::localhost(),
            config_path: config_dir.path().join("spooky-runtime-swap.yaml"),
            config_dir,
            listen_port: reserve_udp_port(),
            metrics_port: reserve_tcp_port(),
            control_api_port: reserve_tcp_port(),
            current_config: None,
            runtime_bundle: None,
            listen_addr: None,
        }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn make_config(&self, upstreams: HashMap<String, Upstream>) -> Config {
        Config {
            version: 1,
            listen: Listen {
                protocol: "http3".to_string(),
                port: self.listen_port,
                address: "127.0.0.1".to_string(),
                tls: Tls {
                    cert: self.tls.cert_path.clone(),
                    key: self.tls.key_path.clone(),
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
            upstream_tls: UpstreamTls::default(),
            log: Log {
                level: "info".to_string(),
                file: Default::default(),
                format: LogFormat::Plain,
            },
            performance: spooky_config::config::Performance::default(),
            observability: Observability {
                metrics: MetricsEndpoint {
                    enabled: true,
                    required: true,
                    address: "127.0.0.1".to_string(),
                    port: self.metrics_port,
                    path: "/metrics".to_string(),
                    max_connections: 32,
                    connection_timeout_ms: 5_000,
                },
                control_api: ControlApi {
                    enabled: true,
                    required: true,
                    address: "127.0.0.1".to_string(),
                    port: self.control_api_port,
                    health_path: "/health".to_string(),
                    ready_path: "/ready".to_string(),
                    runtime_path: "/runtime".to_string(),
                    restart_path: "/restart".to_string(),
                    reload_path: "/reload".to_string(),
                    reload_certs_path: "/reload-certs".to_string(),
                    auth_token: Some("runtime-swap-token".to_string()),
                    max_connections: 32,
                    connection_timeout_ms: 5_000,
                },
                ..Observability::default()
            },
            resilience: spooky_config::config::Resilience::default(),
            security: Security::default(),
        }
    }

    pub fn start_h1_backend<F, Fut>(&mut self, handler: F) -> SocketAddr
    where
        F: Fn(Request<Incoming>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response<Full<Bytes>>, Infallible>> + Send + 'static,
    {
        let fixture = self.rt.block_on(start_h1_backend(handler));
        let addr = fixture.addr;
        self.backends.push(fixture);
        addr
    }

    pub fn start_h1_static_backend(&mut self, body: &'static [u8]) -> SocketAddr {
        self.start_h1_backend(move |_req| async move {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(body))))
        })
    }

    pub fn start_listener(&mut self, config: Config) -> Result<SocketAddr, String> {
        validate(&config).map_err(|err| format!("config validation failed: {err}"))?;
        self.write_config_file(&config)?;

        let runtime_config =
            RuntimeConfig::from_config(&config).map_err(|err| format!("runtime config: {err}"))?;
        let runtime_bundle = RuntimeListener::build_runtime_bundle(
            self.config_path.to_string_lossy().to_string(),
            config.log.clone(),
            &runtime_config,
        )
        .map_err(|err| format!("runtime bundle: {err}"))?;
        let runtime_bundle_handle = Arc::new(RuntimeBundleHandle::new(runtime_bundle.clone()));
        let listener_config = runtime_bundle
            .runtime_config
            .primary_listener_runtime_config()
            .ok_or_else(|| "missing primary listener runtime config".to_string())?;
        let socket = RuntimeListener::bind_socket(&listener_config, false)
            .map_err(|err| format!("bind socket: {err}"))?;

        {
            let _enter = self.rt.enter();
            RuntimeListener::spawn_control_plane_tasks_with_runtime_bundle(
                &runtime_bundle.runtime_config,
                runtime_bundle.shared_state.as_ref(),
                Arc::clone(&runtime_bundle_handle),
                1,
            )
            .map_err(|err| format!("control plane tasks: {err}"))?;
        }

        let listener = RuntimeListener::new_with_socket_and_shared_state(
            listener_config,
            socket,
            Arc::clone(&runtime_bundle.shared_state),
        )
        .map_err(|err| format!("listener: {err}"))?
        .with_runtime_bundle(Arc::clone(&runtime_bundle_handle));

        let listen_addr = listener
            .socket
            .local_addr()
            .map_err(|err| format!("listen addr: {err}"))?;

        self.listener_task = Some(ListenerTaskGuard::spawn(&self.rt, listener));
        self.listen_addr = Some(listen_addr);
        self.current_config = Some(config);
        self.runtime_bundle = Some(runtime_bundle_handle);
        Ok(listen_addr)
    }

    pub fn rewrite_config<F>(&mut self, edit: F) -> Result<(), String>
    where
        F: FnOnce(&mut Config),
    {
        let mut config = self
            .current_config
            .clone()
            .ok_or_else(|| "listener not started".to_string())?;
        edit(&mut config);
        validate(&config).map_err(|err| format!("config validation failed: {err}"))?;
        self.write_config_file(&config)?;
        self.current_config = Some(config);
        Ok(())
    }

    pub fn runtime_snapshot(&self) -> Result<JsonValue, String> {
        let path = self
            .current_config
            .as_ref()
            .ok_or_else(|| "listener not started".to_string())?
            .observability
            .control_api
            .runtime_path
            .clone();
        let token = self.control_api_token()?;
        self.rt.block_on(self.poll_control_api_json(
            Method::GET,
            path,
            token,
            StatusCode::OK,
            Duration::from_secs(5),
        ))
    }

    pub fn trigger_runtime_reload(&self) -> Result<JsonValue, String> {
        self.trigger_runtime_reload_expect(StatusCode::ACCEPTED)
    }

    pub fn trigger_runtime_reload_expect(
        &self,
        expected_status: StatusCode,
    ) -> Result<JsonValue, String> {
        let path = self
            .current_config
            .as_ref()
            .ok_or_else(|| "listener not started".to_string())?
            .observability
            .control_api
            .reload_path
            .clone();
        let token = self.control_api_token()?;
        self.rt.block_on(self.poll_control_api_json(
            Method::POST,
            path,
            token,
            expected_status,
            Duration::from_secs(5),
        ))
    }

    pub fn metrics_text(&self) -> Result<String, String> {
        let path = self
            .current_config
            .as_ref()
            .ok_or_else(|| "listener not started".to_string())?
            .observability
            .metrics
            .path
            .clone();
        self.metrics_text_at(&path)
    }

    pub fn metrics_text_at(&self, path: &str) -> Result<String, String> {
        self.rt.block_on(
            self.poll_metrics_text(path.to_string(), StatusCode::OK, Duration::from_secs(5)),
        )
    }

    pub fn metrics_status_at(&self, path: &str) -> Result<StatusCode, String> {
        self.rt.block_on(self.poll_metrics_status(
            path.to_string(),
            Duration::from_secs(5),
        ))
    }

    pub fn run_request(&self, request: H3RequestSpec<'_>) -> Result<H3Response, String> {
        let listen_addr = self
            .listen_addr
            .ok_or_else(|| "listener not started".to_string())?;
        run_request_to(listen_addr, request)
    }

    pub fn listen_addr(&self) -> Result<SocketAddr, String> {
        self.listen_addr
            .ok_or_else(|| "listener not started".to_string())
    }

    pub fn current_generation(&self) -> Result<u64, String> {
        Ok(self
            .runtime_bundle
            .as_ref()
            .ok_or_else(|| "runtime bundle unavailable".to_string())?
            .current_generation())
    }

    pub fn lifecycle_phase(&self) -> Result<RuntimeLifecyclePhase, String> {
        Ok(self
            .runtime_bundle
            .as_ref()
            .ok_or_else(|| "runtime bundle unavailable".to_string())?
            .lifecycle()
            .phase())
    }

    pub fn begin_lifecycle_drain(&self) -> Result<LifecycleTransitionResult, String> {
        Ok(self
            .runtime_bundle
            .as_ref()
            .ok_or_else(|| "runtime bundle unavailable".to_string())?
            .lifecycle()
            .begin_drain())
    }

    pub fn request_watchdog_restart(&self, reason: &str) -> Result<bool, String> {
        Ok(self
            .runtime_bundle
            .as_ref()
            .ok_or_else(|| "runtime bundle unavailable".to_string())?
            .current_view()
            .shared_services()
            .watchdog
            .request_restart(reason))
    }

    pub fn fresh_quic_connection_establishes_within(
        &self,
        timeout: Duration,
    ) -> Result<bool, String> {
        let listen_addr = self.listen_addr()?;
        quic_connection_establishes_within(listen_addr, timeout)
    }

    fn control_api_token(&self) -> Result<String, String> {
        self.current_config
            .as_ref()
            .and_then(|config| config.observability.control_api.auth_token.clone())
            .ok_or_else(|| "missing control api auth token".to_string())
    }

    fn write_config_file(&self, config: &Config) -> Result<(), String> {
        let rendered = render_runtime_swap_config(config)?;
        std::fs::write(&self.config_path, rendered)
            .map_err(|err| format!("write config file '{}': {err}", self.config_path.display()))
    }

    async fn poll_metrics_text(
        &self,
        path: String,
        expected_status: StatusCode,
        timeout: Duration,
    ) -> Result<String, String> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.metrics_port));
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            match metrics_request_once(addr, &path).await {
                Ok((status, body)) if status == expected_status && !body.is_empty() => {
                    return Ok(body);
                }
                Ok((status, body)) => {
                    last_error = format!("unexpected metrics response {status} body={body:?}");
                }
                Err(err) => {
                    last_error = err;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Err(format!(
            "metrics endpoint not reachable within {:?} ({})",
            timeout, last_error
        ))
    }

    async fn poll_metrics_status(
        &self,
        path: String,
        timeout: Duration,
    ) -> Result<StatusCode, String> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.metrics_port));
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            match metrics_request_once(addr, &path).await {
                Ok((status, _body)) => return Ok(status),
                Err(err) => last_error = err,
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Err(format!(
            "metrics endpoint status not reachable within {:?} ({})",
            timeout, last_error
        ))
    }

    async fn poll_control_api_json(
        &self,
        method: Method,
        path: String,
        token: String,
        expected_status: StatusCode,
        timeout: Duration,
    ) -> Result<JsonValue, String> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.control_api_port));
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            match control_api_request_once(
                addr,
                &self.tls.cert_path,
                method.clone(),
                &path,
                &token,
            )
            .await
            {
                Ok((status, body)) if status == expected_status => return Ok(body),
                Ok((status, body)) => {
                    last_error = format!("unexpected control api response {status} body={body}");
                }
                Err(err) => {
                    last_error = err;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Err(format!(
            "control api '{}' not reachable within {:?} ({})",
            path, timeout, last_error
        ))
    }
}

fn reserve_tcp_port() -> u16 {
    StdTcpListener::bind(("127.0.0.1", 0))
        .expect("reserve tcp port")
        .local_addr()
        .expect("tcp local addr")
        .port()
}

fn reserve_udp_port() -> u16 {
    StdUdpSocket::bind(("127.0.0.1", 0))
        .expect("reserve udp port")
        .local_addr()
        .expect("udp local addr")
        .port()
}

fn read_test_root_store(cert_path: &str) -> Result<RootCertStore, String> {
    let mut roots = RootCertStore::empty();
    let certs = CertificateDer::pem_file_iter(cert_path)
        .map_err(|err| format!("open cert file: {err}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("parse certs: {err}"))?;

    for cert in certs {
        roots
            .add(cert)
            .map_err(|err| format!("add root cert: {err}"))?;
    }

    Ok(roots)
}

async fn metrics_request_once(addr: SocketAddr, path: &str) -> Result<(StatusCode, String), String> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|err| format!("metrics connect: {err}"))?;
    let (mut sender, conn) = http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|err| format!("metrics handshake: {err}"))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let request = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("host", "127.0.0.1")
        .body(Empty::<Bytes>::new())
        .map_err(|err| format!("metrics request build: {err}"))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|err| format!("metrics request: {err}"))?;
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|err| format!("metrics read body: {err}"))?
        .to_bytes();
    let body = String::from_utf8(body.to_vec()).map_err(|err| format!("metrics utf8: {err}"))?;
    Ok((status, body))
}

async fn control_api_request_once(
    addr: SocketAddr,
    cert_path: &str,
    method: Method,
    path: &str,
    token: &str,
) -> Result<(StatusCode, JsonValue), String> {
    let roots = read_test_root_store(cert_path)?;
    let tls_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name = ServerName::try_from("localhost")
        .map_err(|err| format!("server name: {err}"))?
        .to_owned();

    let stream = TcpStream::connect(addr)
        .await
        .map_err(|err| format!("control api connect: {err}"))?;
    let tls_stream = connector
        .connect(server_name, stream)
        .await
        .map_err(|err| format!("control api tls connect: {err}"))?;
    let (mut sender, conn) = http1::handshake(TokioIo::new(tls_stream))
        .await
        .map_err(|err| format!("control api handshake: {err}"))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("authorization", format!("Bearer {token}"))
        .body(Empty::<Bytes>::new())
        .map_err(|err| format!("control api request build: {err}"))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|err| format!("control api request: {err}"))?;
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|err| format!("control api read body: {err}"))?
        .to_bytes();
    let json = serde_json::from_slice(&body).map_err(|err| {
        format!(
            "control api json parse: {err}; payload={}",
            String::from_utf8_lossy(&body)
        )
    })?;
    Ok((status, json))
}

fn quic_connection_establishes_within(addr: SocketAddr, timeout: Duration) -> Result<bool, String> {
    let socket = StdUdpSocket::bind("0.0.0.0:0").map_err(|err| format!("udp bind: {err}"))?;
    let local_addr = socket
        .local_addr()
        .map_err(|err| format!("udp local addr: {err}"))?;

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

    let scid_bytes = [7u8; quiche::MAX_CONN_ID_LEN];
    let scid = quiche::ConnectionId::from_ref(&scid_bytes);
    let mut conn = quiche::connect(Some("localhost"), &scid, local_addr, addr, &mut config)
        .map_err(|err| format!("connect: {err:?}"))?;

    let mut out = [0u8; MAX_UDP_PAYLOAD_BYTES];
    let mut buf = [0u8; MAX_DATAGRAM_SIZE_BYTES];

    let (written, send_info) = conn.send(&mut out).map_err(|err| format!("send: {err:?}"))?;
    socket
        .send_to(&out[..written], send_info.to)
        .map_err(|err| format!("send_to: {err}"))?;

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        socket
            .set_read_timeout(Some(remaining.min(Duration::from_millis(50))))
            .map_err(|err| format!("read timeout: {err}"))?;

        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                let _ = conn.recv(
                    &mut buf[..len],
                    quiche::RecvInfo {
                        from,
                        to: local_addr,
                    },
                );
            }
            Err(ref err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                conn.on_timeout();
            }
            Err(err) => return Err(format!("recv_from: {err}")),
        }

        loop {
            match conn.send(&mut out) {
                Ok((written, send_info)) => {
                    let _ = socket.send_to(&out[..written], send_info.to);
                }
                Err(quiche::Error::Done) => break,
                Err(err) => return Err(format!("send loop: {err:?}")),
            }
        }

        if conn.is_established() {
            return Ok(true);
        }
    }

    Ok(false)
}

fn render_runtime_swap_config(config: &Config) -> Result<String, String> {
    let mut yaml = String::new();
    writeln!(&mut yaml, "version: {}", config.version).map_err(|err| err.to_string())?;
    writeln!(&mut yaml, "listen:").map_err(|err| err.to_string())?;
    writeln!(&mut yaml, "  protocol: {}", config.listen.protocol).map_err(|err| err.to_string())?;
    writeln!(&mut yaml, "  address: \"{}\"", config.listen.address).map_err(|err| err.to_string())?;
    writeln!(&mut yaml, "  port: {}", config.listen.port).map_err(|err| err.to_string())?;
    writeln!(&mut yaml, "  tls:").map_err(|err| err.to_string())?;
    writeln!(&mut yaml, "    cert: \"{}\"", config.listen.tls.cert).map_err(|err| err.to_string())?;
    writeln!(&mut yaml, "    key: \"{}\"", config.listen.tls.key).map_err(|err| err.to_string())?;

    if let Some(load_balancing) = &config.load_balancing {
        writeln!(&mut yaml, "load_balancing:").map_err(|err| err.to_string())?;
        writeln!(&mut yaml, "  type: {}", load_balancing.lb_type).map_err(|err| err.to_string())?;
    }

    writeln!(&mut yaml, "upstream:").map_err(|err| err.to_string())?;
    for (name, upstream) in &config.upstream {
        writeln!(&mut yaml, "  {}:", yaml_scalar(name)).map_err(|err| err.to_string())?;
        writeln!(&mut yaml, "    load_balancing:").map_err(|err| err.to_string())?;
        writeln!(
            &mut yaml,
            "      type: {}",
            upstream.load_balancing.lb_type
        )
        .map_err(|err| err.to_string())?;
        writeln!(&mut yaml, "    route:").map_err(|err| err.to_string())?;
        if let Some(path_prefix) = upstream.route.path_prefix.as_deref() {
            writeln!(
                &mut yaml,
                "      path_prefix: \"{}\"",
                path_prefix
            )
            .map_err(|err| err.to_string())?;
        }
        writeln!(&mut yaml, "    backends:").map_err(|err| err.to_string())?;
        for backend in &upstream.backends {
            writeln!(&mut yaml, "      - id: {}", yaml_scalar(&backend.id))
                .map_err(|err| err.to_string())?;
            writeln!(
                &mut yaml,
                "        address: \"{}\"",
                backend.address
            )
            .map_err(|err| err.to_string())?;
            writeln!(&mut yaml, "        weight: {}", backend.weight)
                .map_err(|err| err.to_string())?;
        }
    }

    writeln!(&mut yaml, "log:").map_err(|err| err.to_string())?;
    writeln!(&mut yaml, "  level: {}", yaml_scalar(&config.log.level)).map_err(|err| err.to_string())?;
    writeln!(&mut yaml, "  file:").map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    enabled: {}",
        config.log.file.enabled
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    path: \"{}\"",
        config.log.file.path
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "  format: {}",
        match config.log.format {
            LogFormat::Plain => "plain",
            LogFormat::Json => "json",
        }
    )
    .map_err(|err| err.to_string())?;

    writeln!(&mut yaml, "performance:").map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "  new_connections_burst: {}",
        config.performance.new_connections_burst
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "  new_connections_per_sec: {}",
        config.performance.new_connections_per_sec
    )
    .map_err(|err| err.to_string())?;

    writeln!(&mut yaml, "observability:").map_err(|err| err.to_string())?;
    writeln!(&mut yaml, "  metrics:").map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    enabled: {}",
        config.observability.metrics.enabled
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    required: {}",
        config.observability.metrics.required
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    address: \"{}\"",
        config.observability.metrics.address
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    port: {}",
        config.observability.metrics.port
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    path: \"{}\"",
        config.observability.metrics.path
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    max_connections: {}",
        config.observability.metrics.max_connections
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    connection_timeout_ms: {}",
        config.observability.metrics.connection_timeout_ms
    )
    .map_err(|err| err.to_string())?;

    writeln!(&mut yaml, "  control_api:").map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    enabled: {}",
        config.observability.control_api.enabled
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    required: {}",
        config.observability.control_api.required
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    address: \"{}\"",
        config.observability.control_api.address
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    port: {}",
        config.observability.control_api.port
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    health_path: \"{}\"",
        config.observability.control_api.health_path
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    ready_path: \"{}\"",
        config.observability.control_api.ready_path
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    runtime_path: \"{}\"",
        config.observability.control_api.runtime_path
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    restart_path: \"{}\"",
        config.observability.control_api.restart_path
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    reload_path: \"{}\"",
        config.observability.control_api.reload_path
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    reload_certs_path: \"{}\"",
        config.observability.control_api.reload_certs_path
    )
    .map_err(|err| err.to_string())?;
    if let Some(token) = config.observability.control_api.auth_token.as_deref() {
        writeln!(&mut yaml, "    auth_token: \"{}\"", token).map_err(|err| err.to_string())?;
    }
    writeln!(
        &mut yaml,
        "    max_connections: {}",
        config.observability.control_api.max_connections
    )
    .map_err(|err| err.to_string())?;
    writeln!(
        &mut yaml,
        "    connection_timeout_ms: {}",
        config.observability.control_api.connection_timeout_ms
    )
    .map_err(|err| err.to_string())?;

    Ok(yaml)
}

fn yaml_scalar(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        value.to_string()
    } else {
        format!("{value:?}")
    }
}
