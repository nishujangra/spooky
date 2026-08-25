use std::{convert::Infallible, sync::OnceLock};

use http_body_util::Full;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use impulse_config::runtime::RuntimeExternalAuth;
use serde_json::Value;
use tokio::task::AbortHandle;

use super::*;
use crate::runtime::connection::{
    auth::{
        ExternalAuthDecision, ExternalAuthDenyResponse, ExternalAuthExecutionPolicy,
        ExternalAuthResponseMetadata, ExternalAuthResult, ExternalAuthStateTransition,
        ExternalAuthTaskConfig, OidcAuthorizationCheck, oidc_audience_matches,
        oidc_authorization_check, oidc_discovery_target, oidc_scope_satisfied,
        resolve_external_auth_state_transition, validate_oidc_provider_metadata,
    },
    outcome::{
        AdmissionOutcomeClass, BackendOutcomeTarget, RouteOutcomeTarget, observe_admission_outcome,
    },
    request::PendingForward,
    stream::{RejectionReason, RequestExecutionState, TerminalReason, TimeoutReason},
};

const MAX_AUTH_BODY_BYTES: usize = 64 * 1024;

pub(super) struct AuthStart {
    pub(super) rx: oneshot::Receiver<ExternalAuthResult>,
    pub(super) abort: AbortHandle,
    pub(super) deadline: Instant,
}

struct OidcExternalAuthInput {
    pending_forward: Arc<PendingForward>,
    discovery_url: Option<String>,
    issuer_url: Option<String>,
    client_id: String,
    client_secret: Option<String>,
    audience: Option<String>,
    scopes: Vec<String>,
    request_headers: Vec<impulse_config::runtime::RuntimeExternalAuthRequestHeader>,
    timeout: Duration,
}

struct AuthHttpClient {
    client: Client<hyper_rustls::HttpsConnector<HttpConnector>, BoxBody<Bytes, Infallible>>,
}

static AUTH_HTTP_CLIENT: OnceLock<AuthHttpClient> = OnceLock::new();

impl AuthHttpClient {
    fn shared() -> &'static Self {
        AUTH_HTTP_CLIENT.get_or_init(|| {
            let https = HttpsConnectorBuilder::new()
                .with_webpki_roots()
                .https_or_http()
                .enable_http1()
                .enable_http2()
                .build();
            let client = Client::builder(hyper_util::rt::TokioExecutor::new())
                .pool_max_idle_per_host(32)
                .pool_idle_timeout(Duration::from_secs(30))
                .build(https);
            Self { client }
        })
    }

    async fn send(
        &self,
        request: Request<BoxBody<Bytes, Infallible>>,
    ) -> Result<Response<Incoming>, ProxyError> {
        self.client
            .request(request)
            .await
            .map_err(|err| ProxyError::Transport(err.to_string()))
    }
}

fn is_unsafe_forwarded_auth_request_header(name: &[u8]) -> bool {
    name.eq_ignore_ascii_case(http::header::HOST.as_str().as_bytes())
        || name.eq_ignore_ascii_case(http::header::CONNECTION.as_str().as_bytes())
        || name.eq_ignore_ascii_case(http::header::CONTENT_LENGTH.as_str().as_bytes())
        || name.eq_ignore_ascii_case(http::header::TRANSFER_ENCODING.as_str().as_bytes())
        || name.eq_ignore_ascii_case(http::header::UPGRADE.as_str().as_bytes())
        || name.eq_ignore_ascii_case(http::header::TE.as_str().as_bytes())
        || name.eq_ignore_ascii_case(http::header::TRAILER.as_str().as_bytes())
        || name.eq_ignore_ascii_case(http::header::EXPECT.as_str().as_bytes())
        || name.eq_ignore_ascii_case(b"keep-alive")
        || name.eq_ignore_ascii_case(b"proxy-connection")
}

