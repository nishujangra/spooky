use std::{collections::VecDeque, sync::Arc, time::Instant};

use bytes::Bytes;
use http::StatusCode;
use spooky_errors::ProxyError;
use tokio::{
    sync::{OwnedSemaphorePermit, mpsc, oneshot},
    task::AbortHandle,
};
use tracing::Span;

use crate::{
    OverloadShedReason,
    resilience::{adaptive_admission::AdaptivePermit, route_queue::RouteQueuePermit},
    runtime::connection::{
        auth::{ExternalAuthFailureDisposition, ExternalAuthResult},
        request::PendingForward,
        response::{ResponseChunk, UpstreamResult},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamPhase {
    /// Still receiving request headers/body from the QUIC client.
    ReceivingRequest,
    /// Request fully received; waiting for the upstream response.
    AwaitingUpstream,
    /// Upstream responded; streaming response back to the QUIC client.
    SendingResponse,
    /// Stream finished cleanly.
    Completed,
    /// Stream terminated.
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamAdmissionState {
    /// Request admission is still pending an external auth/authz decision.
    WaitingForAuth,
    /// Request cleared admission checks and may proceed to upstream forwarding.
    ReadyToForward,
    /// Request was denied by admission/auth checks and should not be forwarded.
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelMode {
    None,
    Connect,
    Websocket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    Normal,
    Bodyless,
    HeadLike,
    ConnectTunnel,
    WebsocketTunnel,
}

impl RequestMode {
    pub fn from_intake(
        tunnel_mode: TunnelMode,
        method: &str,
        content_length: Option<usize>,
    ) -> Self {
        match tunnel_mode {
            TunnelMode::Connect => Self::ConnectTunnel,
            TunnelMode::Websocket => Self::WebsocketTunnel,
            TunnelMode::None
                if content_length.unwrap_or(0) == 0 && method.eq_ignore_ascii_case("HEAD") =>
            {
                Self::HeadLike
            }
            TunnelMode::None
                if content_length.unwrap_or(0) == 0 && method.eq_ignore_ascii_case("GET") =>
            {
                Self::Bodyless
            }
            TunnelMode::None => Self::Normal,
        }
    }

    pub fn is_tunnel(self) -> bool {
        matches!(self, Self::ConnectTunnel | Self::WebsocketTunnel)
    }

    pub fn suppresses_response_body(self) -> bool {
        matches!(self, Self::HeadLike)
    }

    pub fn bodyless_mode(self) -> bool {
        matches!(self, Self::Bodyless | Self::HeadLike)
    }

    pub fn tunnel_mode(self) -> TunnelMode {
        match self {
            Self::Normal | Self::Bodyless | Self::HeadLike => TunnelMode::None,
            Self::ConnectTunnel => TunnelMode::Connect,
            Self::WebsocketTunnel => TunnelMode::Websocket,
        }
    }

    pub fn initial_body_state(self) -> RequestBodyState {
        if self.bodyless_mode() {
            RequestBodyState::FinReceived
        } else {
            RequestBodyState::Open
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestBodyState {
    Open,
    Buffered,
    FinReceived,
    ClosedToUpstream,
}

#[allow(dead_code)]
impl RequestBodyState {
    pub fn from_runtime(
        request_fin_received: bool,
        has_buffered_body: bool,
        forward_open: bool,
    ) -> Self {
        if forward_open {
            if has_buffered_body {
                Self::Buffered
            } else if request_fin_received {
                Self::FinReceived
            } else {
                Self::Open
            }
        } else if request_fin_received {
            Self::ClosedToUpstream
        } else if has_buffered_body {
            Self::Buffered
        } else {
            Self::Open
        }
    }

    pub fn on_buffered(self) -> Self {
        match self {
            Self::ClosedToUpstream => Self::ClosedToUpstream,
            _ => Self::Buffered,
        }
    }

    pub fn on_downstream_finished(self) -> Self {
        match self {
            Self::ClosedToUpstream => Self::ClosedToUpstream,
            _ => Self::FinReceived,
        }
    }

    pub fn on_forward_closed(self) -> Self {
        Self::ClosedToUpstream
    }

    pub fn request_fin_received(self) -> bool {
        matches!(self, Self::FinReceived | Self::ClosedToUpstream)
    }

    pub fn can_accept_downstream_body(self) -> bool {
        matches!(self, Self::Open | Self::Buffered)
    }

    pub fn forwarding_complete(self) -> bool {
        matches!(self, Self::ClosedToUpstream)
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseEmissionState {
    DeferredHeaders,
    HeadersSent,
    StreamingBody,
    TrailersPending,
    EndPending,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum ResponseBackpressureState {
    Ready,
    Blocked(ResponseChunk),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionReason {
    ResponseStreamFinished,
    ImmediateResponse,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationReason {
    ClientReset,
    ConnectionClosed,
    OperatorAbort,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutReason {
    RequestBodyIdle,
    RequestBodyTotal,
    ExternalAuth,
    TotalRequest,
    AwaitingUpstream,
    ResponseBodyIdle,
    ResponseBodyTotal,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionReason {
    AuthDenied,
    AuthUnavailable,
    ValidationFailed,
    RateLimited,
    Overloaded,
    RequestBodyNotAllowed,
    RequestBodyTooLarge,
    ResponsePrebufferCap,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendFailureReason {
    DispatchSpawnFailed,
    UpstreamResultChannelDropped,
    UpstreamTimeout,
    UpstreamTransport,
    UpstreamProtocol,
    UpstreamTls,
    UpstreamBridge,
    ResponseWriteFailed,
    ResponseStreamAborted,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RequestContext {
    pub request_id: u64,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub traceparent: Option<String>,
    pub trace_span: Option<Span>,
    pub method: String,
    pub path: String,
    pub authority: Option<String>,
    pub start: Instant,
    pub total_request_deadline: Instant,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RoutingSnapshot {
    pub backend_addr: String,
    pub backend_index: usize,
    pub upstream_name: String,
    pub route_reason: String,
    pub route_path_len: usize,
    pub route_host_specific: bool,
    pub backend_lb: Option<String>,
}

#[allow(dead_code)]
pub struct RequestIntakeState {
    pub context: RequestContext,
    pub request_mode: RequestMode,
    pub request_body: RequestBodyState,
}

#[allow(dead_code)]
pub struct AwaitingAuthState {
    pub context: RequestContext,
    pub routing: RoutingSnapshot,
    pub request_mode: RequestMode,
    pub request_body: RequestBodyState,
    pub request_body_runtime: RequestBodyRuntime,
    pub pending_forward: Arc<PendingForward>,
    pub(crate) auth_result_rx: oneshot::Receiver<ExternalAuthResult>,
    pub auth_abort: AbortHandle,
    pub auth_deadline: Instant,
    pub(crate) auth_disposition: ExternalAuthFailureDisposition,
}

impl AwaitingAuthState {
    pub(crate) fn poll_non_blocking(&mut self, now: Instant) -> Option<ExternalAuthResult> {
        if now >= self.auth_deadline {
            return Some(Err(ProxyError::Timeout));
        }

        match self.auth_result_rx.try_recv() {
            Ok(result) => Some(result),
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => Some(Err(ProxyError::Transport(
                "external auth task dropped sender".into(),
            ))),
        }
    }
}

#[allow(dead_code)]
pub struct DispatchReadyState {
    pub context: RequestContext,
    pub routing: RoutingSnapshot,
    pub request_mode: RequestMode,
    pub request_body: RequestBodyState,
    pub request_body_runtime: RequestBodyRuntime,
    pub pending_forward: Arc<PendingForward>,
}

pub struct RequestBodyRuntime {
    pub body_buf: VecDeque<Bytes>,
    pub body_buf_bytes: usize,
    pub body_bytes_received: usize,
    pub last_body_activity: Instant,
    pub request_fin_received: bool,
}

#[allow(dead_code)]
pub struct AdmissionPermits {
    pub global: OwnedSemaphorePermit,
    pub upstream: OwnedSemaphorePermit,
    pub adaptive: AdaptivePermit,
    pub route_queue: RouteQueuePermit,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendAccountingState {
    pub response_status: Option<StatusCode>,
    pub finalized: bool,
}

#[allow(dead_code)]
impl BackendAccountingState {
    pub fn should_finalize(self) -> bool {
        !self.finalized
    }
}

#[allow(dead_code)]
pub struct BackendDispatchState {
    pub body_tx: Option<mpsc::Sender<Bytes>>,
    pub(crate) upstream_result_rx: oneshot::Receiver<UpstreamResult>,
    pub backend_accounting: BackendAccountingState,
}

#[allow(dead_code)]
pub struct AdmittedState {
    pub context: RequestContext,
    pub routing: RoutingSnapshot,
    pub request_mode: RequestMode,
    pub request_body: RequestBodyState,
    pub request_body_runtime: RequestBodyRuntime,
    pub pending_forward: Arc<PendingForward>,
    pub permits: AdmissionPermits,
}

#[allow(dead_code)]
pub struct AwaitingUpstreamState {
    pub context: RequestContext,
    pub routing: RoutingSnapshot,
    pub request_mode: RequestMode,
    pub request_body: RequestBodyState,
    pub request_body_runtime: RequestBodyRuntime,
    pub pending_forward: Arc<PendingForward>,
    pub permits: AdmissionPermits,
    pub dispatch: BackendDispatchState,
}

#[allow(dead_code)]
pub struct ResponseStreamingState {
    pub context: RequestContext,
    pub routing: RoutingSnapshot,
    pub request_mode: RequestMode,
    pub request_body_runtime: RequestBodyRuntime,
    pub permits: AdmissionPermits,
    pub final_status: StatusCode,
    pub emission: ResponseEmissionState,
    pub(crate) response_chunk_rx: mpsc::Receiver<ResponseChunk>,
    pub(crate) backpressure: ResponseBackpressureState,
    pub backend_accounting: BackendAccountingState,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    pub context: RequestContext,
    pub routing: Option<RoutingSnapshot>,
    pub request_mode: RequestMode,
    pub response_status: Option<StatusCode>,
    pub overload_reason: Option<OverloadShedReason>,
    pub backend_accounting: Option<BackendAccountingState>,
}

#[allow(dead_code)]
pub struct CompletedState {
    pub reason: CompletionReason,
    pub snapshot: TerminalSnapshot,
}

#[allow(dead_code)]
pub struct CancelledState {
    pub reason: CancellationReason,
    pub snapshot: TerminalSnapshot,
}

#[allow(dead_code)]
pub struct TimedOutState {
    pub reason: TimeoutReason,
    pub snapshot: TerminalSnapshot,
}

#[allow(dead_code)]
pub struct RejectedState {
    pub reason: RejectionReason,
    pub snapshot: TerminalSnapshot,
}

#[allow(dead_code)]
pub struct BackendFailedState {
    pub reason: BackendFailureReason,
    pub snapshot: TerminalSnapshot,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalReason {
    Completed(CompletionReason),
    Cancelled(CancellationReason),
    TimedOut(TimeoutReason),
    Rejected(RejectionReason),
    BackendFailed(BackendFailureReason),
}

#[allow(dead_code)]
impl TerminalReason {
    pub fn terminal_status(self, snapshot: &TerminalSnapshot) -> Option<StatusCode> {
        snapshot.response_status
    }

    pub fn into_terminal_state(self, snapshot: TerminalSnapshot) -> TerminalState {
        match self {
            Self::Completed(reason) => {
                TerminalState::Completed(CompletedState { reason, snapshot })
            }
            Self::Cancelled(reason) => {
                TerminalState::Cancelled(CancelledState { reason, snapshot })
            }
            Self::TimedOut(reason) => TerminalState::TimedOut(TimedOutState { reason, snapshot }),
            Self::Rejected(reason) => TerminalState::Rejected(RejectedState { reason, snapshot }),
            Self::BackendFailed(reason) => {
                TerminalState::BackendFailed(BackendFailedState { reason, snapshot })
            }
        }
    }
}

#[allow(dead_code)]
pub enum TerminalState {
    Completed(CompletedState),
    Cancelled(CancelledState),
    TimedOut(TimedOutState),
    Rejected(RejectedState),
    BackendFailed(BackendFailedState),
}

#[allow(dead_code)]
impl TerminalState {
    pub fn terminal_status(&self) -> Option<StatusCode> {
        match self {
            Self::Completed(state) => state.snapshot.response_status,
            Self::Cancelled(state) => state.snapshot.response_status,
            Self::TimedOut(state) => state.snapshot.response_status,
            Self::Rejected(state) => state.snapshot.response_status,
            Self::BackendFailed(state) => state.snapshot.response_status,
        }
    }

    pub fn should_finalize_backend_accounting(&self) -> bool {
        match self {
            Self::Completed(state) => state
                .snapshot
                .backend_accounting
                .is_some_and(BackendAccountingState::should_finalize),
            Self::Cancelled(state) => state
                .snapshot
                .backend_accounting
                .is_some_and(BackendAccountingState::should_finalize),
            Self::TimedOut(state) => state
                .snapshot
                .backend_accounting
                .is_some_and(BackendAccountingState::should_finalize),
            Self::Rejected(state) => state
                .snapshot
                .backend_accounting
                .is_some_and(BackendAccountingState::should_finalize),
            Self::BackendFailed(state) => state
                .snapshot
                .backend_accounting
                .is_some_and(BackendAccountingState::should_finalize),
        }
    }
}

#[allow(dead_code)]
pub enum RequestExecutionState {
    Intake(RequestIntakeState),
    AwaitingAuth(AwaitingAuthState),
    DispatchReady(DispatchReadyState),
    Admitted(AdmittedState),
    AwaitingUpstream(AwaitingUpstreamState),
    StreamingResponse(ResponseStreamingState),
    Terminal(TerminalState),
}

#[allow(dead_code)]
impl RequestExecutionState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal(_))
    }

    pub fn terminal_status(&self) -> Option<StatusCode> {
        match self {
            Self::Terminal(state) => state.terminal_status(),
            _ => None,
        }
    }

    pub fn should_finalize_backend_accounting(&self) -> bool {
        match self {
            Self::AwaitingUpstream(state) => state.dispatch.backend_accounting.should_finalize(),
            Self::StreamingResponse(state) => state.backend_accounting.should_finalize(),
            Self::Terminal(state) => state.should_finalize_backend_accounting(),
            Self::Intake(_)
            | Self::AwaitingAuth(_)
            | Self::DispatchReady(_)
            | Self::Admitted(_) => false,
        }
    }

    pub fn can_accept_request_body(&self) -> bool {
        match self {
            Self::Intake(state) => state.request_body.can_accept_downstream_body(),
            Self::AwaitingAuth(state) => state.request_body.can_accept_downstream_body(),
            Self::DispatchReady(state) => state.request_body.can_accept_downstream_body(),
            Self::Admitted(state) => state.request_body.can_accept_downstream_body(),
            Self::AwaitingUpstream(state) => state.request_body.can_accept_downstream_body(),
            Self::StreamingResponse(_) | Self::Terminal(_) => false,
        }
    }

    pub fn can_poll_upstream(&self) -> bool {
        match self {
            Self::AwaitingUpstream(state) => {
                state.request_mode.is_tunnel() || state.request_body.forwarding_complete()
            }
            _ => false,
        }
    }

    pub fn can_emit_response(&self) -> bool {
        matches!(self, Self::StreamingResponse(_))
    }

    pub fn phase(&self) -> StreamPhase {
        match self {
            Self::Intake(_)
            | Self::AwaitingAuth(_)
            | Self::DispatchReady(_)
            | Self::Admitted(_) => StreamPhase::ReceivingRequest,
            Self::AwaitingUpstream(_) => StreamPhase::AwaitingUpstream,
            Self::StreamingResponse(_) => StreamPhase::SendingResponse,
            Self::Terminal(TerminalState::Completed(_)) => StreamPhase::Completed,
            Self::Terminal(_) => StreamPhase::Terminal,
        }
    }

    pub fn admission_state(&self) -> StreamAdmissionState {
        match self {
            Self::AwaitingAuth(_) => StreamAdmissionState::WaitingForAuth,
            Self::DispatchReady(_)
            | Self::Admitted(_)
            | Self::AwaitingUpstream(_)
            | Self::StreamingResponse(_) => StreamAdmissionState::ReadyToForward,
            Self::Terminal(TerminalState::Rejected(_)) => StreamAdmissionState::Denied,
            Self::Intake(_) | Self::Terminal(_) => StreamAdmissionState::ReadyToForward,
        }
    }
}
