use bytes::Bytes;
use core::net::SocketAddr;
use spooky_config::{backend_endpoint::BackendEndpoint, config::Config};
use spooky_errors::ProxyError;
use spooky_lb::UpstreamPool;
use spooky_transport::h2_pool::H2Pool;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::UdpSocket,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tracing::Span;

use crate::Metrics;
use crate::RetryReason;
use crate::cid_radix::CidRadix;
use crate::constants::MAX_DATAGRAM_SIZE_BYTES;
use crate::resilience::{AdaptivePermit, RouteQueuePermit, RuntimeResilience};
use crate::route_index::{self, RouteDecisionReason};
use crate::watchdog::WatchdogCoordinator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendLbKind {
    RoundRobin,
    ConsistentHash,
    Random,
    LeastConnections,
    LatencyAware,
    StickyCid,
    Unknown,
}

impl BackendLbKind {
    pub fn from_name(value: &str) -> Self {
        match value {
            "round-robin" => Self::RoundRobin,
            "consistent-hash" => Self::ConsistentHash,
            "random" => Self::Random,
            "least-connections" => Self::LeastConnections,
            "latency-aware" => Self::LatencyAware,
            "sticky-cid" => Self::StickyCid,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RoundRobin => "round-robin",
            Self::ConsistentHash => "consistent-hash",
            Self::Random => "random",
            Self::LeastConnections => "least-connections",
            Self::LatencyAware => "latency-aware",
            Self::StickyCid => "sticky-cid",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub enum LoadBalancingKeyPolicy {
    Default,
    Path,
    Authority,
    Method,
    Header(String),
}

impl LoadBalancingKeyPolicy {
    pub fn from_config(raw: Option<&str>) -> Self {
        let Some(raw) = raw else {
            return Self::Default;
        };
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "" => Self::Default,
            "default" => Self::Default,
            "path" => Self::Path,
            "authority" | "host" => Self::Authority,
            "method" => Self::Method,
            _ => {
                if let Some(header) = normalized.strip_prefix("header:") {
                    if header.is_empty() {
                        return Self::Default;
                    }
                    Self::Header(header.to_string())
                } else {
                    Self::Default
                }
            }
        }
    }
}

pub struct SharedRuntimeState {
    pub(crate) h2_pool: Arc<H2Pool>,
    pub(crate) backend_endpoints: Arc<HashMap<String, BackendEndpoint>>,
    pub(crate) upstream_pools: HashMap<String, Arc<RwLock<UpstreamPool>>>,
    pub(crate) upstream_lb_keys: HashMap<String, LoadBalancingKeyPolicy>,
    pub(crate) upstream_inflight: HashMap<String, Arc<Semaphore>>,
    pub(crate) global_inflight: Arc<Semaphore>,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) resilience: Arc<RuntimeResilience>,
    pub(crate) watchdog: Arc<WatchdogCoordinator>,
}

impl SharedRuntimeState {
    pub fn bind_metrics_worker_slot(&self, slot: usize) {
        self.metrics.bind_worker_slot(slot);
    }

    pub fn inc_ingress_queue_drop(&self) {
        self.metrics.inc_ingress_queue_drop();
    }

    pub fn inc_ingress_queue_drop_bytes(&self, bytes: usize) {
        self.metrics.inc_ingress_queue_drop_bytes(bytes);
    }

    pub fn set_ingress_queue_bytes(&self, bytes: usize) {
        self.metrics.set_ingress_queue_bytes(bytes);
    }