pub(super) fn append_auth_request_headers(
    builder: &mut http::request::Builder,
    pending_forward: &PendingForward,
    configured_headers: &[impulse_config::runtime::RuntimeExternalAuthRequestHeader],
) {
    let request_headers = pending_forward.request_headers_read_only();
    for header in request_headers.iter() {
        if header.name().starts_with(b":") || is_unsafe_forwarded_auth_request_header(header.name())
        {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            http::header::HeaderName::from_bytes(header.name()),
            http::header::HeaderValue::from_bytes(header.value()),
        ) && let Some(headers) = builder.headers_mut()
        {
            headers.append(name, value);
        }
    }
    for header in configured_headers {
        builder.headers_mut().into_iter().for_each(|headers| {
            if let (Ok(name), Ok(value)) = (
                http::header::HeaderName::from_bytes(header.name.as_bytes()),
                http::header::HeaderValue::from_str(&header.value),
            ) {
                headers.insert(name, value);
            }
        });
    }
    if let Some(headers) = builder.headers_mut() {
        if let Ok(value) = http::header::HeaderValue::from_str(&pending_forward.method) {
            headers.insert(
                http::header::HeaderName::from_static("x-impulse-original-method"),
                value,
            );
        }
        if let Ok(value) = http::header::HeaderValue::from_str(&pending_forward.path) {
            headers.insert(
                http::header::HeaderName::from_static("x-impulse-original-path"),
                value,
            );
        }
        if let Some(authority) = pending_forward.authority.as_deref()
            && let Ok(value) = http::header::HeaderValue::from_str(authority)
        {
            headers.insert(
                http::header::HeaderName::from_static("x-impulse-original-authority"),
                value,
            );
        }
        if let Ok(value) = http::header::HeaderValue::from_str(&pending_forward.upstream_name) {
            headers.insert(
                http::header::HeaderName::from_static("x-impulse-route-upstream"),
                value,
            );
        }
        if let Ok(value) = http::header::HeaderValue::from_str(&pending_forward.backend_addr) {
            headers.insert(
                http::header::HeaderName::from_static("x-impulse-backend-address"),
                value,
            );
        }
    }
}

async fn collect_auth_body(mut body: Incoming) -> Result<Vec<u8>, ProxyError> {
    use http_body_util::BodyExt as _;

    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|err| ProxyError::Transport(err.to_string()))?;
        let Ok(chunk) = frame.into_data() else {
            continue;
        };
        let next_len = bytes.len().saturating_add(chunk.len());
        if next_len > MAX_AUTH_BODY_BYTES {
            return Err(ProxyError::Transport(format!(
                "external auth body exceeded {MAX_AUTH_BODY_BYTES} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn authorization_header_from_pending_forward(pending_forward: &PendingForward) -> Option<String> {
    let request_headers = pending_forward.request_headers_read_only();
    request_headers
        .iter()
        .find(|header| {
            header
                .name()
                .eq_ignore_ascii_case(http::header::AUTHORIZATION.as_str().as_bytes())
        })
        .and_then(|header| std::str::from_utf8(header.value()).ok().map(str::to_string))
}

fn percent_encode_component(raw: &str) -> String {
    let mut encoded = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
                encoded.push(char::from(b"0123456789ABCDEF"[(byte & 0x0F) as usize]));
            }
        }
    }
    encoded
}

async fn send_auth_request(
    request: Request<BoxBody<Bytes, Infallible>>,
    timeout: Duration,
) -> Result<Response<Incoming>, ProxyError> {
    tokio::time::timeout(timeout, AuthHttpClient::shared().send(request))
        .await
        .map_err(|_| ProxyError::Timeout)?
}

async fn run_external_auth_with_timeout(
    pending_forward: Arc<PendingForward>,
    external_auth: RuntimeExternalAuth,
    timeout: Duration,
) -> ExternalAuthResult {
    tokio::time::timeout(timeout, run_external_auth(pending_forward, external_auth))
        .await
        .map_err(|_| ProxyError::Timeout)?
}

