#![allow(dead_code)]

use std::{
    collections::HashMap,
    net::TcpListener as StdTcpListener,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use spooky_config::runtime::{RuntimeBackendConnectionPolicy, RuntimeBackendTransportKind};
use spooky_transport::{SharedDnsResolver, UpstreamTransportPool};
use tokio::net::TcpListener;

#[derive(Clone, Copy)]
enum TestServerProtocol {
    Http1,
    H2,
}

#[derive(Clone, Copy)]
pub enum TransportTestProtocol {
    Http1,
    H2,
}

impl TransportTestProtocol {
    fn server_protocol(self) -> TestServerProtocol {
        match self {
            Self::Http1 => TestServerProtocol::Http1,
            Self::H2 => TestServerProtocol::H2,
        }
    }

    pub fn runtime_kind(self) -> RuntimeBackendTransportKind {
        match self {
            Self::Http1 => RuntimeBackendTransportKind::Http1,
            Self::H2 => RuntimeBackendTransportKind::H2,
        }
    }
}

pub struct ConcurrencyTracker {
    current: AtomicUsize,
    max: AtomicUsize,
}

impl ConcurrencyTracker {
    pub fn new() -> Self {
        Self {
            current: AtomicUsize::new(0),
            max: AtomicUsize::new(0),
        }
    }

    pub fn enter(&self) {
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        let mut prev = self.max.load(Ordering::SeqCst);
        while now > prev {
            match self
                .max
                .compare_exchange(prev, now, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(next) => prev = next,
            }
        }
    }

    pub fn exit(&self) {
        self.current.fetch_sub(1, Ordering::SeqCst);
    }

    pub fn max_observed(&self) -> usize {
        self.max.load(Ordering::SeqCst)
    }
}

pub fn loopback_bind_restricted(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::PermissionDenied
        || matches!(err.raw_os_error(), Some(1) | Some(13))
}

pub fn request(uri: &str) -> Request<BoxBody<Bytes, std::convert::Infallible>> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Full::new(Bytes::new()).boxed())
        .expect("request")
}

pub fn request_to_backend(
    backend: &str,
) -> Request<BoxBody<Bytes, std::convert::Infallible>> {
    request(&format!("http://{backend}/"))
}

pub fn reserve_unused_port() -> u16 {
    StdTcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

pub fn connection_policy(max_inflight: usize) -> RuntimeBackendConnectionPolicy {
    RuntimeBackendConnectionPolicy {
        max_inflight,
        max_idle_per_backend: 64,
        pool_idle_timeout: Duration::from_secs(30),
        connect_timeout: Duration::from_secs(2),
        execution_timeout: Duration::from_secs(5),
    }
}

pub fn build_pool(
    backends: impl IntoIterator<Item = (String, RuntimeBackendTransportKind)>,
    max_inflight: usize,
    resolver: SharedDnsResolver,
) -> UpstreamTransportPool {
    build_pool_with_policy(backends, connection_policy(max_inflight), resolver)
}

pub fn build_pool_with_policy(
    backends: impl IntoIterator<Item = (String, RuntimeBackendTransportKind)>,
    connection_policy: RuntimeBackendConnectionPolicy,
    resolver: SharedDnsResolver,
) -> UpstreamTransportPool {
    UpstreamTransportPool::new_from_runtime_backends(
        backends,
        HashMap::new(),
        connection_policy,
        resolver,
    )
    .expect("transport pool")
}

pub fn build_single_backend_pool(
    backend: String,
    transport_kind: RuntimeBackendTransportKind,
    max_inflight: usize,
    resolver: SharedDnsResolver,
) -> UpstreamTransportPool {
    build_pool([(backend, transport_kind)], max_inflight, resolver)
}

pub async fn read_body(response: Response<Incoming>) -> Bytes {
    response
        .into_body()
    .collect()
        .await
        .expect("collect body")
        .to_bytes()
}

pub fn backend_address(port: u16) -> String {
    format!("127.0.0.1:{port}")
}

pub async fn start_backend_server(
    protocol: TransportTestProtocol,
    body: &'static [u8],
    delay: Duration,
    tracker: Option<Arc<ConcurrencyTracker>>,
) -> std::io::Result<String> {
    let port = start_server(protocol.server_protocol(), body, delay, tracker).await?;
    Ok(backend_address(port))
}

pub async fn start_backend_pool(
    protocol: TransportTestProtocol,
    body: &'static [u8],
    delay: Duration,
    tracker: Option<Arc<ConcurrencyTracker>>,
    max_inflight: usize,
    resolver: SharedDnsResolver,
) -> std::io::Result<(String, UpstreamTransportPool)> {
    let backend = start_backend_server(protocol, body, delay, tracker).await?;
    let pool = build_single_backend_pool(
        backend.clone(),
        protocol.runtime_kind(),
        max_inflight,
        resolver,
    );
    Ok((backend, pool))
}

pub async fn start_shared_backend_pool(
    protocol: TransportTestProtocol,
    body: &'static [u8],
    delay: Duration,
    tracker: Option<Arc<ConcurrencyTracker>>,
    max_inflight: usize,
    resolver: SharedDnsResolver,
) -> std::io::Result<(String, Arc<UpstreamTransportPool>)> {
    let (backend, pool) =
        start_backend_pool(protocol, body, delay, tracker, max_inflight, resolver).await?;
    Ok((backend, Arc::new(pool)))
}

pub async fn start_h1_server(
    body: &'static [u8],
    delay: Duration,
    tracker: Option<Arc<ConcurrencyTracker>>,
) -> std::io::Result<u16> {
    start_server(TransportTestProtocol::Http1.server_protocol(), body, delay, tracker).await
}

pub async fn start_h2_server(
    body: &'static [u8],
    delay: Duration,
    tracker: Option<Arc<ConcurrencyTracker>>,
) -> std::io::Result<u16> {
    start_server(TransportTestProtocol::H2.server_protocol(), body, delay, tracker).await
}

async fn start_server(
    protocol: TestServerProtocol,
    body: &'static [u8],
    delay: Duration,
    tracker: Option<Arc<ConcurrencyTracker>>,
) -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let tracker = tracker.clone();
            let service = service_fn(move |_req: Request<Incoming>| {
                let tracker = tracker.clone();
                async move {
                    if let Some(tracker) = &tracker {
                        tracker.enter();
                    }
                    tokio::time::sleep(delay).await;
                    if let Some(tracker) = &tracker {
                        tracker.exit();
                    }
                    Ok::<_, std::convert::Infallible>(Response::new(Full::new(Bytes::from_static(
                        body,
                    ))))
                }
            });

            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                match protocol {
                    TestServerProtocol::Http1 => {
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(io, service)
                            .await;
                    }
                    TestServerProtocol::H2 => {
                        let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                            .serve_connection(io, service)
                            .await;
                    }
                }
            });
        }
    });

    Ok(port)
}