    pub fn snapshot_backend_health(&self) -> (usize, usize) {
        let mut healthy = 0usize;
        let mut total = 0usize;

        for pool in self.upstream_pools.values() {
            let guard = match pool.read() {
                Ok(guard) => guard,
                Err(_) => continue,
            };
            let pool_total = guard.pool.len();
            total = total.saturating_add(pool_total);
            healthy = healthy.saturating_add(guard.pool.healthy_len().min(pool_total));
        }

        (healthy, total)
    }
}

pub struct QUICListener {
    pub socket: UdpSocket,
    pub local_addr: SocketAddr,
    pub config: Config,
    pub quic_config: quiche::Config,
    pub h3_config: Arc<quiche::h3::Config>,
    pub h2_pool: Arc<H2Pool>,
    pub backend_endpoints: Arc<HashMap<String, BackendEndpoint>>,
    pub upstream_pools: HashMap<String, Arc<RwLock<UpstreamPool>>>,
    pub upstream_lb_keys: HashMap<String, LoadBalancingKeyPolicy>,
    pub upstream_inflight: HashMap<String, Arc<Semaphore>>,
    pub global_inflight: Arc<Semaphore>,
    pub(crate) routing_index: route_index::RouteIndex,
    pub metrics: Arc<Metrics>,
    pub resilience: Arc<RuntimeResilience>,
    pub watchdog: Arc<WatchdogCoordinator>,
    pub draining: bool,
    pub drain_start: Option<Instant>,
    pub watchdog_worker_drained: bool,
    pub drain_timeout: Duration,
    pub backend_timeout: Duration,
    pub backend_body_idle_timeout: Duration,
    pub backend_body_total_timeout: Duration,
    pub client_body_idle_timeout: Duration,
    pub backend_total_request_timeout: Duration,
    pub max_active_connections: usize,
    pub max_request_body_bytes: usize,
    pub max_response_body_bytes: usize,
    pub request_buffer_global_cap_bytes: usize,
    pub unknown_length_response_prebuffer_bytes: usize,
    pub require_client_cert: bool,
    pub response_chunk_buffer_pool: Arc<Mutex<Vec<Vec<Bytes>>>>,

    pub recv_buf: [u8; MAX_DATAGRAM_SIZE_BYTES],
    pub send_buf: [u8; MAX_DATAGRAM_SIZE_BYTES],

    pub connections: HashMap<Arc<[u8]>, QuicConnection>, // KEY: SCID(server connection id)
    pub cid_routes: HashMap<Arc<[u8]>, Arc<[u8]>>,       // KEY: alias SCID, VALUE: primary SCID
    pub peer_routes: HashMap<SocketAddr, Arc<[u8]>>,     // KEY: peer address, VALUE: primary SCID
    pub cid_radix: CidRadix,
    pub(crate) conn_rate_limiter: crate::quic_listener::TokenBucket,
}

pub struct QuicConnection {
    pub quic: quiche::Connection,
    pub h3: Option<quiche::h3::Connection>,
    pub h3_config: Arc<quiche::h3::Config>,
    pub streams: HashMap<u64, RequestEnvelope>,

    pub peer_address: SocketAddr,
    pub last_activity: Instant,
    pub primary_scid: Arc<[u8]>,
    pub sticky_cid_key: Arc<str>,
    pub routing_scids: HashSet<Arc<[u8]>>,
    pub packets_since_rotation: u64,
    pub last_scid_rotation: Instant,
}

/// Result type returned by the in-flight H2 forwarding task.
pub type ForwardResult =
    Result<(http::StatusCode, http::HeaderMap, hyper::body::Incoming), ProxyError>;

#[derive(Debug, Clone, Copy, Default)]
pub struct HedgeTelemetry {
    pub launched: bool,
    pub hedge_won: bool,
    pub hedge_wasted: bool,
    pub primary_won_after_trigger: bool,
    pub primary_late_ms: u64,
}

pub struct UpstreamResult {
    pub forward: ForwardResult,
    pub hedge: HedgeTelemetry,
    pub retry_count: u8,
    /// Set when a retry was attempted; the error reason that triggered it.
    pub retry_attempt_reason: Option<RetryReason>,
    /// Set when a retry was denied; the first denial reason encountered.
    pub retry_denial_reason: Option<RetryReason>,
}

/// Lifecycle phase of a single HTTP/3 request stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamPhase {
    /// Still receiving request headers/body from the QUIC client.
    ReceivingRequest,
    /// Request fully received; waiting for the upstream H2 response.
    AwaitingUpstream,
    /// Upstream responded; streaming response back to the QUIC client.
    SendingResponse,
    /// Stream finished cleanly.
    Completed,
    /// Stream terminated with an error.
    Failed,
}

/// A chunk of the upstream response being streamed back to the client.
#[derive(Debug)]
pub enum ResponseChunk {
    /// Emit downstream response headers (used when headers are deferred until
    /// body-size validation completes).
    Start {
        status: http::StatusCode,
        headers: Vec<(Vec<u8>, Vec<u8>)>,
    },
    Data(Bytes),
    End,
    Error(ProxyError),
}