async fn run_http_external_auth(
    pending_forward: Arc<PendingForward>,
    endpoint: String,
    request_headers: Vec<impulse_config::runtime::RuntimeExternalAuthRequestHeader>,
    response_header_allowlist: Vec<String>,
    timeout: Duration,
) -> ExternalAuthResult {
    let mut builder = Request::builder().method(http::Method::GET).uri(endpoint);
    append_auth_request_headers(&mut builder, &pending_forward, &request_headers);
    let request = builder
        .body(BoxBody::new(Full::new(Bytes::new())))
        .map_err(|err| ProxyError::Transport(err.to_string()))?;
    let response = send_auth_request(request, timeout).await?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = if status.is_success() || status.is_redirection() {
        Vec::new()
    } else {
        collect_auth_body(response.into_body()).await?
    };
    crate::runtime::connection::auth::map_http_external_auth_response(
        ExternalAuthResponseMetadata {
            status,
            headers: &headers,
            body: &body,
        },
        &response_header_allowlist,
    )
}

async fn fetch_json_document(uri: String, timeout: Duration) -> Result<Value, ProxyError> {
    let request = Request::builder()
        .method(http::Method::GET)
        .uri(uri)
        .body(BoxBody::new(Full::new(Bytes::new())))
        .map_err(|err| ProxyError::Transport(err.to_string()))?;
    let response = send_auth_request(request, timeout).await?;
    if !response.status().is_success() {
        return Err(ProxyError::Transport(format!(
            "oidc discovery returned {}",
            response.status()
        )));
    }
    let body = collect_auth_body(response.into_body()).await?;
    serde_json::from_slice(&body).map_err(|err| ProxyError::Transport(err.to_string()))
}

async fn run_oidc_external_auth(input: OidcExternalAuthInput) -> ExternalAuthResult {
    let OidcExternalAuthInput {
        pending_forward,
        discovery_url,
        issuer_url,
        client_id,
        client_secret,
        audience,
        scopes,
        request_headers,
        timeout,
    } = input;
    let token = match oidc_authorization_check(
        authorization_header_from_pending_forward(&pending_forward).as_deref(),
    ) {
        OidcAuthorizationCheck::Token(token) => token,
        OidcAuthorizationCheck::Challenge(response) => {
            return Ok(ExternalAuthDecision::Challenge(response));
        }
    };
    let discovery = oidc_discovery_target(discovery_url.as_deref(), issuer_url.as_deref())?;
    let document = fetch_json_document(discovery.url, timeout).await?;
    let metadata = validate_oidc_provider_metadata(&document)?;

    let mut body = format!(
        "token={}&client_id={}",
        percent_encode_component(&token),
        percent_encode_component(&client_id)
    );
    if let Some(secret) = client_secret.as_deref() {
        body.push_str("&client_secret=");
        body.push_str(&percent_encode_component(secret));
    }
    if let Some(audience) = audience.as_deref() {
        body.push_str("&audience=");
        body.push_str(&percent_encode_component(audience));
    }

    let mut builder = Request::builder()
        .method(http::Method::POST)
        .uri(metadata.introspection_endpoint)
        .header(
            http::header::CONTENT_TYPE,
            http::header::HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
    append_auth_request_headers(&mut builder, &pending_forward, &request_headers);
    let request = builder
        .body(BoxBody::new(Full::new(Bytes::from(body))))
        .map_err(|err| ProxyError::Transport(err.to_string()))?;
    let response = send_auth_request(request, timeout).await?;
    if !response.status().is_success() {
        if response.status().is_client_error() {
            return Ok(ExternalAuthDecision::Deny(ExternalAuthDenyResponse {
                status: http::StatusCode::FORBIDDEN,
                headers: Vec::new(),
                body: b"oidc token rejected\n".to_vec(),
            }));
        }
        return Err(ProxyError::Transport(format!(
            "oidc introspection returned {}",
            response.status()
        )));
    }
    let payload = collect_auth_body(response.into_body()).await?;
    let value: Value =
        serde_json::from_slice(&payload).map_err(|err| ProxyError::Transport(err.to_string()))?;
    if !value
        .get("active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(ExternalAuthDecision::Deny(ExternalAuthDenyResponse {
            status: http::StatusCode::FORBIDDEN,
            headers: Vec::new(),
            body: b"inactive oidc token\n".to_vec(),
        }));
    }
    if let Some(issuer_url) = issuer_url.as_deref()
        && value.get("iss").and_then(Value::as_str) != Some(issuer_url)
    {
        return Ok(ExternalAuthDecision::Deny(ExternalAuthDenyResponse {
            status: http::StatusCode::FORBIDDEN,
            headers: Vec::new(),
            body: b"unexpected oidc issuer\n".to_vec(),
        }));
    }
    if !oidc_audience_matches(audience.as_deref(), value.get("aud")) {
        return Ok(ExternalAuthDecision::Deny(ExternalAuthDenyResponse {
            status: http::StatusCode::FORBIDDEN,
            headers: Vec::new(),
            body: b"unexpected oidc audience\n".to_vec(),
        }));
    }
    if !scopes.is_empty() {
        let Some(scope_value) = value.get("scope").and_then(Value::as_str) else {
            return Ok(ExternalAuthDecision::Deny(ExternalAuthDenyResponse {
                status: http::StatusCode::FORBIDDEN,
                headers: Vec::new(),
                body: b"missing oidc scopes\n".to_vec(),
            }));
        };
        if !oidc_scope_satisfied(&scopes, scope_value) {
            return Ok(ExternalAuthDecision::Deny(ExternalAuthDenyResponse {
                status: http::StatusCode::FORBIDDEN,
                headers: Vec::new(),
                body: b"missing oidc scopes\n".to_vec(),
            }));
        }
    }

    Ok(ExternalAuthDecision::Allow {
        request_header_mutations: Vec::new(),
    })
}

async fn run_external_auth(
    pending_forward: Arc<PendingForward>,
    external_auth: RuntimeExternalAuth,
) -> ExternalAuthResult {
    let timeout = ExternalAuthExecutionPolicy::from_external_auth(&external_auth).timeout;
    match external_auth {
        RuntimeExternalAuth::Http {
            endpoint,
            request_headers,
            response_header_allowlist,
            ..
        } => {
            run_http_external_auth(
                pending_forward,
                endpoint,
                request_headers,
                response_header_allowlist,
                timeout,
            )
            .await
        }
        RuntimeExternalAuth::Oidc {
            discovery_url,
            issuer_url,
            client_id,
            client_secret,
            audience,
            scopes,
            request_headers,
            ..
        } => {
            run_oidc_external_auth(OidcExternalAuthInput {
                pending_forward,
                discovery_url,
                issuer_url,
                client_id,
                client_secret,
                audience,
                scopes,
                request_headers,
                timeout,
            })
            .await
        }
    }
}

pub(super) fn start_external_auth_task(
    pending_forward: Arc<PendingForward>,
    external_auth: RuntimeExternalAuth,
) -> Result<AuthStart, ProxyError> {
    let task_config = ExternalAuthTaskConfig::from_external_auth(&external_auth);
    let (tx, rx) = oneshot::channel();
    let fut = async move {
        let result =
            run_external_auth_with_timeout(pending_forward, external_auth, task_config.timeout)
                .await;
        let _ = tx.send(result);
    };
    let Some(handle) = runtime_handle() else {
        return Err(ProxyError::Transport(
            "dropping external auth task: no runtime available".into(),
        ));
    };
    let join = handle.spawn(fut);
    Ok(AuthStart {
        rx,
        abort: join.abort_handle(),
        deadline: Instant::now() + task_config.timeout,
    })
}

pub(in crate::quic_listener) async fn evaluate_pending_forward_external_auth(
    pending_forward: Arc<PendingForward>,
    external_auth: RuntimeExternalAuth,
) -> ExternalAuthStateTransition {
    let task_config = ExternalAuthTaskConfig::from_external_auth(&external_auth);
    let result =
        run_external_auth_with_timeout(pending_forward, external_auth, task_config.timeout).await;
    resolve_external_auth_state_transition(result, task_config.disposition)
}