pub type DeferredResponseHeaders = (http::StatusCode, Vec<(Vec<u8>, Vec<u8>)>);

pub struct RequestEnvelope {
    pub request_id: u64,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub traceparent: Option<String>,
    pub trace_span: Option<Span>,

    pub method: String,
    pub path: String,
    pub route_path_end: usize,
    pub authority: Option<String>,
    /// Sender half of the body channel.  Dropping it signals end-of-body to hyper.
    pub body_tx: Option<mpsc::Sender<Bytes>>,
    /// Body chunks that arrived before the channel had capacity.
    pub body_buf: VecDeque<Bytes>,
    /// Current bytes held in `body_buf`.
    pub body_buf_bytes: usize,
    /// Total body bytes received on this stream (buffered + already forwarded).
    pub body_bytes_received: usize,
    /// Last observed request-body byte arrival time.
    pub last_body_activity: Instant,
    /// Resolved backend address and index (for health marking on response).
    pub backend_addr: Option<String>,
    pub backend_index: Option<usize>,
    pub upstream_name: Option<String>,
    pub(crate) route_reason: Option<RouteDecisionReason>,
    pub route_path_len: Option<usize>,
    pub route_host_specific: Option<bool>,
    pub backend_lb: Option<BackendLbKind>,
    pub upstream_pool: Option<Arc<RwLock<UpstreamPool>>>,
    pub routing_transparency_enabled: bool,
    pub routing_transparency_include_reason: bool,
    pub response_status: Option<u16>,
    pub backend_request_finished: bool,
    pub global_inflight_permit: Option<OwnedSemaphorePermit>,
    pub upstream_inflight_permit: Option<OwnedSemaphorePermit>,
    pub adaptive_admission_permit: Option<AdaptivePermit>,
    pub route_queue_permit: Option<RouteQueuePermit>,
    pub start: Instant,
    pub total_request_deadline: Instant,
    pub bodyless_mode: bool,

    pub retry_count: u8,
    pub error_kind: Option<&'static str>,

    /// Current lifecycle phase of this stream.
    pub phase: StreamPhase,
    /// True once the client has sent FIN on the request stream.
    pub request_fin_received: bool,
    /// In-flight upstream forwarding task for this request stream.
    pub upstream_task: Option<tokio::task::JoinHandle<UpstreamResult>>,
    /// Upstream response body stream, polled directly from the worker loop.
    pub response_body: Option<hyper::body::Incoming>,
    /// Buffered response chunks awaiting QUIC write availability.
    pub response_chunk_queue: VecDeque<ResponseChunk>,
    /// True when headers should be deferred until full body validation completes.
    pub response_defer_headers_until_body_validated: bool,
    /// Deferred response status/headers for unknown-length bodies.
    pub response_pending_headers: Option<DeferredResponseHeaders>,
    /// Buffered unknown-length body chunks until validation completes.
    pub response_prebuffer_chunks: Vec<Bytes>,
    /// Total upstream response body bytes observed so far.
    pub response_bytes_received: usize,
    /// Last observed upstream response-body byte time.
    pub response_last_body_activity: Option<Instant>,
    /// Deadline for receiving the first upstream response body byte.
    pub response_first_byte_deadline: Option<Instant>,
    /// Whether any upstream response body bytes have been observed.
    pub response_body_started: bool,
    /// True once downstream response headers are emitted on this stream.
    pub response_headers_sent: bool,
    /// A chunk that could not be written due to QUIC send backpressure; retried next poll.
    pub pending_chunk: Option<ResponseChunk>,
}

impl RequestEnvelope {
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn route_path(&self) -> &str {
        &self.path[..self.route_path_end.min(self.path.len())]
    }
}

#[derive(Debug)]
pub enum HealthClassification {
    Success, // 2xx, 3xx responses
    Failure, // 5xx responses, Transport/Pool/Timeout errors
    Neutral, // 4xx responses, Bridge/TLS errors
}

pub fn outcome_from_status(status: http::StatusCode) -> HealthClassification {
    if status.is_server_error() {
        // 5xx
        HealthClassification::Failure
    } else if status.is_client_error() {
        // 4xx
        HealthClassification::Neutral
    } else {
        // 2xx, 3xx
        HealthClassification::Success
    }
}