impl QUICListener {
    pub(super) fn complete_auth_result(
        stream_id: u64,
        req: &mut RequestEnvelope,
        result: ExternalAuthResult,
        h3: &mut quiche::h3::Connection,
        quic: &mut quiche::Connection,
        exec_ctx: &ForwardingExecutionCtx<'_>,
        shared_ctx: &ForwardingSharedCtx<'_>,
    ) -> Result<bool, quiche::h3::Error> {
        let metrics = shared_ctx.metrics.as_ref();
        let RequestExecutionState::AwaitingAuth(awaiting_auth) = &req.execution else {
            Self::send_simple_response(
                h3,
                quic,
                stream_id,
                http::StatusCode::INTERNAL_SERVER_ERROR,
                b"missing awaiting auth state\n",
            )?;
            return Ok(false);
        };
        let transition =
            resolve_external_auth_state_transition(result, awaiting_auth.auth_disposition);
        match transition {
            ExternalAuthStateTransition::Admitted {
                request_header_mutations,
            } => {
                metrics.inc_external_auth_allowed();
                req.transition_awaiting_auth_to_dispatch_ready(request_header_mutations);
                Self::materialize_forward_after_auth(stream_id, req, h3, quic, exec_ctx, shared_ctx)
            }
            ExternalAuthStateTransition::RejectedAuthDenied { decision } => {
                req.response_status = Some(decision.status().as_u16());
                metrics.inc_policy_denied();
                metrics.inc_external_auth_denied();
                let _ = observe_admission_outcome(
                    metrics,
                    RouteOutcomeTarget {
                        route: req.upstream_name.as_deref().unwrap_or("unrouted"),
                    },
                    Some(BackendOutcomeTarget {
                        upstream: req.upstream_name.as_deref().unwrap_or("unrouted"),
                        backend_addr: req.backend_addr.as_deref(),
                        backend_index: req.backend_index,
                    }),
                    req.start.elapsed(),
                    decision.status(),
                    AdmissionOutcomeClass::AuthDenied,
                );
                warn!(
                    "admission denied: request_id={} upstream={} reason=auth_denied status={}",
                    req.request_id,
                    req.upstream_name.as_deref().unwrap_or("unrouted"),
                    req.response_status.unwrap_or(0)
                );
                Self::send_external_auth_decision_response(h3, quic, stream_id, &decision)?;
                req.mark_terminal_outcome_recorded();
                req.transition_to_terminal_with_cleanup(
                    TerminalReason::Rejected(RejectionReason::AuthDenied),
                    metrics,
                );
                Ok(false)
            }
            ExternalAuthStateTransition::RejectedAuthUnavailable {
                status,
                body,
                error,
            } => {
                metrics.inc_external_auth_error();
                if let Some(error) = &error {
                    debug!(
                        "admission denied: request_id={} upstream={} reason=auth_unavailable detail={:?}",
                        req.request_id,
                        req.upstream_name.as_deref().unwrap_or("unrouted"),
                        error
                    );
                }
                req.response_status = Some(status.as_u16());
                let _ = observe_admission_outcome(
                    metrics,
                    RouteOutcomeTarget {
                        route: req.upstream_name.as_deref().unwrap_or("unrouted"),
                    },
                    Some(BackendOutcomeTarget {
                        upstream: req.upstream_name.as_deref().unwrap_or("unrouted"),
                        backend_addr: req.backend_addr.as_deref(),
                        backend_index: req.backend_index,
                    }),
                    req.start.elapsed(),
                    status,
                    AdmissionOutcomeClass::Failed { timed_out: false },
                );
                Self::send_simple_response(h3, quic, stream_id, status, body)?;
                req.mark_terminal_outcome_recorded();
                req.transition_to_terminal_with_cleanup(
                    TerminalReason::Rejected(RejectionReason::AuthUnavailable),
                    metrics,
                );
                Ok(false)
            }
            ExternalAuthStateTransition::TimedOutAuth { status, body } => {
                metrics.inc_external_auth_timeout();
                req.response_status = Some(status.as_u16());
                let _ = observe_admission_outcome(
                    metrics,
                    RouteOutcomeTarget {
                        route: req.upstream_name.as_deref().unwrap_or("unrouted"),
                    },
                    Some(BackendOutcomeTarget {
                        upstream: req.upstream_name.as_deref().unwrap_or("unrouted"),
                        backend_addr: req.backend_addr.as_deref(),
                        backend_index: req.backend_index,
                    }),
                    req.start.elapsed(),
                    status,
                    AdmissionOutcomeClass::Failed { timed_out: true },
                );
                Self::send_simple_response(h3, quic, stream_id, status, body)?;
                req.mark_terminal_outcome_recorded();
                req.transition_to_terminal_with_cleanup(
                    TerminalReason::TimedOut(TimeoutReason::ExternalAuth),
                    metrics,
                );
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use impulse_config::runtime::{
        RuntimeApiKeyAuth, RuntimeAuthPolicy, RuntimeExternalAuthFailureMode,
        RuntimeForwardedHeaderPolicy, RuntimeHostPolicy, RuntimeProtocolPolicy,
        RuntimeUpstreamPolicy,
    };

    use super::*;
    use crate::{
        quic_listener::admission::{
            AdmissionPolicyDecision, UnauthorizedDecision, admission_rejection_response,
            evaluate_forwarding_pre_admission_policy,
        },
        resilience::{brownout::BrownoutController, scoped_rate_limit::ScopedRateLimiters},
    };

    fn decision_contract(
        decision: &crate::runtime::connection::auth::ExternalAuthDecision,
    ) -> (http::StatusCode, Vec<(String, String)>, Vec<u8>) {
        match decision {
            crate::runtime::connection::auth::ExternalAuthDecision::Allow {
                request_header_mutations: _,
            } => (http::StatusCode::OK, Vec::new(), Vec::new()),
            crate::runtime::connection::auth::ExternalAuthDecision::Deny(response) => (
                response.status,
                response.headers.clone(),
                response.body.clone(),
            ),
            crate::runtime::connection::auth::ExternalAuthDecision::Redirect(response) => {
                let mut headers = response.headers.clone();
                headers.push((
                    http::header::LOCATION.as_str().to_string(),
                    response.location.clone(),
                ));
                (response.status, headers, Vec::new())
            }
            crate::runtime::connection::auth::ExternalAuthDecision::Challenge(response) => {
                let mut headers = response.headers.clone();
                headers.push((
                    http::header::WWW_AUTHENTICATE.as_str().to_string(),
                    response.www_authenticate.clone(),
                ));
                (response.status, headers, response.body.clone())
            }
        }
    }

    #[test]
    fn append_auth_request_headers_strips_unsafe_headers_and_overrides_with_configured_values() {
        let pending_forward = PendingForward::sample_for_test(vec![
            quiche::h3::Header::new(b":method", b"GET"),
            quiche::h3::Header::new(b"host", b"client.example.com"),
            quiche::h3::Header::new(b"connection", b"keep-alive"),
            quiche::h3::Header::new(b"content-length", b"42"),
            quiche::h3::Header::new(b"x-forwarded-for", b"1.2.3.4"),
            quiche::h3::Header::new(b"x-auth-user", b"alice"),
            quiche::h3::Header::new(b"x-role", b"stale"),
        ]);
        let mut builder = http::Request::builder()
            .method(http::Method::GET)
            .uri("https://auth.internal/check");

        append_auth_request_headers(
            &mut builder,
            &pending_forward,
            &[impulse_config::runtime::RuntimeExternalAuthRequestHeader {
                name: "x-role".to_string(),
                value: "admin".to_string(),
            }],
        );

        let headers = builder.headers_ref().expect("headers");
        assert!(!headers.contains_key(http::header::HOST));
        assert!(!headers.contains_key(http::header::CONNECTION));
        assert!(!headers.contains_key(http::header::CONTENT_LENGTH));
        assert_eq!(
            headers
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok()),
            Some("1.2.3.4")
        );
        assert_eq!(
            headers
                .get("x-auth-user")
                .and_then(|value| value.to_str().ok()),
            Some("alice")
        );
        assert_eq!(
            headers.get("x-role").and_then(|value| value.to_str().ok()),
            Some("admin")
        );
        assert_eq!(
            headers
                .get("x-impulse-original-method")
                .and_then(|value| value.to_str().ok()),
            Some("GET")
        );
        assert_eq!(
            headers
                .get("x-impulse-original-path")
                .and_then(|value| value.to_str().ok()),
            Some("/v1/chat")
        );
        assert_eq!(
            headers
                .get("x-impulse-original-authority")
                .and_then(|value| value.to_str().ok()),
            Some("api.example.com")
        );
    }

    #[test]
    fn percent_encode_component_preserves_safe_bytes_and_encodes_reserved_bytes() {
        assert_eq!(percent_encode_component("AZaz09-_.~"), "AZaz09-_.~");
        assert_eq!(
            percent_encode_component("token value+/=?&%"),
            "token%20value%2B%2F%3D%3F%26%25"
        );
        assert_eq!(percent_encode_component("\n"), "%0A");
    }

    #[test]
    fn authorization_header_lookup_observes_pending_auth_mutations() {
        let pending_forward = PendingForward {
            auth_header_mutations: vec![
                crate::runtime::connection::auth::PendingHeaderMutation::Upsert {
                    name: http::header::AUTHORIZATION.as_str().as_bytes().to_vec(),
                    value: b"Bearer refreshed-token".to_vec(),
                },
            ],
            ..PendingForward::sample_for_test(vec![quiche::h3::Header::new(
                b"authorization",
                b"Bearer stale-token",
            )])
        };

        assert_eq!(
            authorization_header_from_pending_forward(&pending_forward).as_deref(),
            Some("Bearer refreshed-token")
        );
    }

    #[test]
    fn local_auth_precedence_is_explicit_before_external_auth_candidates_are_relevant() {
        let policy = RuntimeUpstreamPolicy {
            upstream_auth: RuntimeAuthPolicy {
                api_key: Some(RuntimeApiKeyAuth {
                    header_name: "x-api-key".to_string(),
                    keys: vec!["secret".to_string()],
                }),
                jwt: None,
                external_auth: Some(RuntimeExternalAuth::Http {
                    endpoint: "http://127.0.0.1:9000/auth".to_string(),
                    request_headers: Vec::new(),
                    response_header_allowlist: Vec::new(),
                    timeout: Duration::from_millis(250),
                    failure_mode: RuntimeExternalAuthFailureMode::FailClosed,
                }),
                required_scopes: Vec::new(),
                required_roles: Vec::new(),
            },
            host: RuntimeHostPolicy::default(),
            forwarded_headers: RuntimeForwardedHeaderPolicy::default(),
            protocol: RuntimeProtocolPolicy::default(),
        };
        let brownout = BrownoutController::new(false, 100, 90, Vec::new());
        let scoped_rate_limits = ScopedRateLimiters::new(&[]);

        let decision = evaluate_forwarding_pre_admission_policy(
            &policy,
            None,
            &brownout,
            0,
            "api",
            "GET",
            "/resource",
            Some("api.example.com"),
            "198.51.100.10:443".parse().expect("client addr"),
            1,
            &scoped_rate_limits,
        );

        assert!(matches!(
            decision,
            AdmissionPolicyDecision::Unauthorized(UnauthorizedDecision {
                status: http::StatusCode::UNAUTHORIZED,
                body: b"unauthorized\n",
                ..
            })
        ));
    }

    #[test]
    fn equivalent_quic_external_auth_challenge_and_bootstrap_auth_denial_share_contract() {
        let quic = crate::runtime::connection::auth::ExternalAuthDecision::Challenge(
            crate::runtime::connection::auth::ExternalAuthChallengeResponse {
                status: http::StatusCode::UNAUTHORIZED,
                headers: Vec::new(),
                www_authenticate: "Bearer".to_string(),
                body: b"unauthorized\n".to_vec(),
            },
        );
        let bootstrap = admission_rejection_response(&AdmissionPolicyDecision::Unauthorized(
            UnauthorizedDecision {
                challenge: crate::quic_listener::admission::AuthChallengeKind::Bearer,
                status: http::StatusCode::UNAUTHORIZED,
                body: b"unauthorized\n",
            },
        ))
        .expect("bootstrap auth rejection");

        let (quic_status, quic_headers, quic_body) = decision_contract(&quic);
        let bootstrap_headers = vec![(
            http::header::WWW_AUTHENTICATE.as_str().to_string(),
            bootstrap
                .www_authenticate
                .expect("bootstrap challenge")
                .to_string(),
        )];

        assert_eq!(quic_status, bootstrap.status);
        assert_eq!(quic_body, bootstrap.body.to_vec());
        assert_eq!(quic_headers, bootstrap_headers);
    }
}
