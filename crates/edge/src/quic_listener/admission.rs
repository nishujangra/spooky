#[cfg(test)]
use std::{collections::VecDeque, sync::Mutex};
use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    sync::{Arc, OnceLock, RwLock, Weak},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use boring::{
    bn::BigNum,
    ec::{EcGroup, EcKey},
    ecdsa::EcdsaSig,
    hash::MessageDigest,
    nid::Nid,
    pkey::{Id as PKeyId, PKey, Public},
    rsa::{Padding, Rsa},
    sign::Verifier,
};
use bytes::Bytes;
use hmac::{Hmac, Mac};
use http::StatusCode;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::{
    Request, Response,
    body::{Body, Incoming},
};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use serde_json::Value;
use sha2::{Digest, Sha256};
use spooky_config::{
    config::{JwksStartupBehavior, JwtAlgorithm},
    runtime::{RuntimeConfig, RuntimeJwtAuth, RuntimeJwtVerificationKey, RuntimeUpstreamPolicy},
};
use spooky_lb::upstream_pool::UpstreamPool;
use subtle::ConstantTimeEq;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

use super::{LbHeaderLookup, QUICListener, runtime_handle, spawn_supervised_async_task};
use crate::{
    RouteOutcome,
    metrics::OverloadShedReason,
    resilience::{
        adaptive_admission::AdaptivePermit,
        brownout::BrownoutController,
        route_queue::{RouteQueuePermit, RouteQueueRejection},
        runtime::RuntimeResilience,
        scoped_rate_limit::{ScopedRateLimitRule, ScopedRateLimiters},
    },
    runtime::tasks::RuntimeTaskRegistry,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthChallengeKind {
    ApiKey,
    Bearer,
}

impl AuthChallengeKind {
    pub(super) fn as_www_authenticate(self) -> &'static str {
        match self {
            Self::ApiKey => "ApiKey",
            Self::Bearer => "Bearer",
        }
    }
}

impl OverloadDecisionReason {
    pub(super) fn metrics_reason(self) -> OverloadShedReason {
        match self {
            Self::Brownout => OverloadShedReason::Brownout,
            Self::AdaptiveAdmission => OverloadShedReason::AdaptiveAdmission,
            Self::RouteCap => OverloadShedReason::RouteCap,
            Self::RouteGlobalCap => OverloadShedReason::RouteGlobalCap,
            Self::GlobalInflight => OverloadShedReason::GlobalInflight,
            Self::UpstreamInflight => OverloadShedReason::UpstreamInflight,
        }
    }

    fn response_body(self) -> &'static [u8] {
        match self {
            Self::Brownout => b"brownout active, non-core route shed\n",
            Self::AdaptiveAdmission => b"adaptive admission overload\n",
            Self::RouteCap => b"route queue cap exceeded\n",
            Self::RouteGlobalCap => b"global queue cap exceeded\n",
            Self::GlobalInflight => b"overloaded, retry later\n",
            Self::UpstreamInflight => b"upstream overloaded, retry later\n",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UnauthorizedDecision {
    pub(super) challenge: AuthChallengeKind,
    pub(super) status: StatusCode,
    pub(super) body: &'static [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RateLimitedDecision {
    pub(super) rule_name: String,
    pub(super) route: String,
    pub(super) status: StatusCode,
    pub(super) body: &'static [u8],
    pub(super) retry_after_seconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverloadDecisionReason {
    Brownout,
    AdaptiveAdmission,
    RouteCap,
    RouteGlobalCap,
    GlobalInflight,
    UpstreamInflight,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OverloadDecision {
    pub(super) reason: OverloadDecisionReason,
    pub(super) status: StatusCode,
    pub(super) body: &'static [u8],
    pub(super) retry_after_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AdmissionRejectionResponse {
    pub(super) status: StatusCode,
    pub(super) body: &'static [u8],
    pub(super) www_authenticate: Option<&'static str>,
    pub(super) retry_after_seconds: Option<u32>,
}

#[derive(Debug, Clone)]
pub(super) struct PostAuthAdmissionFailure {
    pub(super) status: StatusCode,
    pub(super) body: &'static [u8],
    pub(super) overload_reason: Option<OverloadDecisionReason>,
    pub(super) route_outcome: Option<RouteOutcome>,
    pub(super) observe_adaptive_overload: bool,
}

pub(super) struct PostAuthAdmissionReady {
    pub(super) backend_index: usize,
    pub(super) upstream_pool: Arc<RwLock<UpstreamPool>>,
    pub(super) global_permit: OwnedSemaphorePermit,
    pub(super) upstream_permit: OwnedSemaphorePermit,
    pub(super) adaptive_permit: AdaptivePermit,
    pub(super) route_queue_permit: RouteQueuePermit,
    pub(super) waited_for_global_permit: bool,
    pub(super) waited_for_upstream_permit: bool,
}

#[derive(Debug, Clone)]
pub(super) enum PostAuthAdmissionRejection {
    Overloaded(OverloadDecision),
    Failed(PostAuthAdmissionFailure),
}

pub(super) enum PostAuthAdmissionExecution {
    Ready(PostAuthAdmissionReady),
    Rejected(PostAuthAdmissionRejection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AdmissionPolicyDecision {
    AdmitReady,
    Unauthorized(UnauthorizedDecision),
    RateLimited(RateLimitedDecision),
    Overloaded(OverloadDecision),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn evaluate_forwarding_pre_admission_policy<F>(
    policy: &RuntimeUpstreamPolicy,
    header_lookup: Option<&LbHeaderLookup<'_>>,
    brownout: &BrownoutController,
    inflight_percent: u8,
    route: &str,
    retry_after_seconds: u32,
    scoped_rate_limits: &ScopedRateLimiters,
    key_for_rule: F,
) -> AdmissionPolicyDecision
where
    F: FnMut(&ScopedRateLimitRule) -> Option<String>,
{
    let auth = evaluate_local_auth_policy(policy, header_lookup);
    if auth != AdmissionPolicyDecision::AdmitReady {
        return auth;
    }

    let brownout = evaluate_brownout_policy(brownout, inflight_percent, route, retry_after_seconds);
    if brownout != AdmissionPolicyDecision::AdmitReady {
        return brownout;
    }

    evaluate_scoped_rate_limit_policy(scoped_rate_limits, route, key_for_rule)
}

pub(super) fn evaluate_local_auth_policy(
    policy: &RuntimeUpstreamPolicy,
    header_lookup: Option<&LbHeaderLookup<'_>>,
) -> AdmissionPolicyDecision {
    if !api_key_is_authorized(policy, header_lookup) {
        return AdmissionPolicyDecision::Unauthorized(UnauthorizedDecision {
            challenge: AuthChallengeKind::ApiKey,
            status: StatusCode::UNAUTHORIZED,
            body: b"unauthorized\n",
        });
    }

    if !jwt_is_authorized(policy, header_lookup) {
        return AdmissionPolicyDecision::Unauthorized(UnauthorizedDecision {
            challenge: AuthChallengeKind::Bearer,
            status: StatusCode::UNAUTHORIZED,
            body: b"unauthorized\n",
        });
    }

    AdmissionPolicyDecision::AdmitReady
}

pub(super) fn evaluate_scoped_rate_limit_policy<F>(
    scoped_rate_limits: &ScopedRateLimiters,
    route: &str,
    key_for_rule: F,
) -> AdmissionPolicyDecision
where
    F: FnMut(&ScopedRateLimitRule) -> Option<String>,
{
    let Some(rejection) = scoped_rate_limits.check(route, key_for_rule) else {
        return AdmissionPolicyDecision::AdmitReady;
    };

    AdmissionPolicyDecision::RateLimited(RateLimitedDecision {
        rule_name: rejection.rule_name,
        route: rejection.route,
        status: StatusCode::TOO_MANY_REQUESTS,
        body: b"request rate limited\n",
        retry_after_seconds: rejection.retry_after_seconds,
    })
}

pub(super) fn evaluate_brownout_policy(
    brownout: &BrownoutController,
    inflight_percent: u8,
    route: &str,
    retry_after_seconds: u32,
) -> AdmissionPolicyDecision {
    brownout.observe_admission_pressure(inflight_percent);
    if brownout.route_allowed(route) {
        return AdmissionPolicyDecision::AdmitReady;
    }

    overload_decision(OverloadDecisionReason::Brownout, retry_after_seconds)
}

fn overload_decision(
    reason: OverloadDecisionReason,
    retry_after_seconds: u32,
) -> AdmissionPolicyDecision {
    AdmissionPolicyDecision::Overloaded(OverloadDecision {
        reason,
        status: StatusCode::SERVICE_UNAVAILABLE,
        body: reason.response_body(),
        retry_after_seconds: retry_after_seconds.max(1),
    })
}

fn overload_decision_for_route_queue_rejection(
    rejection: RouteQueueRejection,
    retry_after_seconds: u32,
) -> AdmissionPolicyDecision {
    let reason = match rejection {
        RouteQueueRejection::GlobalCap => OverloadDecisionReason::RouteGlobalCap,
        RouteQueueRejection::RouteCap => OverloadDecisionReason::RouteCap,
    };
    overload_decision(reason, retry_after_seconds)
}

pub(super) fn admission_rejection_response(
    decision: &AdmissionPolicyDecision,
) -> Option<AdmissionRejectionResponse> {
    match decision {
        AdmissionPolicyDecision::AdmitReady => None,
        AdmissionPolicyDecision::Unauthorized(decision) => Some(AdmissionRejectionResponse {
            status: decision.status,
            body: decision.body,
            www_authenticate: Some(decision.challenge.as_www_authenticate()),
            retry_after_seconds: None,
        }),
        AdmissionPolicyDecision::RateLimited(decision) => Some(AdmissionRejectionResponse {
            status: decision.status,
            body: decision.body,
            www_authenticate: None,
            retry_after_seconds: Some(decision.retry_after_seconds.max(1)),
        }),
        AdmissionPolicyDecision::Overloaded(decision) => Some(AdmissionRejectionResponse {
            status: decision.status,
            body: decision.body,
            www_authenticate: None,
            retry_after_seconds: Some(decision.retry_after_seconds.max(1)),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_forwarding_post_auth_admission(
    resilience: &RuntimeResilience,
    upstream_name: &str,
    upstream_pool: Option<&Arc<RwLock<UpstreamPool>>>,
    backend_index: Option<usize>,
    pending_forward_backend_index: usize,
    upstream_inflight: &HashMap<String, Arc<Semaphore>>,
    global_inflight: Arc<Semaphore>,
    inflight_acquire_wait: Duration,
) -> PostAuthAdmissionExecution {
    let adaptive_permit = match resilience.adaptive_admission.try_acquire() {
        Some(permit) => permit,
        None => {
            return PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Overloaded(
                overloaded(
                    OverloadDecisionReason::AdaptiveAdmission,
                    resilience.shed_retry_after_seconds,
                ),
            ));
        }
    };

    let route_queue_permit = match resilience.route_queue.try_acquire(upstream_name) {
        Ok(permit) => permit,
        Err(rejection) => {
            return PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Overloaded(
                overload_from_route_queue_rejection(rejection, resilience.shed_retry_after_seconds),
            ));
        }
    };

    let (global_permit, waited_for_global_permit) =
        match try_acquire_owned_with_micro_wait(global_inflight, inflight_acquire_wait) {
            Ok(value) => value,
            Err(_) => {
                return PostAuthAdmissionExecution::Rejected(
                    PostAuthAdmissionRejection::Overloaded(overloaded(
                        OverloadDecisionReason::GlobalInflight,
                        resilience.shed_retry_after_seconds,
                    )),
                );
            }
        };

    let (upstream_permit, waited_for_upstream_permit) =
        match upstream_inflight.get(upstream_name).cloned() {
            Some(semaphore) => {
                match try_acquire_owned_with_micro_wait(semaphore, inflight_acquire_wait) {
                    Ok(value) => value,
                    Err(_) => {
                        return PostAuthAdmissionExecution::Rejected(
                            PostAuthAdmissionRejection::Overloaded(overloaded(
                                OverloadDecisionReason::UpstreamInflight,
                                resilience.shed_retry_after_seconds,
                            )),
                        );
                    }
                }
            }
            None => {
                return PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Failed(
                    PostAuthAdmissionFailure {
                        status: StatusCode::SERVICE_UNAVAILABLE,
                        body: b"upstream admission limiter unavailable\n",
                        overload_reason: Some(OverloadDecisionReason::UpstreamInflight),
                        route_outcome: Some(RouteOutcome::OverloadShed),
                        observe_adaptive_overload: true,
                    },
                ));
            }
        };

    let backend_index = backend_index.unwrap_or(pending_forward_backend_index);
    let Some(upstream_pool) = upstream_pool.cloned() else {
        return PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Failed(
            PostAuthAdmissionFailure {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                body: b"missing upstream pool\n",
                overload_reason: None,
                route_outcome: None,
                observe_adaptive_overload: false,
            },
        ));
    };

    let backend_healthy = upstream_pool
        .read()
        .ok()
        .is_some_and(|pool| pool.is_backend_healthy(backend_index));
    if !backend_healthy {
        return PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Failed(
            PostAuthAdmissionFailure {
                status: StatusCode::SERVICE_UNAVAILABLE,
                body: b"selected backend no longer healthy\n",
                overload_reason: None,
                route_outcome: Some(RouteOutcome::Failure),
                observe_adaptive_overload: false,
            },
        ));
    }

    PostAuthAdmissionExecution::Ready(PostAuthAdmissionReady {
        backend_index,
        upstream_pool,
        global_permit,
        upstream_permit,
        adaptive_permit,
        route_queue_permit,
        waited_for_global_permit,
        waited_for_upstream_permit,
    })
}

pub(super) fn try_acquire_owned_with_micro_wait(
    semaphore: Arc<Semaphore>,
    _wait_budget: Duration,
) -> Result<(OwnedSemaphorePermit, bool), TryAcquireError> {
    // Never block the synchronous QUIC worker thread: acquire immediately or
    // shed. A blocking wait here stalls every connection on the shard.
    semaphore.try_acquire_owned().map(|permit| (permit, false))
}

pub(super) fn api_key_is_authorized(
    policy: &RuntimeUpstreamPolicy,
    header_lookup: Option<&LbHeaderLookup<'_>>,
) -> bool {
    let Some(api_key) = policy.upstream_auth.api_key.as_ref() else {
        return true;
    };
    let Some(provided) = header_lookup.and_then(|lookup| lookup(api_key.header_name.as_str()))
    else {
        return false;
    };
    let provided = provided.trim();
    !provided.is_empty()
        && api_key
            .keys
            .iter()
            .any(|expected| bool::from(provided.as_bytes().ct_eq(expected.as_bytes())))
}

pub(super) fn jwt_is_authorized(
    policy: &RuntimeUpstreamPolicy,
    header_lookup: Option<&LbHeaderLookup<'_>>,
) -> bool {
    let Some(jwt) = policy.upstream_auth.jwt.as_ref() else {
        return true;
    };
    let Some(raw) = header_lookup.and_then(|lookup| lookup(http::header::AUTHORIZATION.as_str()))
    else {
        return false;
    };
    let Some(token) = QUICListener::bearer_token_from_authorization_value(&raw) else {
        return false;
    };
    let claims = match validate_jwt_token(token.as_str(), jwt, SystemTime::now()) {
        Ok(validated) => validated.claims,
        Err(failure) => {
            observe_jwt_validation_failure(jwt, token.as_str(), &failure);
            log_jwt_validation_rejection(jwt, token.as_str(), &failure);
            return false;
        }
    };
    jwt_claims_satisfy_rbac(policy, &claims)
}

#[cfg(test)]
pub(super) fn validated_hs256_jwt_claims(
    token: &str,
    jwt: &RuntimeJwtAuth,
    now: SystemTime,
) -> Option<Value> {
    let validated = validate_jwt_token(token, jwt, now).ok()?;
    matches!(validated.algorithm, JwtAlgorithm::Hs256).then_some(validated.claims)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JwtValidationFailureReason {
    MalformedToken,
    MalformedHeader,
    MalformedClaims,
    MissingAlgorithm,
    AlgorithmNotAllowed,
    UnsupportedAlgorithm,
    MissingKid,
    MissingVerificationKey,
    AmbiguousVerificationKey,
    KeySourceUnavailable,
    PemKeyParseFailed,
    JwkKeyParseFailed,
    InvalidKeyType,
    UnsupportedCurve,
    KeyTooWeak,
    SignatureInvalid,
    MissingExpiration,
    TokenExpired,
    TokenNotYetValid,
    TokenIssuedInFuture,
    IssuerMismatch,
    AudienceMismatch,
}

impl JwtValidationFailureReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MalformedToken => "malformed_token",
            Self::MalformedHeader => "malformed_header",
            Self::MalformedClaims => "malformed_claims",
            Self::MissingAlgorithm => "missing_algorithm",
            Self::AlgorithmNotAllowed => "algorithm_not_allowed",
            Self::UnsupportedAlgorithm => "unsupported_algorithm",
            Self::MissingKid => "missing_kid",
            Self::MissingVerificationKey => "missing_verification_key",
            Self::AmbiguousVerificationKey => "ambiguous_verification_key",
            Self::KeySourceUnavailable => "key_source_unavailable",
            Self::PemKeyParseFailed => "pem_key_parse_failed",
            Self::JwkKeyParseFailed => "jwk_key_parse_failed",
            Self::InvalidKeyType => "invalid_key_type",
            Self::UnsupportedCurve => "unsupported_curve",
            Self::KeyTooWeak => "key_too_weak",
            Self::SignatureInvalid => "signature_invalid",
            Self::MissingExpiration => "missing_expiration",
            Self::TokenExpired => "token_expired",
            Self::TokenNotYetValid => "token_not_yet_valid",
            Self::TokenIssuedInFuture => "token_issued_in_future",
            Self::IssuerMismatch => "issuer_mismatch",
            Self::AudienceMismatch => "audience_mismatch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct JwtValidationFailure {
    pub(super) reason: JwtValidationFailureReason,
}

impl JwtValidationFailure {
    fn new(reason: JwtValidationFailureReason) -> Self {
        Self { reason }
    }
}

fn log_jwt_validation_rejection(jwt: &RuntimeJwtAuth, token: &str, failure: &JwtValidationFailure) {
    let header = parse_compact_jwt(token)
        .ok()
        .and_then(|parsed| parse_jose_header(&parsed.header_bytes).ok());
    let algorithm = header
        .as_ref()
        .map(|header| jwt_algorithm_name(header.algorithm))
        .unwrap_or("unknown");
    let kid = header
        .as_ref()
        .and_then(|header| header.kid.as_deref())
        .unwrap_or("none");
    if let Some(source) = JwtJwksSourceConfig::from_jwt(jwt) {
        let snapshot =
            JwtJwksSharedCache::shared().snapshot(&source.source_identity, Instant::now());
        let state = snapshot
            .as_ref()
            .map(|entry| jwt_jwks_cache_state_name(entry.state))
            .unwrap_or("missing");
        let cache_reason = snapshot
            .as_ref()
            .and_then(|entry| entry.last_failure_reason)
            .map(|reason| reason.as_str())
            .unwrap_or("none");
        let stale_expired = snapshot
            .as_ref()
            .is_some_and(jwt_jwks_cache_stale_window_expired);
        log::debug!(
            "JWT validation rejected request: reason={} alg={} kid={} jwks_url={} jwks_state={} jwks_failure_reason={} stale_window_expired={}",
            failure.reason.as_str(),
            algorithm,
            kid,
            source.jwks_url,
            state,
            cache_reason,
            stale_expired
        );
        return;
    }
    log::debug!(
        "JWT validation rejected request: reason={} alg={} kid={}",
        failure.reason.as_str(),
        algorithm,
        kid
    );
}

fn observe_jwt_validation_failure(
    jwt: &RuntimeJwtAuth,
    token: &str,
    failure: &JwtValidationFailure,
) {
    let Some(metrics) = current_jwt_jwks_metrics() else {
        return;
    };
    metrics.record_jwt_validation_failure(failure.reason.as_str());
    let header = parse_compact_jwt(token)
        .ok()
        .and_then(|parsed| parse_jose_header(&parsed.header_bytes).ok());
    if matches!(
        failure.reason,
        JwtValidationFailureReason::AlgorithmNotAllowed
            | JwtValidationFailureReason::UnsupportedAlgorithm
            | JwtValidationFailureReason::MissingAlgorithm
    ) {
        let algorithm = header
            .as_ref()
            .map(|header| jwt_algorithm_name(header.algorithm))
            .unwrap_or("unknown");
        metrics.record_jwt_algorithm_rejection(algorithm);
    }
    if failure.reason == JwtValidationFailureReason::MissingVerificationKey
        && let Some(source) = JwtJwksSourceConfig::from_jwt(jwt)
        && header
            .as_ref()
            .and_then(|header| header.kid.as_deref())
            .is_some()
    {
        metrics.record_jwks_unknown_kid(&source.jwks_url);
    }
}

#[derive(Debug, Clone)]
struct ParsedJwt<'a> {
    header_b64: &'a str,
    payload_b64: &'a str,
    header_bytes: Vec<u8>,
    payload_bytes: Vec<u8>,
    signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedJoseHeader {
    algorithm: JwtAlgorithm,
    kid: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ValidatedJwt {
    /// Retained for per-algorithm validation metrics and key-type confusion
    /// checks; only read from tests until those land.
    #[allow(dead_code)]
    pub(super) algorithm: JwtAlgorithm,
    pub(super) claims: Value,
}

#[derive(Debug, Clone)]
enum JwtVerificationKey<'a> {
    Hs256Secret(&'a str),
    RsaPublicKey(PKey<Public>),
    EcP256PublicKey(EcKey<Public>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum JwtVerificationKeySource {
    StaticSecret,
    StaticAsymmetricKeys,
    RemoteJwks { source_identity: String },
}

#[derive(Debug, Clone)]
struct ResolvedJwtVerificationKey<'a> {
    source: JwtVerificationKeySource,
    key: JwtVerificationKey<'a>,
}

#[derive(Debug, Clone)]
enum JwtKeyResolution<'a> {
    Found(ResolvedJwtVerificationKey<'a>),
    StaleButUsable(ResolvedJwtVerificationKey<'a>),
    KeyNotFound {
        source: JwtVerificationKeySource,
    },
    SourceUnavailable {
        source: JwtVerificationKeySource,
    },
    ConfigurationInvalid {
        source: JwtVerificationKeySource,
        reason: JwtValidationFailureReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JwtJwksCacheState {
    NeverFetched,
    Fresh,
    Stale,
    RefreshFailedRetained,
    QuarantinedRetained,
    EmptyUnusable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JwtJwksSourceConfig {
    source_identity: String,
    jwks_url: String,
    allowed_algorithms: Vec<JwtAlgorithm>,
    refresh_interval: Duration,
    request_timeout: Duration,
    cache_ttl: Duration,
    stale_if_error: Duration,
    startup_behavior: JwksStartupBehavior,
}

impl JwtJwksSourceConfig {
    fn from_jwt(jwt: &RuntimeJwtAuth) -> Option<Self> {
        let jwks_url = jwt.jwks_url.as_ref()?.to_string();
        let mut allowed_algorithms = jwt
            .allowed_algorithms
            .iter()
            .copied()
            .filter(|algorithm| matches!(algorithm, JwtAlgorithm::Rs256 | JwtAlgorithm::Es256))
            .collect::<Vec<_>>();
        allowed_algorithms.sort_by_key(|algorithm| jwt_algorithm_name(*algorithm));
        allowed_algorithms.dedup();
        Some(Self {
            source_identity: jwt_jwks_source_identity(&jwks_url, &allowed_algorithms),
            jwks_url,
            allowed_algorithms,
            refresh_interval: jwt.jwks_refresh_interval,
            request_timeout: jwt.jwks_request_timeout,
            cache_ttl: jwt.jwks_cache_ttl,
            stale_if_error: jwt.jwks_stale_if_error,
            startup_behavior: jwt.jwks_startup_behavior.clone(),
        })
    }

    fn on_demand_refresh_cooldown(&self) -> Duration {
        self.refresh_interval
            .min(Duration::from_secs(30))
            .max(Duration::from_secs(5))
    }
}

#[derive(Debug, Clone)]
struct JwtJwksCacheEntry {
    source: JwtJwksSourceConfig,
    state: JwtJwksCacheState,
    active_keys: Vec<JwtJwksActiveKey>,
    refresh_in_flight: bool,
    last_refresh_started_at: Option<Instant>,
    last_refresh_started_wall: Option<SystemTime>,
    last_refresh_completed_at: Option<Instant>,
    last_refresh_completed_wall: Option<SystemTime>,
    last_success_at: Option<Instant>,
    last_success_wall: Option<SystemTime>,
    last_failure_at: Option<Instant>,
    last_failure_wall: Option<SystemTime>,
    last_error: Option<String>,
    last_failure_reason: Option<JwtJwksFetchFailureReason>,
    next_on_demand_refresh_at: Option<Instant>,
}

#[derive(Debug, Clone)]
struct JwtJwksCacheSnapshot {
    source: JwtJwksSourceConfig,
    state: JwtJwksCacheState,
    active_keys: Vec<RuntimeJwtVerificationKey>,
    last_error: Option<String>,
    last_failure_reason: Option<JwtJwksFetchFailureReason>,
    last_success_at: Option<Instant>,
    last_refresh_started_wall: Option<SystemTime>,
    last_success_wall: Option<SystemTime>,
}

#[derive(Debug, Clone)]
struct JwtJwksActiveKey {
    key: RuntimeJwtVerificationKey,
    retained_until: Option<Instant>,
}

struct JwtJwksSharedCache {
    entries: RwLock<HashMap<String, JwtJwksCacheEntry>>,
}

static JWT_JWKS_SHARED_CACHE: OnceLock<JwtJwksSharedCache> = OnceLock::new();
static JWT_JWKS_HTTP_CLIENT: OnceLock<JwtJwksHttpClient> = OnceLock::new();
static JWT_JWKS_METRICS_SINK: OnceLock<RwLock<Weak<crate::Metrics>>> = OnceLock::new();
#[cfg(test)]
type JwtJwksFetchScript = Mutex<HashMap<String, VecDeque<Result<Value, JwtJwksFetchFailure>>>>;
#[cfg(test)]
static JWT_JWKS_FETCH_SCRIPT: OnceLock<JwtJwksFetchScript> = OnceLock::new();

#[allow(dead_code)]
const MAX_JWKS_BODY_BYTES: usize = 256 * 1024;

impl JwtJwksSharedCache {
    fn shared() -> &'static Self {
        JWT_JWKS_SHARED_CACHE.get_or_init(|| Self {
            entries: RwLock::new(HashMap::new()),
        })
    }

    fn register_source(&self, source: JwtJwksSourceConfig) {
        let mut entries = self.entries.write().expect("jwks shared cache write lock");
        entries
            .entry(source.source_identity.clone())
            .and_modify(|entry| {
                let mut merged_algorithms = entry.source.allowed_algorithms.clone();
                for algorithm in &source.allowed_algorithms {
                    if !merged_algorithms.contains(algorithm) {
                        merged_algorithms.push(*algorithm);
                    }
                }
                merged_algorithms.sort_by_key(|algorithm| jwt_algorithm_name(*algorithm));
                entry.source.allowed_algorithms = merged_algorithms;
                entry.source.refresh_interval =
                    entry.source.refresh_interval.min(source.refresh_interval);
                entry.source.request_timeout =
                    entry.source.request_timeout.max(source.request_timeout);
                entry.source.cache_ttl = entry.source.cache_ttl.min(source.cache_ttl);
                entry.source.stale_if_error =
                    entry.source.stale_if_error.max(source.stale_if_error);
                entry.source.startup_behavior =
                    match (&entry.source.startup_behavior, &source.startup_behavior) {
                        (JwksStartupBehavior::RequireReady, _)
                        | (_, JwksStartupBehavior::RequireReady) => {
                            JwksStartupBehavior::RequireReady
                        }
                        _ => JwksStartupBehavior::AllowDegraded,
                    };
            })
            .or_insert_with(|| JwtJwksCacheEntry {
                source,
                state: JwtJwksCacheState::NeverFetched,
                active_keys: Vec::new(),
                refresh_in_flight: false,
                last_refresh_started_at: None,
                last_refresh_started_wall: None,
                last_refresh_completed_at: None,
                last_refresh_completed_wall: None,
                last_success_at: None,
                last_success_wall: None,
                last_failure_at: None,
                last_failure_wall: None,
                last_error: None,
                last_failure_reason: None,
                next_on_demand_refresh_at: None,
            });
    }

    fn snapshot(&self, source_identity: &str, now: Instant) -> Option<JwtJwksCacheSnapshot> {
        self.entries
            .read()
            .expect("jwks shared cache read lock")
            .get(source_identity)
            .map(|entry| JwtJwksCacheSnapshot {
                source: entry.source.clone(),
                state: entry.effective_state(now),
                active_keys: entry
                    .active_keys(now)
                    .into_iter()
                    .map(|active| active.key)
                    .collect(),
                last_error: entry.last_error.clone(),
                last_failure_reason: entry.last_failure_reason,
                last_success_at: entry.last_success_at,
                last_refresh_started_wall: entry.last_refresh_started_wall,
                last_success_wall: entry.last_success_wall,
            })
    }

    fn begin_refresh(&self, source: &JwtJwksSourceConfig, now: Instant) -> bool {
        let mut entries = self.entries.write().expect("jwks shared cache write lock");
        let Some(entry) = entries.get_mut(&source.source_identity) else {
            return false;
        };
        if entry.refresh_in_flight {
            return false;
        }
        entry.refresh_in_flight = true;
        entry.last_refresh_started_at = Some(now);
        entry.last_refresh_started_wall = Some(SystemTime::now());
        true
    }

    fn complete_refresh_success(
        &self,
        source_identity: &str,
        now: Instant,
        keys: Vec<RuntimeJwtVerificationKey>,
    ) {
        let mut entries = self.entries.write().expect("jwks shared cache write lock");
        let Some(entry) = entries.get_mut(source_identity) else {
            return;
        };
        entry.refresh_in_flight = false;
        entry.last_refresh_completed_at = Some(now);
        entry.last_refresh_completed_wall = Some(SystemTime::now());
        entry.last_error = None;
        entry.last_failure_reason = None;
        entry.last_failure_at = None;
        entry.last_failure_wall = None;
        if keys.is_empty() {
            if entry.active_keys(now).is_empty() {
                entry.active_keys.clear();
                entry.state = JwtJwksCacheState::EmptyUnusable;
            } else {
                entry.state = JwtJwksCacheState::QuarantinedRetained;
                entry.prune_expired_keys(now);
                entry.last_failure_at = Some(now);
                entry.last_failure_wall = Some(SystemTime::now());
                entry.last_error =
                    Some("empty_jwks: replacement produced no usable keys".to_string());
                entry.last_failure_reason = Some(JwtJwksFetchFailureReason::MalformedDocument);
            }
            return;
        }
        entry.active_keys = entry.rollover_keys(keys, now);
        entry.last_success_at = Some(now);
        entry.last_success_wall = Some(SystemTime::now());
        entry.state = JwtJwksCacheState::Fresh;
    }

    fn complete_refresh_failure(
        &self,
        source_identity: &str,
        now: Instant,
        failure: &JwtJwksFetchFailure,
    ) {
        let mut entries = self.entries.write().expect("jwks shared cache write lock");
        let Some(entry) = entries.get_mut(source_identity) else {
            return;
        };
        entry.refresh_in_flight = false;
        entry.last_refresh_completed_at = Some(now);
        entry.last_refresh_completed_wall = Some(SystemTime::now());
        entry.last_failure_at = Some(now);
        entry.last_failure_wall = Some(SystemTime::now());
        entry.last_error = Some(failure.to_string());
        entry.last_failure_reason = Some(failure.reason);
        if entry.active_keys(now).is_empty() {
            entry.state = JwtJwksCacheState::EmptyUnusable;
        } else {
            entry.state = JwtJwksCacheState::RefreshFailedRetained;
        }
    }

    fn schedule_on_demand_refresh(
        &self,
        source_identity: &str,
        now: Instant,
    ) -> Option<JwtJwksSourceConfig> {
        let mut entries = self.entries.write().expect("jwks shared cache write lock");
        let entry = entries.get_mut(source_identity)?;
        if entry.refresh_in_flight {
            return None;
        }
        if let Some(next_allowed) = entry.next_on_demand_refresh_at
            && now < next_allowed
        {
            return None;
        }
        entry.refresh_in_flight = true;
        entry.last_refresh_started_at = Some(now);
        entry.last_refresh_started_wall = Some(SystemTime::now());
        entry.next_on_demand_refresh_at = Some(now + entry.source.on_demand_refresh_cooldown());
        Some(entry.source.clone())
    }

    #[cfg(test)]
    fn upsert(&self, source_identity: &str, entry: JwtJwksCacheEntry) {
        self.entries
            .write()
            .expect("jwks shared cache write lock")
            .insert(source_identity.to_string(), entry);
    }

    #[cfg(test)]
    fn remove(&self, source_identity: &str) {
        self.entries
            .write()
            .expect("jwks shared cache write lock")
            .remove(source_identity);
    }
}

fn jwt_jwks_metrics_sink() -> &'static RwLock<Weak<crate::Metrics>> {
    JWT_JWKS_METRICS_SINK.get_or_init(|| RwLock::new(Weak::new()))
}

fn current_jwt_jwks_metrics() -> Option<Arc<crate::Metrics>> {
    jwt_jwks_metrics_sink()
        .read()
        .ok()
        .and_then(|metrics| metrics.upgrade())
}

impl JwtJwksCacheEntry {
    fn active_keys(&self, now: Instant) -> Vec<JwtJwksActiveKey> {
        self.active_keys
            .iter()
            .filter(|active| active.retained_until.is_none_or(|until| now <= until))
            .cloned()
            .collect()
    }

    fn prune_expired_keys(&mut self, now: Instant) {
        self.active_keys
            .retain(|active| active.retained_until.is_none_or(|until| now <= until));
    }

    fn rollover_keys(
        &self,
        new_keys: Vec<RuntimeJwtVerificationKey>,
        now: Instant,
    ) -> Vec<JwtJwksActiveKey> {
        let overlap_until = now + self.source.stale_if_error;
        let mut merged = new_keys
            .into_iter()
            .map(|key| JwtJwksActiveKey {
                key,
                retained_until: None,
            })
            .collect::<Vec<_>>();

        for existing in self.active_keys(now) {
            if merged
                .iter()
                .any(|candidate| jwt_verification_keys_equivalent(&candidate.key, &existing.key))
            {
                continue;
            }
            let existing_kid = static_key_metadata(&existing.key)
                .ok()
                .and_then(|metadata| metadata.kid);
            let same_kid_replaced = existing_kid.as_deref().is_some_and(|kid| {
                merged.iter().any(|candidate| {
                    static_key_metadata(&candidate.key)
                        .ok()
                        .and_then(|metadata| metadata.kid)
                        .as_deref()
                        == Some(kid)
                })
            });
            if same_kid_replaced {
                continue;
            }
            merged.push(JwtJwksActiveKey {
                key: existing.key,
                retained_until: Some(overlap_until),
            });
        }
        merged
    }

    fn effective_state(&self, now: Instant) -> JwtJwksCacheState {
        if self.active_keys(now).is_empty() {
            return match self.state {
                JwtJwksCacheState::NeverFetched => JwtJwksCacheState::NeverFetched,
                _ => JwtJwksCacheState::EmptyUnusable,
            };
        }
        let Some(last_success_at) = self.last_success_at else {
            return self.state;
        };
        let age = now.saturating_duration_since(last_success_at);
        if age <= self.source.cache_ttl {
            return match self.state {
                JwtJwksCacheState::RefreshFailedRetained => {
                    JwtJwksCacheState::RefreshFailedRetained
                }
                JwtJwksCacheState::QuarantinedRetained => JwtJwksCacheState::QuarantinedRetained,
                _ => JwtJwksCacheState::Fresh,
            };
        }
        if age
            <= self
                .source
                .cache_ttl
                .saturating_add(self.source.stale_if_error)
        {
            return match self.state {
                JwtJwksCacheState::RefreshFailedRetained => {
                    JwtJwksCacheState::RefreshFailedRetained
                }
                JwtJwksCacheState::QuarantinedRetained => JwtJwksCacheState::QuarantinedRetained,
                _ => JwtJwksCacheState::Stale,
            };
        }
        JwtJwksCacheState::EmptyUnusable
    }
}

#[allow(dead_code)]
struct JwtJwksHttpClient {
    client: Client<hyper_rustls::HttpsConnector<HttpConnector>, BoxBody<Bytes, Infallible>>,
}

#[allow(dead_code)]
impl JwtJwksHttpClient {
    fn shared() -> &'static Self {
        JWT_JWKS_HTTP_CLIENT.get_or_init(|| {
            let https = HttpsConnectorBuilder::new()
                .with_webpki_roots()
                .https_only()
                .enable_http1()
                .enable_http2()
                .build();
            let client = Client::builder(hyper_util::rt::TokioExecutor::new())
                .pool_max_idle_per_host(8)
                .pool_idle_timeout(Duration::from_secs(30))
                .build(https);
            Self { client }
        })
    }

    async fn send(
        &self,
        request: Request<BoxBody<Bytes, Infallible>>,
    ) -> Result<Response<Incoming>, JwtJwksFetchFailure> {
        self.client
            .request(request)
            .await
            .map_err(|err| JwtJwksFetchFailure::request_failed(err.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JwtJwksFetchFailureReason {
    RequestFailed,
    HttpStatus,
    MalformedDocument,
    AmbiguousDuplicateKid,
}

impl JwtJwksFetchFailureReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::RequestFailed => "request_failed",
            Self::HttpStatus => "http_status",
            Self::MalformedDocument => "malformed_document",
            Self::AmbiguousDuplicateKid => "ambiguous_duplicate_kid",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct JwtJwksFetchFailure {
    pub(super) reason: JwtJwksFetchFailureReason,
    detail: String,
}

#[allow(dead_code)]
impl JwtJwksFetchFailure {
    fn request_failed(detail: String) -> Self {
        Self {
            reason: JwtJwksFetchFailureReason::RequestFailed,
            detail,
        }
    }

    fn http_status(status: StatusCode) -> Self {
        Self {
            reason: JwtJwksFetchFailureReason::HttpStatus,
            detail: format!("jwks endpoint returned {status}"),
        }
    }

    fn malformed_document(detail: impl Into<String>) -> Self {
        Self {
            reason: JwtJwksFetchFailureReason::MalformedDocument,
            detail: detail.into(),
        }
    }

    fn ambiguous_duplicate_kid(kid: &str) -> Self {
        Self {
            reason: JwtJwksFetchFailureReason::AmbiguousDuplicateKid,
            detail: format!("duplicate jwks kid '{kid}'"),
        }
    }
}

impl std::fmt::Display for JwtJwksFetchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason.as_str(), self.detail)
    }
}

#[derive(Debug, Clone)]
struct NormalizedJwksDocument {
    keys: Vec<RuntimeJwtVerificationKey>,
}

#[cfg(test)]
pub(super) fn prime_jwks_cache_for_test(
    source_identity: &str,
    stale: bool,
    keys: Vec<RuntimeJwtVerificationKey>,
) {
    let source = JwtJwksSourceConfig {
        source_identity: source_identity.to_string(),
        jwks_url: source_identity.to_string(),
        allowed_algorithms: vec![JwtAlgorithm::Rs256, JwtAlgorithm::Es256],
        refresh_interval: Duration::from_secs(60),
        request_timeout: Duration::from_secs(5),
        cache_ttl: Duration::from_secs(60),
        stale_if_error: Duration::from_secs(60),
        startup_behavior: JwksStartupBehavior::AllowDegraded,
    };
    JwtJwksSharedCache::shared().upsert(
        source_identity,
        JwtJwksCacheEntry {
            source,
            state: if stale {
                JwtJwksCacheState::Stale
            } else {
                JwtJwksCacheState::Fresh
            },
            active_keys: keys
                .into_iter()
                .map(|key| JwtJwksActiveKey {
                    key,
                    retained_until: None,
                })
                .collect(),
            refresh_in_flight: false,
            last_refresh_started_at: None,
            last_refresh_started_wall: None,
            last_refresh_completed_at: None,
            last_refresh_completed_wall: None,
            last_success_at: Some(Instant::now()),
            last_success_wall: Some(SystemTime::now()),
            last_failure_at: None,
            last_failure_wall: None,
            last_error: None,
            last_failure_reason: None,
            next_on_demand_refresh_at: None,
        },
    );
}

#[cfg(test)]
pub(super) fn mark_jwks_source_unavailable_for_test(source_identity: &str) {
    let source = JwtJwksSourceConfig {
        source_identity: source_identity.to_string(),
        jwks_url: source_identity.to_string(),
        allowed_algorithms: vec![JwtAlgorithm::Rs256, JwtAlgorithm::Es256],
        refresh_interval: Duration::from_secs(60),
        request_timeout: Duration::from_secs(5),
        cache_ttl: Duration::from_secs(60),
        stale_if_error: Duration::from_secs(60),
        startup_behavior: JwksStartupBehavior::AllowDegraded,
    };
    JwtJwksSharedCache::shared().upsert(
        source_identity,
        JwtJwksCacheEntry {
            source,
            state: JwtJwksCacheState::NeverFetched,
            active_keys: Vec::new(),
            refresh_in_flight: false,
            last_refresh_started_at: None,
            last_refresh_started_wall: None,
            last_refresh_completed_at: None,
            last_refresh_completed_wall: None,
            last_success_at: None,
            last_success_wall: None,
            last_failure_at: Some(Instant::now()),
            last_failure_wall: Some(SystemTime::now()),
            last_error: Some("request_failed: scripted unavailable jwks source".to_string()),
            last_failure_reason: Some(JwtJwksFetchFailureReason::RequestFailed),
            next_on_demand_refresh_at: None,
        },
    );
}

#[cfg(test)]
pub(super) fn mark_jwks_source_invalid_for_test(source_identity: &str) {
    let source = JwtJwksSourceConfig {
        source_identity: source_identity.to_string(),
        jwks_url: source_identity.to_string(),
        allowed_algorithms: vec![JwtAlgorithm::Rs256, JwtAlgorithm::Es256],
        refresh_interval: Duration::from_secs(60),
        request_timeout: Duration::from_secs(5),
        cache_ttl: Duration::from_secs(60),
        stale_if_error: Duration::from_secs(60),
        startup_behavior: JwksStartupBehavior::AllowDegraded,
    };
    JwtJwksSharedCache::shared().upsert(
        source_identity,
        JwtJwksCacheEntry {
            source,
            state: JwtJwksCacheState::EmptyUnusable,
            active_keys: Vec::new(),
            refresh_in_flight: false,
            last_refresh_started_at: None,
            last_refresh_started_wall: None,
            last_refresh_completed_at: None,
            last_refresh_completed_wall: None,
            last_success_at: None,
            last_success_wall: None,
            last_failure_at: Some(Instant::now()),
            last_failure_wall: Some(SystemTime::now()),
            last_error: Some("malformed_document: scripted invalid jwks source".to_string()),
            last_failure_reason: Some(JwtJwksFetchFailureReason::MalformedDocument),
            next_on_demand_refresh_at: None,
        },
    );
}

#[cfg(test)]
pub(super) fn clear_jwks_cache_for_test(source_identity: &str) {
    JwtJwksSharedCache::shared().remove(source_identity);
}

fn jwt_jwks_source_identity(jwks_url: &str, allowed_algorithms: &[JwtAlgorithm]) -> String {
    let _ = allowed_algorithms;
    jwks_url.to_string()
}

fn runtime_jwks_sources(config: &RuntimeConfig) -> Vec<JwtJwksSourceConfig> {
    let mut sources = HashMap::<String, JwtJwksSourceConfig>::new();
    for upstream in config.upstreams.values() {
        let Some(jwt) = upstream.policy.upstream_auth.jwt.as_ref() else {
            continue;
        };
        let Some(source) = JwtJwksSourceConfig::from_jwt(jwt) else {
            continue;
        };
        sources
            .entry(source.source_identity.clone())
            .or_insert(source);
    }
    sources.into_values().collect()
}

impl QUICListener {
    pub(super) fn register_jwt_jwks_metrics(metrics: &Arc<crate::Metrics>) {
        if let Ok(mut sink) = jwt_jwks_metrics_sink().write() {
            *sink = Arc::downgrade(metrics);
        }
    }

    pub(super) fn initialize_jwks_startup(
        config: &RuntimeConfig,
    ) -> Result<(), spooky_errors::ProxyError> {
        let sources = runtime_jwks_sources(config);
        if sources.is_empty() {
            return Ok(());
        }

        for source in &sources {
            JwtJwksSharedCache::shared().register_source(source.clone());
        }

        let require_ready = sources
            .into_iter()
            .filter(|source| matches!(source.startup_behavior, JwksStartupBehavior::RequireReady))
            .collect::<Vec<_>>();
        if require_ready.is_empty() {
            return Ok(());
        }

        std::thread::spawn(move || -> Result<(), String> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| format!("failed to create JWKS startup runtime: {err}"))?;
            runtime.block_on(async move {
                for source in require_ready {
                    refresh_jwks_source_once(source.clone(), "startup_preflight")
                        .await
                        .map_err(|failure| {
                            format!(
                                "jwks startup preflight failed source={} startup_behavior=require_ready detail={}",
                                source.jwks_url, failure
                            )
                        })?;
                    let snapshot = JwtJwksSharedCache::shared()
                        .snapshot(&source.source_identity, Instant::now())
                        .ok_or_else(|| {
                            format!(
                                "jwks startup preflight failed source={} startup_behavior=require_ready detail=missing_cache_snapshot",
                                source.jwks_url
                            )
                        })?;
                    if !jwt_jwks_cache_state_usable(snapshot.state) {
                        return Err(format!(
                            "jwks startup preflight failed source={} startup_behavior=require_ready state={} detail={}",
                            source.jwks_url,
                            jwt_jwks_cache_state_name(snapshot.state),
                            snapshot.last_error.unwrap_or_else(|| {
                                "jwks source has no usable keys after startup preflight"
                                    .to_string()
                            })
                        ));
                    }
                }
                Ok(())
            })
        })
        .join()
        .map_err(|_| {
            spooky_errors::ProxyError::Transport(
                "jwks startup preflight thread panicked".to_string(),
            )
        })?
        .map_err(spooky_errors::ProxyError::Transport)
    }
}

fn maybe_spawn_jwks_on_demand_refresh(source: &JwtJwksSourceConfig) {
    let now = Instant::now();
    let Some(source) =
        JwtJwksSharedCache::shared().schedule_on_demand_refresh(&source.source_identity, now)
    else {
        return;
    };
    let Some(handle) = runtime_handle() else {
        return;
    };
    handle.spawn(async move {
        let _ = refresh_jwks_source_inflight(source, "on_demand_unknown_kid").await;
    });
}

fn jwt_jwks_cache_state_name(state: JwtJwksCacheState) -> &'static str {
    match state {
        JwtJwksCacheState::NeverFetched => "never_fetched",
        JwtJwksCacheState::Fresh => "fresh",
        JwtJwksCacheState::Stale => "stale",
        JwtJwksCacheState::RefreshFailedRetained => "refresh_failed_retained",
        JwtJwksCacheState::QuarantinedRetained => "quarantined_retained",
        JwtJwksCacheState::EmptyUnusable => "empty_unusable",
    }
}

fn jwt_jwks_cache_state_usable(state: JwtJwksCacheState) -> bool {
    matches!(
        state,
        JwtJwksCacheState::Fresh
            | JwtJwksCacheState::Stale
            | JwtJwksCacheState::RefreshFailedRetained
            | JwtJwksCacheState::QuarantinedRetained
    )
}

fn jwt_jwks_cache_stale_window_expired(snapshot: &JwtJwksCacheSnapshot) -> bool {
    matches!(snapshot.state, JwtJwksCacheState::EmptyUnusable) && snapshot.last_success_at.is_some()
}

#[derive(Debug, Clone)]
pub(super) struct JwtJwksRuntimeSnapshot {
    pub(super) jwks_url: String,
    pub(super) allowed_algorithms: Vec<String>,
    pub(super) startup_behavior: &'static str,
    pub(super) state: &'static str,
    pub(super) active_key_count: usize,
    pub(super) age_seconds: Option<u64>,
    pub(super) last_refresh_attempt_unix_seconds: Option<u64>,
    pub(super) last_refresh_success_unix_seconds: Option<u64>,
    pub(super) last_failure_reason: Option<String>,
    pub(super) last_error: Option<String>,
}

pub(super) fn snapshot_runtime_jwks_sources(config: &RuntimeConfig) -> Vec<JwtJwksRuntimeSnapshot> {
    let now = Instant::now();
    let mut snapshots = runtime_jwks_sources(config)
        .into_iter()
        .map(|source| {
            let entry = JwtJwksSharedCache::shared().snapshot(&source.source_identity, now);
            let state = entry
                .as_ref()
                .map(|entry| jwt_jwks_cache_state_name(entry.state))
                .unwrap_or("never_fetched");
            let active_key_count = entry
                .as_ref()
                .map(|entry| entry.active_keys.len())
                .unwrap_or_default();
            let age_seconds = entry.as_ref().and_then(|entry| {
                entry
                    .last_success_wall
                    .and_then(system_time_to_unix_seconds)
                    .and_then(|last_success| {
                        system_time_to_unix_seconds(SystemTime::now())
                            .map(|now| now.saturating_sub(last_success))
                    })
            });
            JwtJwksRuntimeSnapshot {
                jwks_url: source.jwks_url.clone(),
                allowed_algorithms: source
                    .allowed_algorithms
                    .iter()
                    .map(|algorithm| jwt_algorithm_name(*algorithm).to_string())
                    .collect(),
                startup_behavior: match source.startup_behavior {
                    JwksStartupBehavior::RequireReady => "require_ready",
                    JwksStartupBehavior::AllowDegraded => "allow_degraded",
                },
                state,
                active_key_count,
                age_seconds,
                last_refresh_attempt_unix_seconds: entry
                    .as_ref()
                    .and_then(|entry| entry.last_refresh_started_wall)
                    .and_then(system_time_to_unix_seconds),
                last_refresh_success_unix_seconds: entry
                    .as_ref()
                    .and_then(|entry| entry.last_success_wall)
                    .and_then(system_time_to_unix_seconds),
                last_failure_reason: entry
                    .as_ref()
                    .and_then(|entry| entry.last_failure_reason)
                    .map(|reason| reason.as_str().to_string()),
                last_error: entry.as_ref().and_then(|entry| entry.last_error.clone()),
            }
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|left, right| left.jwks_url.cmp(&right.jwks_url));
    snapshots
}

fn system_time_to_unix_seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn jwt_verification_keys_equivalent(
    left: &RuntimeJwtVerificationKey,
    right: &RuntimeJwtVerificationKey,
) -> bool {
    match (left, right) {
        (
            RuntimeJwtVerificationKey::Pem {
                kid: left_kid,
                alg: left_alg,
                public_key_pem: left_pem,
            },
            RuntimeJwtVerificationKey::Pem {
                kid: right_kid,
                alg: right_alg,
                public_key_pem: right_pem,
            },
        ) => left_kid == right_kid && left_alg == right_alg && left_pem == right_pem,
        (
            RuntimeJwtVerificationKey::Jwk {
                kid: left_kid,
                alg: left_alg,
                jwk: left_jwk,
            },
            RuntimeJwtVerificationKey::Jwk {
                kid: right_kid,
                alg: right_alg,
                jwk: right_jwk,
            },
        ) => left_kid == right_kid && left_alg == right_alg && left_jwk == right_jwk,
        _ => false,
    }
}

async fn refresh_jwks_source_once(
    source: JwtJwksSourceConfig,
    trigger: &'static str,
) -> Result<(), JwtJwksFetchFailure> {
    let cache = JwtJwksSharedCache::shared();
    let started_at = Instant::now();
    if !cache.begin_refresh(&source, started_at) {
        return Ok(());
    }
    if let Some(metrics) = current_jwt_jwks_metrics() {
        metrics.record_jwks_refresh_started(&source.jwks_url, SystemTime::now());
    }
    log::debug!(
        "JWKS refresh started source={} trigger={} configured_algorithms={:?}",
        source.jwks_url,
        trigger,
        source
            .allowed_algorithms
            .iter()
            .map(|algorithm| jwt_algorithm_name(*algorithm))
            .collect::<Vec<_>>()
    );
    refresh_jwks_source_inflight(source, trigger).await
}

async fn refresh_jwks_source_inflight(
    source: JwtJwksSourceConfig,
    trigger: &'static str,
) -> Result<(), JwtJwksFetchFailure> {
    let cache = JwtJwksSharedCache::shared();
    let previous = cache.snapshot(&source.source_identity, Instant::now());
    match fetch_and_normalize_jwks(
        &source.jwks_url,
        &source.allowed_algorithms,
        source.request_timeout,
    )
    .await
    {
        Ok(keys) => {
            cache.complete_refresh_success(&source.source_identity, Instant::now(), keys);
            if let Some(snapshot) = cache.snapshot(&source.source_identity, Instant::now()) {
                if let Some(metrics) = current_jwt_jwks_metrics() {
                    metrics.record_jwks_refresh_success(
                        &source.jwks_url,
                        jwt_jwks_cache_state_name(snapshot.state),
                        snapshot.active_keys.len(),
                        SystemTime::now(),
                        snapshot.last_success_wall,
                    );
                }
                log::info!(
                    "JWKS key-set replacement source={} trigger={} previous_active_keys={} active_keys={} state={}",
                    source.jwks_url,
                    trigger,
                    previous
                        .as_ref()
                        .map(|entry| entry.active_keys.len())
                        .unwrap_or_default(),
                    snapshot.active_keys.len(),
                    jwt_jwks_cache_state_name(snapshot.state)
                );
                match snapshot.state {
                    JwtJwksCacheState::Fresh | JwtJwksCacheState::Stale => {
                        log::debug!(
                            "JWKS refresh published usable key set source={} trigger={} state={} active_keys={}",
                            source.jwks_url,
                            trigger,
                            jwt_jwks_cache_state_name(snapshot.state),
                            snapshot.active_keys.len()
                        );
                    }
                    JwtJwksCacheState::QuarantinedRetained => {
                        log::warn!(
                            "JWKS refresh quarantined replacement and retained last-known-good keys source={} trigger={} state={} active_keys={} detail={}",
                            source.jwks_url,
                            trigger,
                            jwt_jwks_cache_state_name(snapshot.state),
                            snapshot.active_keys.len(),
                            snapshot
                                .last_error
                                .as_deref()
                                .unwrap_or("replacement produced no usable keys")
                        );
                    }
                    JwtJwksCacheState::EmptyUnusable => {
                        log::warn!(
                            "JWKS refresh left source unusable; JWT requests will be rejected source={} trigger={} state={} detail={}",
                            source.jwks_url,
                            trigger,
                            jwt_jwks_cache_state_name(snapshot.state),
                            snapshot
                                .last_error
                                .as_deref()
                                .unwrap_or("replacement produced no usable keys")
                        );
                    }
                    JwtJwksCacheState::NeverFetched | JwtJwksCacheState::RefreshFailedRetained => {
                        log::warn!(
                            "JWKS refresh ended in unexpected cache state source={} trigger={} state={}",
                            source.jwks_url,
                            trigger,
                            jwt_jwks_cache_state_name(snapshot.state)
                        );
                    }
                }
            }
            Ok(())
        }
        Err(failure) => {
            cache.complete_refresh_failure(&source.source_identity, Instant::now(), &failure);
            let snapshot = cache.snapshot(&source.source_identity, Instant::now());
            if let Some(metrics) = current_jwt_jwks_metrics() {
                metrics.record_jwks_refresh_failure(
                    &source.jwks_url,
                    snapshot
                        .as_ref()
                        .map(|entry| jwt_jwks_cache_state_name(entry.state))
                        .unwrap_or("missing"),
                    snapshot
                        .as_ref()
                        .map(|entry| entry.active_keys.len())
                        .unwrap_or_default(),
                    SystemTime::now(),
                    snapshot.as_ref().and_then(|entry| entry.last_success_wall),
                    Some(failure.reason.as_str()),
                );
            }
            let state = snapshot
                .as_ref()
                .map(|entry| jwt_jwks_cache_state_name(entry.state))
                .unwrap_or("missing");
            let retained_keys = snapshot
                .as_ref()
                .map(|entry| entry.active_keys.len())
                .unwrap_or(0);
            let action = if snapshot
                .as_ref()
                .is_some_and(|entry| jwt_jwks_cache_state_usable(entry.state))
            {
                "retain_last_known_good"
            } else {
                "reject_tokens"
            };
            log::warn!(
                "JWKS refresh failed source={} trigger={} state={} active_keys={} action={} detail={}",
                source.jwks_url,
                trigger,
                state,
                retained_keys,
                action,
                failure
            );
            Err(failure)
        }
    }
}

impl QUICListener {
    pub(super) fn spawn_jwks_refresh(
        config: &RuntimeConfig,
        task_registry: Arc<RuntimeTaskRegistry>,
    ) {
        let sources = runtime_jwks_sources(config);
        if sources.is_empty() {
            return;
        }
        let Some(handle) = runtime_handle() else {
            log::error!("JWKS refresh disabled: no Tokio runtime available");
            return;
        };

        for source in sources {
            JwtJwksSharedCache::shared().register_source(source.clone());
            let task_source = source.clone();
            let registration = spawn_supervised_async_task(
                &handle,
                "jwks-refresh",
                None,
                async move {
                    let _ = refresh_jwks_source_once(task_source.clone(), "startup").await;
                    if matches!(
                        task_source.startup_behavior,
                        JwksStartupBehavior::RequireReady
                    ) {
                        let snapshot = JwtJwksSharedCache::shared()
                            .snapshot(&task_source.source_identity, Instant::now());
                        if !matches!(
                            snapshot.as_ref().map(|entry| entry.state),
                            Some(
                                JwtJwksCacheState::Fresh
                                    | JwtJwksCacheState::Stale
                                    | JwtJwksCacheState::RefreshFailedRetained
                            )
                        ) {
                            log::warn!(
                                "JWKS source not ready after startup refresh source={} startup_behavior=require_ready",
                                task_source.jwks_url
                            );
                        }
                    }

                    let mut ticker = tokio::time::interval(
                        task_source.refresh_interval.max(Duration::from_secs(1)),
                    );
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        ticker.tick().await;
                        let _ = refresh_jwks_source_once(task_source.clone(), "periodic").await;
                    }
                },
            );
            task_registry.register(registration);
        }
    }
}

struct JwtKeyResolver<'a> {
    jwt: &'a RuntimeJwtAuth,
    algorithm: JwtAlgorithm,
    kid: Option<&'a str>,
    jwks_cache: &'static JwtJwksSharedCache,
}

impl<'a> JwtKeyResolver<'a> {
    fn new(jwt: &'a RuntimeJwtAuth, algorithm: JwtAlgorithm, kid: Option<&'a str>) -> Self {
        Self {
            jwt,
            algorithm,
            kid,
            jwks_cache: JwtJwksSharedCache::shared(),
        }
    }

    fn resolve(&self) -> JwtKeyResolution<'a> {
        if self.jwt.require_kid && self.kid.is_none() {
            return JwtKeyResolution::ConfigurationInvalid {
                source: match self.algorithm {
                    JwtAlgorithm::Hs256 => JwtVerificationKeySource::StaticSecret,
                    JwtAlgorithm::Rs256 | JwtAlgorithm::Es256 => {
                        if self.jwt.jwks_url.is_some() {
                            JwtVerificationKeySource::RemoteJwks {
                                source_identity: self.jwt.jwks_url.clone().unwrap_or_default(),
                            }
                        } else {
                            JwtVerificationKeySource::StaticAsymmetricKeys
                        }
                    }
                },
                reason: JwtValidationFailureReason::MissingKid,
            };
        }

        match self.algorithm {
            JwtAlgorithm::Hs256 => self.resolve_static_secret_source(),
            JwtAlgorithm::Rs256 | JwtAlgorithm::Es256 => self.resolve_asymmetric_sources(),
        }
    }

    fn resolve_static_secret_source(&self) -> JwtKeyResolution<'a> {
        if self.jwt.secret.is_empty() {
            return JwtKeyResolution::KeyNotFound {
                source: JwtVerificationKeySource::StaticSecret,
            };
        }
        JwtKeyResolution::Found(ResolvedJwtVerificationKey {
            source: JwtVerificationKeySource::StaticSecret,
            key: JwtVerificationKey::Hs256Secret(self.jwt.secret.as_str()),
        })
    }

    fn resolve_asymmetric_sources(&self) -> JwtKeyResolution<'a> {
        let mut found = Vec::new();
        let mut stale = Vec::new();
        let mut source_unavailable = None;
        let mut key_not_found = None;

        if !self.jwt.static_keys.is_empty() {
            match self.resolve_static_asymmetric_source() {
                JwtKeyResolution::Found(resolved) => found.push(resolved),
                JwtKeyResolution::StaleButUsable(resolved) => stale.push(resolved),
                JwtKeyResolution::KeyNotFound { source } => key_not_found = Some(source),
                JwtKeyResolution::SourceUnavailable { source } => source_unavailable = Some(source),
                JwtKeyResolution::ConfigurationInvalid { source, reason } => {
                    return JwtKeyResolution::ConfigurationInvalid { source, reason };
                }
            }
        }

        if let Some(source) = JwtJwksSourceConfig::from_jwt(self.jwt) {
            match self.resolve_remote_jwks_source(&source) {
                JwtKeyResolution::Found(resolved) => found.push(resolved),
                JwtKeyResolution::StaleButUsable(resolved) => stale.push(resolved),
                JwtKeyResolution::KeyNotFound { source } => key_not_found = Some(source),
                JwtKeyResolution::SourceUnavailable { source } => source_unavailable = Some(source),
                JwtKeyResolution::ConfigurationInvalid { source, reason } => {
                    return JwtKeyResolution::ConfigurationInvalid { source, reason };
                }
            }
        }

        if found.len() + stale.len() > 1 {
            return JwtKeyResolution::ConfigurationInvalid {
                source: if found
                    .first()
                    .map(|resolved| {
                        matches!(resolved.source, JwtVerificationKeySource::RemoteJwks { .. })
                    })
                    .unwrap_or(false)
                {
                    found[0].source.clone()
                } else if let Some(resolved) = stale.first() {
                    resolved.source.clone()
                } else {
                    JwtVerificationKeySource::StaticAsymmetricKeys
                },
                reason: JwtValidationFailureReason::AmbiguousVerificationKey,
            };
        }

        if let Some(resolved) = found.into_iter().next() {
            return JwtKeyResolution::Found(resolved);
        }
        if let Some(resolved) = stale.into_iter().next() {
            return JwtKeyResolution::StaleButUsable(resolved);
        }
        if let Some(source) = source_unavailable {
            return JwtKeyResolution::SourceUnavailable { source };
        }
        if let Some(source) = key_not_found {
            return JwtKeyResolution::KeyNotFound { source };
        }

        JwtKeyResolution::KeyNotFound {
            source: if let Some(source) = JwtJwksSourceConfig::from_jwt(self.jwt) {
                JwtVerificationKeySource::RemoteJwks {
                    source_identity: source.source_identity,
                }
            } else {
                JwtVerificationKeySource::StaticAsymmetricKeys
            },
        }
    }

    fn resolve_static_asymmetric_source(&self) -> JwtKeyResolution<'a> {
        resolve_matching_asymmetric_key(
            &self.jwt.static_keys,
            self.algorithm,
            self.kid,
            JwtVerificationKeySource::StaticAsymmetricKeys,
            JwtJwksCacheState::Fresh,
        )
    }

    fn resolve_remote_jwks_source(
        &self,
        source_config: &JwtJwksSourceConfig,
    ) -> JwtKeyResolution<'a> {
        self.jwks_cache.register_source(source_config.clone());
        let source = JwtVerificationKeySource::RemoteJwks {
            source_identity: source_config.source_identity.clone(),
        };
        let Some(entry) = self
            .jwks_cache
            .snapshot(&source_config.source_identity, Instant::now())
        else {
            return JwtKeyResolution::SourceUnavailable { source };
        };

        match entry.state {
            JwtJwksCacheState::NeverFetched => JwtKeyResolution::SourceUnavailable { source },
            JwtJwksCacheState::EmptyUnusable => {
                if matches!(
                    entry.last_failure_reason,
                    Some(
                        JwtJwksFetchFailureReason::MalformedDocument
                            | JwtJwksFetchFailureReason::AmbiguousDuplicateKid
                    )
                ) {
                    JwtKeyResolution::ConfigurationInvalid {
                        source,
                        reason: JwtValidationFailureReason::JwkKeyParseFailed,
                    }
                } else {
                    JwtKeyResolution::SourceUnavailable { source }
                }
            }
            JwtJwksCacheState::Fresh
            | JwtJwksCacheState::Stale
            | JwtJwksCacheState::RefreshFailedRetained
            | JwtJwksCacheState::QuarantinedRetained => {
                let resolution = resolve_matching_asymmetric_key(
                    &entry.active_keys,
                    self.algorithm,
                    self.kid,
                    source.clone(),
                    entry.state,
                );
                if matches!(resolution, JwtKeyResolution::StaleButUsable(_)) {
                    log::debug!(
                        "Serving JWT verification from stale JWKS cache source={} state={} kid={} alg={}",
                        source_config.jwks_url,
                        jwt_jwks_cache_state_name(entry.state),
                        self.kid.unwrap_or("none"),
                        jwt_algorithm_name(self.algorithm)
                    );
                }
                if matches!(resolution, JwtKeyResolution::KeyNotFound { .. }) && self.kid.is_some()
                {
                    log::debug!(
                        "Unknown JWKS kid encountered source={} kid={} alg={} action=trigger_refresh_hint",
                        source_config.jwks_url,
                        self.kid.unwrap_or("none"),
                        jwt_algorithm_name(self.algorithm)
                    );
                    maybe_spawn_jwks_on_demand_refresh(&entry.source);
                }
                resolution
            }
        }
    }
}

pub(super) fn validate_jwt_token(
    token: &str,
    jwt: &RuntimeJwtAuth,
    now: SystemTime,
) -> Result<ValidatedJwt, JwtValidationFailure> {
    let parsed = parse_compact_jwt(token)?;
    let header = parse_jose_header(&parsed.header_bytes)?;
    let algorithm = validate_jwt_algorithm_policy(jwt, header.algorithm)?;
    let key = match JwtKeyResolver::new(jwt, algorithm, header.kid.as_deref()).resolve() {
        JwtKeyResolution::Found(resolved) | JwtKeyResolution::StaleButUsable(resolved) => {
            resolved.key
        }
        JwtKeyResolution::KeyNotFound { .. } => {
            return Err(JwtValidationFailure::new(
                JwtValidationFailureReason::MissingVerificationKey,
            ));
        }
        JwtKeyResolution::SourceUnavailable { .. } => {
            return Err(JwtValidationFailure::new(
                JwtValidationFailureReason::KeySourceUnavailable,
            ));
        }
        JwtKeyResolution::ConfigurationInvalid { reason, .. } => {
            return Err(JwtValidationFailure::new(reason));
        }
    };
    verify_jwt_signature(&parsed, algorithm, key)?;
    let claims = parse_jwt_claims(&parsed.payload_bytes)?;
    validate_jwt_registered_claims(jwt, &claims, now)?;

    Ok(ValidatedJwt { algorithm, claims })
}

#[allow(dead_code)]
pub(super) async fn fetch_and_normalize_jwks(
    jwks_url: &str,
    allowed_algorithms: &[JwtAlgorithm],
    timeout: Duration,
) -> Result<Vec<RuntimeJwtVerificationKey>, JwtJwksFetchFailure> {
    let document = fetch_jwks_document(jwks_url, timeout).await?;
    let normalized = normalize_jwks_document(jwks_url, &document, allowed_algorithms)?;
    Ok(normalized.keys)
}

#[cfg(test)]
pub(super) fn normalize_jwks_document_for_test(
    jwks_url: &str,
    document: &Value,
    allowed_algorithms: &[JwtAlgorithm],
) -> Result<Vec<RuntimeJwtVerificationKey>, JwtJwksFetchFailure> {
    normalize_jwks_document(jwks_url, document, allowed_algorithms)
        .map(|normalized| normalized.keys)
}

#[allow(dead_code)]
async fn fetch_jwks_document(
    jwks_url: &str,
    timeout: Duration,
) -> Result<Value, JwtJwksFetchFailure> {
    #[cfg(test)]
    if let Some(result) = take_scripted_jwks_fetch_for_test(jwks_url) {
        return result;
    }

    let request = Request::builder()
        .method(http::Method::GET)
        .uri(jwks_url)
        .body(BoxBody::new(Full::new(Bytes::new())))
        .map_err(|err| JwtJwksFetchFailure::request_failed(err.to_string()))?;
    let response = tokio::time::timeout(timeout, JwtJwksHttpClient::shared().send(request))
        .await
        .map_err(|_| JwtJwksFetchFailure::request_failed("jwks request timed out".to_string()))??;
    if !response.status().is_success() {
        return Err(JwtJwksFetchFailure::http_status(response.status()));
    }
    let body = collect_jwks_body(response.into_body()).await?;
    serde_json::from_slice::<Value>(&body)
        .map_err(|err| JwtJwksFetchFailure::malformed_document(err.to_string()))
}

#[cfg(test)]
fn take_scripted_jwks_fetch_for_test(jwks_url: &str) -> Option<Result<Value, JwtJwksFetchFailure>> {
    JWT_JWKS_FETCH_SCRIPT
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("jwks fetch script lock")
        .get_mut(jwks_url)
        .and_then(VecDeque::pop_front)
}

#[cfg(test)]
pub(super) fn script_jwks_fetches_for_test(
    jwks_url: &str,
    responses: Vec<Result<Value, JwtJwksFetchFailure>>,
) {
    JWT_JWKS_FETCH_SCRIPT
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("jwks fetch script lock")
        .insert(jwks_url.to_string(), VecDeque::from(responses));
}

#[allow(dead_code)]
async fn collect_jwks_body(body: Incoming) -> Result<Vec<u8>, JwtJwksFetchFailure> {
    collect_jwks_body_bounded(body).await
}

async fn collect_jwks_body_bounded<B>(mut body: B) -> Result<Vec<u8>, JwtJwksFetchFailure>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|err| JwtJwksFetchFailure::request_failed(err.to_string()))?;
        let Ok(chunk) = frame.into_data() else {
            continue;
        };
        let next_len = bytes.len().saturating_add(chunk.len());
        if next_len > MAX_JWKS_BODY_BYTES {
            return Err(JwtJwksFetchFailure::malformed_document(format!(
                "jwks document exceeded {} bytes",
                MAX_JWKS_BODY_BYTES
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn normalize_jwks_document(
    jwks_url: &str,
    document: &Value,
    allowed_algorithms: &[JwtAlgorithm],
) -> Result<NormalizedJwksDocument, JwtJwksFetchFailure> {
    let keys = document
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| JwtJwksFetchFailure::malformed_document("jwks document missing keys[]"))?;
    let allowed_algorithms = allowed_algorithms.iter().copied().collect::<HashSet<_>>();
    let mut normalized = Vec::new();
    let mut seen_kids = HashSet::new();

    for (index, jwk) in keys.iter().enumerate() {
        let Some(key) = (match normalize_jwks_key(jwk, &allowed_algorithms) {
            Ok(key) => key,
            Err(detail) => {
                log::warn!(
                    "Ignoring suspicious JWKS key source={} index={} reason={}",
                    jwks_url,
                    index,
                    detail
                );
                None
            }
        }) else {
            continue;
        };
        let effective_kid = static_key_metadata(&key)
            .map_err(|failure| JwtJwksFetchFailure::malformed_document(failure.reason.as_str()))?
            .kid;
        if let Some(kid) = effective_kid.as_deref()
            && !seen_kids.insert(kid.to_string())
        {
            return Err(JwtJwksFetchFailure::ambiguous_duplicate_kid(kid));
        }
        normalized.push(key);
    }

    let configured_algorithms = allowed_algorithms
        .iter()
        .copied()
        .map(jwt_algorithm_name)
        .collect::<Vec<_>>()
        .join(",");
    log::debug!(
        "JWKS fetch normalized source={} accepted_keys={} configured_algorithms={}",
        jwks_url,
        normalized.len(),
        configured_algorithms
    );

    Ok(NormalizedJwksDocument { keys: normalized })
}

fn normalize_jwks_key(
    jwk: &Value,
    allowed_algorithms: &HashSet<JwtAlgorithm>,
) -> Result<Option<RuntimeJwtVerificationKey>, String> {
    let Some(jwk_object) = jwk.as_object() else {
        return Err("must be a JSON object".to_string());
    };

    if let Some(use_value) = jwk_object.get("use").and_then(Value::as_str)
        && use_value != "sig"
    {
        log::debug!("Ignoring JWKS key: key use '{}' is not accepted", use_value);
        return Ok(None);
    }

    if let Some(key_ops) = jwk_object.get("key_ops").and_then(Value::as_array)
        && !key_ops
            .iter()
            .filter_map(Value::as_str)
            .any(|operation| operation == "verify")
    {
        log::debug!("Ignoring JWKS key: key_ops does not include verify");
        return Ok(None);
    }

    let algorithm = match jwk_object.get("alg").and_then(Value::as_str) {
        Some(alg) => parse_jwt_alg_str(alg)
            .map_err(|_| format!("declares unsupported alg '{alg}' for jwks normalization"))?,
        None => infer_jwk_algorithm(jwk)?,
    };
    if !matches!(algorithm, JwtAlgorithm::Rs256 | JwtAlgorithm::Es256) {
        log::debug!(
            "Ignoring JWKS key: algorithm '{}' is not an asymmetric signing algorithm",
            jwt_algorithm_name(algorithm)
        );
        return Ok(None);
    }
    if !allowed_algorithms.contains(&algorithm) {
        log::debug!(
            "Ignoring JWKS key: algorithm '{}' is not enabled by policy",
            jwt_algorithm_name(algorithm)
        );
        return Ok(None);
    }

    let kid = jwk_object
        .get("kid")
        .and_then(Value::as_str)
        .map(str::to_string);
    let jwk_string =
        serde_json::to_string(jwk).map_err(|err| format!("failed to serialize jwk: {err}"))?;
    let normalized = RuntimeJwtVerificationKey::Jwk {
        kid,
        alg: Some(algorithm),
        jwk: jwk_string,
    };
    parse_static_verification_key(&normalized, algorithm).map_err(|failure| {
        format!(
            "cannot be used for {} verification: {}",
            jwt_algorithm_name(algorithm),
            failure.reason.as_str()
        )
    })?;
    Ok(Some(normalized))
}

fn infer_jwk_algorithm(jwk: &Value) -> Result<JwtAlgorithm, String> {
    let Some(kty) = jwk.get("kty").and_then(Value::as_str) else {
        return Err("is missing kty".to_string());
    };
    match kty {
        "RSA" => Ok(JwtAlgorithm::Rs256),
        "EC" => match jwk.get("crv").and_then(Value::as_str) {
            Some("P-256") => Ok(JwtAlgorithm::Es256),
            Some(other) => Err(format!("declares unsupported EC curve '{other}'")),
            None => Err("is missing crv for EC key".to_string()),
        },
        other => Err(format!("declares unsupported kty '{other}'")),
    }
}

fn jwt_algorithm_name(algorithm: JwtAlgorithm) -> &'static str {
    match algorithm {
        JwtAlgorithm::Hs256 => "HS256",
        JwtAlgorithm::Rs256 => "RS256",
        JwtAlgorithm::Es256 => "ES256",
    }
}

fn parse_compact_jwt(token: &str) -> Result<ParsedJwt<'_>, JwtValidationFailure> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(signature_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::MalformedToken,
        ));
    };
    let header_bytes = URL_SAFE_NO_PAD
        .decode(header_b64)
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::MalformedHeader))?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::MalformedClaims))?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::MalformedToken))?;

    Ok(ParsedJwt {
        header_b64,
        payload_b64,
        header_bytes,
        payload_bytes,
        signature,
    })
}

fn parse_jose_header(header_bytes: &[u8]) -> Result<ParsedJoseHeader, JwtValidationFailure> {
    let header = serde_json::from_slice::<Value>(header_bytes)
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::MalformedHeader))?;
    let alg = header
        .get("alg")
        .and_then(Value::as_str)
        .ok_or_else(|| JwtValidationFailure::new(JwtValidationFailureReason::MissingAlgorithm))?;
    let algorithm = match alg {
        "HS256" => JwtAlgorithm::Hs256,
        "RS256" => JwtAlgorithm::Rs256,
        "ES256" => JwtAlgorithm::Es256,
        _ => {
            return Err(JwtValidationFailure::new(
                JwtValidationFailureReason::UnsupportedAlgorithm,
            ));
        }
    };

    Ok(ParsedJoseHeader {
        algorithm,
        kid: header
            .get("kid")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn validate_jwt_algorithm_policy(
    jwt: &RuntimeJwtAuth,
    algorithm: JwtAlgorithm,
) -> Result<JwtAlgorithm, JwtValidationFailure> {
    if !jwt.allowed_algorithms.contains(&algorithm) {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::AlgorithmNotAllowed,
        ));
    }
    match algorithm {
        JwtAlgorithm::Hs256 | JwtAlgorithm::Rs256 | JwtAlgorithm::Es256 => Ok(algorithm),
    }
}

fn verify_jwt_signature(
    parsed: &ParsedJwt<'_>,
    algorithm: JwtAlgorithm,
    key: JwtVerificationKey<'_>,
) -> Result<(), JwtValidationFailure> {
    match (algorithm, key) {
        (JwtAlgorithm::Hs256, JwtVerificationKey::Hs256Secret(secret)) => {
            let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
                return Err(JwtValidationFailure::new(
                    JwtValidationFailureReason::MissingVerificationKey,
                ));
            };
            mac.update(format!("{}.{}", parsed.header_b64, parsed.payload_b64).as_bytes());
            let expected = mac.finalize().into_bytes();
            if expected.len() != parsed.signature.len()
                || !bool::from(expected.as_slice().ct_eq(parsed.signature.as_slice()))
            {
                return Err(JwtValidationFailure::new(
                    JwtValidationFailureReason::SignatureInvalid,
                ));
            }
            Ok(())
        }
        (JwtAlgorithm::Rs256, JwtVerificationKey::RsaPublicKey(public_key)) => {
            let mut verifier =
                Verifier::new(MessageDigest::sha256(), &public_key).map_err(|_| {
                    JwtValidationFailure::new(JwtValidationFailureReason::SignatureInvalid)
                })?;
            verifier.set_rsa_padding(Padding::PKCS1).map_err(|_| {
                JwtValidationFailure::new(JwtValidationFailureReason::SignatureInvalid)
            })?;
            verifier
                .update(format!("{}.{}", parsed.header_b64, parsed.payload_b64).as_bytes())
                .map_err(|_| {
                    JwtValidationFailure::new(JwtValidationFailureReason::SignatureInvalid)
                })?;
            if !verifier.verify(&parsed.signature).map_err(|_| {
                JwtValidationFailure::new(JwtValidationFailureReason::SignatureInvalid)
            })? {
                return Err(JwtValidationFailure::new(
                    JwtValidationFailureReason::SignatureInvalid,
                ));
            }
            Ok(())
        }
        (JwtAlgorithm::Es256, JwtVerificationKey::EcP256PublicKey(public_key)) => {
            let der_signature = jose_es256_signature_to_der(&parsed.signature)?;
            let digest = Sha256::digest(format!("{}.{}", parsed.header_b64, parsed.payload_b64));
            let ecdsa_sig = EcdsaSig::from_der(&der_signature).map_err(|_| {
                JwtValidationFailure::new(JwtValidationFailureReason::SignatureInvalid)
            })?;
            if !ecdsa_sig.verify(&digest, &public_key).map_err(|_| {
                JwtValidationFailure::new(JwtValidationFailureReason::SignatureInvalid)
            })? {
                return Err(JwtValidationFailure::new(
                    JwtValidationFailureReason::SignatureInvalid,
                ));
            }
            Ok(())
        }
        (JwtAlgorithm::Hs256, JwtVerificationKey::RsaPublicKey(_))
        | (JwtAlgorithm::Hs256, JwtVerificationKey::EcP256PublicKey(_)) => Err(
            JwtValidationFailure::new(JwtValidationFailureReason::InvalidKeyType),
        ),
        (JwtAlgorithm::Rs256 | JwtAlgorithm::Es256, _) => Err(JwtValidationFailure::new(
            JwtValidationFailureReason::InvalidKeyType,
        )),
    }
}

fn resolve_matching_asymmetric_key<'a>(
    keys: &[RuntimeJwtVerificationKey],
    algorithm: JwtAlgorithm,
    requested_kid: Option<&str>,
    source: JwtVerificationKeySource,
    cache_state: JwtJwksCacheState,
) -> JwtKeyResolution<'a> {
    let mut candidates = Vec::new();
    for key in keys {
        let metadata = match static_key_metadata(key) {
            Ok(metadata) => metadata,
            Err(failure) => {
                return JwtKeyResolution::ConfigurationInvalid {
                    source,
                    reason: failure.reason,
                };
            }
        };
        let effective_kid = metadata
            .kid
            .as_deref()
            .or_else(|| static_key_config_kid(key));
        match requested_kid {
            Some(requested_kid) => {
                if effective_kid != Some(requested_kid) {
                    continue;
                }
            }
            None => {
                // Tokens without a `kid` are only accepted when exactly one
                // algorithm-compatible key remains after policy filtering.
                // Multiple candidates are treated as ambiguous rather than
                // guessing which verification mode the issuer intended.
            }
        }
        if let Some(key_alg) = metadata.alg.or_else(|| static_key_config_alg(key))
            && key_alg != algorithm
        {
            continue;
        }
        candidates.push(key);
    }

    if candidates.is_empty() {
        return JwtKeyResolution::KeyNotFound { source };
    }
    if candidates.len() > 1 {
        return JwtKeyResolution::ConfigurationInvalid {
            source,
            reason: JwtValidationFailureReason::AmbiguousVerificationKey,
        };
    }

    // Both callers filter these states before selecting a key, so this is
    // defensive: degrade to a rejection rather than panicking on the request
    // path if that ever stops holding.
    match cache_state {
        JwtJwksCacheState::NeverFetched | JwtJwksCacheState::EmptyUnusable => {
            return JwtKeyResolution::SourceUnavailable { source };
        }
        JwtJwksCacheState::Fresh
        | JwtJwksCacheState::Stale
        | JwtJwksCacheState::RefreshFailedRetained
        | JwtJwksCacheState::QuarantinedRetained => {}
    }

    let resolved_key = match parse_static_verification_key(candidates[0], algorithm) {
        Ok(key) => key,
        Err(failure) => {
            return JwtKeyResolution::ConfigurationInvalid {
                source,
                reason: failure.reason,
            };
        }
    };
    let resolved = ResolvedJwtVerificationKey {
        source,
        key: resolved_key,
    };
    match cache_state {
        JwtJwksCacheState::Stale
        | JwtJwksCacheState::RefreshFailedRetained
        | JwtJwksCacheState::QuarantinedRetained => JwtKeyResolution::StaleButUsable(resolved),
        _ => JwtKeyResolution::Found(resolved),
    }
}

#[derive(Debug, Clone, Default)]
struct StaticKeyMetadata {
    kid: Option<String>,
    alg: Option<JwtAlgorithm>,
}

fn static_key_metadata(
    key: &RuntimeJwtVerificationKey,
) -> Result<StaticKeyMetadata, JwtValidationFailure> {
    match key {
        RuntimeJwtVerificationKey::Pem { kid, alg, .. } => Ok(StaticKeyMetadata {
            kid: kid.clone(),
            alg: *alg,
        }),
        RuntimeJwtVerificationKey::Jwk { kid, alg, jwk } => {
            let parsed = parse_jwk_value(jwk)?;
            let jwk_kid = parsed
                .get("kid")
                .and_then(Value::as_str)
                .map(str::to_string);
            let jwk_alg = parsed
                .get("alg")
                .and_then(Value::as_str)
                .map(parse_jwt_alg_str)
                .transpose()?;
            if let (Some(config_kid), Some(jwk_kid)) = (kid.as_deref(), jwk_kid.as_deref())
                && config_kid != jwk_kid
            {
                return Err(JwtValidationFailure::new(
                    JwtValidationFailureReason::JwkKeyParseFailed,
                ));
            }
            if let (Some(config_alg), Some(jwk_alg)) = (*alg, jwk_alg)
                && config_alg != jwk_alg
            {
                return Err(JwtValidationFailure::new(
                    JwtValidationFailureReason::JwkKeyParseFailed,
                ));
            }
            Ok(StaticKeyMetadata {
                kid: kid.clone().or(jwk_kid),
                alg: alg.or(jwk_alg),
            })
        }
    }
}

fn static_key_config_kid(key: &RuntimeJwtVerificationKey) -> Option<&str> {
    match key {
        RuntimeJwtVerificationKey::Pem { kid, .. } | RuntimeJwtVerificationKey::Jwk { kid, .. } => {
            kid.as_deref()
        }
    }
}

fn static_key_config_alg(key: &RuntimeJwtVerificationKey) -> Option<JwtAlgorithm> {
    match key {
        RuntimeJwtVerificationKey::Pem { alg, .. } | RuntimeJwtVerificationKey::Jwk { alg, .. } => {
            *alg
        }
    }
}

fn parse_static_verification_key(
    key: &RuntimeJwtVerificationKey,
    algorithm: JwtAlgorithm,
) -> Result<JwtVerificationKey<'static>, JwtValidationFailure> {
    match key {
        RuntimeJwtVerificationKey::Pem { public_key_pem, .. } => {
            parse_pem_verification_key(public_key_pem, algorithm)
        }
        RuntimeJwtVerificationKey::Jwk { jwk, .. } => parse_jwk_verification_key(jwk, algorithm),
    }
}

fn parse_pem_verification_key(
    public_key_pem: &str,
    algorithm: JwtAlgorithm,
) -> Result<JwtVerificationKey<'static>, JwtValidationFailure> {
    match algorithm {
        JwtAlgorithm::Rs256 => {
            if let Ok(public_key) = PKey::public_key_from_pem(public_key_pem.as_bytes()) {
                if !matches!(public_key.id(), PKeyId::RSA | PKeyId::RSAPSS) {
                    return Err(JwtValidationFailure::new(
                        JwtValidationFailureReason::InvalidKeyType,
                    ));
                }
                ensure_rsa_key_strength(&public_key)?;
                return Ok(JwtVerificationKey::RsaPublicKey(public_key));
            }
            let rsa = Rsa::public_key_from_pem(public_key_pem.as_bytes())
                .or_else(|_| Rsa::public_key_from_pem_pkcs1(public_key_pem.as_bytes()))
                .map_err(|_| {
                    JwtValidationFailure::new(JwtValidationFailureReason::PemKeyParseFailed)
                })?;
            let key = PKey::from_rsa(rsa).map_err(|_| {
                JwtValidationFailure::new(JwtValidationFailureReason::PemKeyParseFailed)
            })?;
            ensure_rsa_key_strength(&key)?;
            Ok(JwtVerificationKey::RsaPublicKey(key))
        }
        JwtAlgorithm::Es256 => {
            if let Ok(public_key) = PKey::public_key_from_pem(public_key_pem.as_bytes()) {
                if public_key.id() != PKeyId::EC {
                    return Err(JwtValidationFailure::new(
                        JwtValidationFailureReason::InvalidKeyType,
                    ));
                }
                let ec_key = public_key.ec_key().map_err(|_| {
                    JwtValidationFailure::new(JwtValidationFailureReason::PemKeyParseFailed)
                })?;
                ensure_p256_public_key(&ec_key)?;
                return Ok(JwtVerificationKey::EcP256PublicKey(ec_key));
            }
            let ec_key = EcKey::public_key_from_pem(public_key_pem.as_bytes()).map_err(|_| {
                JwtValidationFailure::new(JwtValidationFailureReason::PemKeyParseFailed)
            })?;
            ensure_p256_public_key(&ec_key)?;
            Ok(JwtVerificationKey::EcP256PublicKey(ec_key))
        }
        JwtAlgorithm::Hs256 => Err(JwtValidationFailure::new(
            JwtValidationFailureReason::InvalidKeyType,
        )),
    }
}

fn parse_jwk_verification_key(
    jwk: &str,
    algorithm: JwtAlgorithm,
) -> Result<JwtVerificationKey<'static>, JwtValidationFailure> {
    let jwk = parse_jwk_value(jwk)?;
    match algorithm {
        JwtAlgorithm::Rs256 => {
            let kty = jwk.get("kty").and_then(Value::as_str);
            if kty != Some("RSA") {
                return Err(JwtValidationFailure::new(
                    JwtValidationFailureReason::InvalidKeyType,
                ));
            }
            let n = jwk
                .get("n")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    JwtValidationFailure::new(JwtValidationFailureReason::JwkKeyParseFailed)
                })
                .and_then(decode_jwk_bignum)?;
            let e = jwk
                .get("e")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    JwtValidationFailure::new(JwtValidationFailureReason::JwkKeyParseFailed)
                })
                .and_then(decode_jwk_bignum)?;
            let rsa = Rsa::from_public_components(n, e).map_err(|_| {
                JwtValidationFailure::new(JwtValidationFailureReason::JwkKeyParseFailed)
            })?;
            let key = PKey::from_rsa(rsa).map_err(|_| {
                JwtValidationFailure::new(JwtValidationFailureReason::JwkKeyParseFailed)
            })?;
            ensure_rsa_key_strength(&key)?;
            Ok(JwtVerificationKey::RsaPublicKey(key))
        }
        JwtAlgorithm::Es256 => {
            let kty = jwk.get("kty").and_then(Value::as_str);
            if kty != Some("EC") {
                return Err(JwtValidationFailure::new(
                    JwtValidationFailureReason::InvalidKeyType,
                ));
            }
            let crv = jwk.get("crv").and_then(Value::as_str);
            if crv != Some("P-256") {
                return Err(JwtValidationFailure::new(
                    JwtValidationFailureReason::UnsupportedCurve,
                ));
            }
            let x = jwk
                .get("x")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    JwtValidationFailure::new(JwtValidationFailureReason::JwkKeyParseFailed)
                })
                .and_then(decode_jwk_bignum)?;
            let y = jwk
                .get("y")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    JwtValidationFailure::new(JwtValidationFailureReason::JwkKeyParseFailed)
                })
                .and_then(decode_jwk_bignum)?;
            let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).map_err(|_| {
                JwtValidationFailure::new(JwtValidationFailureReason::UnsupportedCurve)
            })?;
            let ec_key =
                EcKey::from_public_key_affine_coordinates(&group, &x, &y).map_err(|_| {
                    JwtValidationFailure::new(JwtValidationFailureReason::JwkKeyParseFailed)
                })?;
            ensure_p256_public_key(&ec_key)?;
            Ok(JwtVerificationKey::EcP256PublicKey(ec_key))
        }
        JwtAlgorithm::Hs256 => Err(JwtValidationFailure::new(
            JwtValidationFailureReason::InvalidKeyType,
        )),
    }
}

fn parse_jwk_value(jwk: &str) -> Result<Value, JwtValidationFailure> {
    serde_json::from_str::<Value>(jwk)
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::JwkKeyParseFailed))
}

fn parse_jwt_alg_str(alg: &str) -> Result<JwtAlgorithm, JwtValidationFailure> {
    match alg {
        "HS256" => Ok(JwtAlgorithm::Hs256),
        "RS256" => Ok(JwtAlgorithm::Rs256),
        "ES256" => Ok(JwtAlgorithm::Es256),
        _ => Err(JwtValidationFailure::new(
            JwtValidationFailureReason::JwkKeyParseFailed,
        )),
    }
}

fn decode_jwk_bignum(encoded: &str) -> Result<BigNum, JwtValidationFailure> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::JwkKeyParseFailed))?;
    BigNum::from_slice(&bytes)
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::JwkKeyParseFailed))
}

/// Smallest RSA modulus accepted for RS256 verification. Anything shorter is
/// forgeable in practice, so reject it rather than trusting operator config or
/// a remote JWKS document to only publish sound keys.
const MIN_RSA_KEY_BITS: u32 = 2048;

fn ensure_rsa_key_strength(key: &PKey<Public>) -> Result<(), JwtValidationFailure> {
    let bits = key
        .rsa()
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::InvalidKeyType))?
        .size()
        .saturating_mul(8);
    if bits < MIN_RSA_KEY_BITS {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::KeyTooWeak,
        ));
    }
    Ok(())
}

fn ensure_p256_public_key(ec_key: &EcKey<Public>) -> Result<(), JwtValidationFailure> {
    ec_key
        .check_key()
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::InvalidKeyType))?;
    let curve = ec_key.group().curve_name();
    if curve != Some(Nid::X9_62_PRIME256V1) {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::UnsupportedCurve,
        ));
    }
    Ok(())
}

fn jose_es256_signature_to_der(signature: &[u8]) -> Result<Vec<u8>, JwtValidationFailure> {
    if signature.len() != 64 {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::SignatureInvalid,
        ));
    }
    let r = BigNum::from_slice(&signature[..32])
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::SignatureInvalid))?;
    let s = BigNum::from_slice(&signature[32..])
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::SignatureInvalid))?;
    // `from_private_components` assembles a signature from its (r, s) scalars;
    // despite the name it involves no private key material.
    EcdsaSig::from_private_components(r, s)
        .and_then(|sig| sig.to_der())
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::SignatureInvalid))
}

fn parse_jwt_claims(payload_bytes: &[u8]) -> Result<Value, JwtValidationFailure> {
    serde_json::from_slice::<Value>(payload_bytes)
        .map_err(|_| JwtValidationFailure::new(JwtValidationFailureReason::MalformedClaims))
}

fn validate_jwt_registered_claims(
    jwt: &RuntimeJwtAuth,
    claims: &Value,
    now: SystemTime,
) -> Result<(), JwtValidationFailure> {
    let Ok(now_secs) = now.duration_since(UNIX_EPOCH).map(|value| value.as_secs()) else {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::MalformedClaims,
        ));
    };
    let clock_skew_secs = jwt.clock_skew.as_secs();
    let exp = claims
        .get("exp")
        .and_then(Value::as_u64)
        .ok_or_else(|| JwtValidationFailure::new(JwtValidationFailureReason::MissingExpiration))?;
    if now_secs > exp.saturating_add(clock_skew_secs) {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::TokenExpired,
        ));
    }
    if claims
        .get("nbf")
        .and_then(Value::as_u64)
        .is_some_and(|nbf| now_secs.saturating_add(clock_skew_secs) < nbf)
    {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::TokenNotYetValid,
        ));
    }
    if claims
        .get("iat")
        .and_then(Value::as_u64)
        .is_some_and(|iat| now_secs.saturating_add(clock_skew_secs) < iat)
    {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::TokenIssuedInFuture,
        ));
    }
    if !jwt_issuer_matches(jwt, claims) {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::IssuerMismatch,
        ));
    }
    if !jwt_audience_matches(jwt, claims) {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::AudienceMismatch,
        ));
    }

    Ok(())
}

fn jwt_issuer_matches(jwt: &RuntimeJwtAuth, claims: &Value) -> bool {
    let expected = jwt_expected_issuers(jwt);
    if expected.is_empty() {
        return true;
    }
    let actual = claims.get("iss").and_then(Value::as_str);
    expected.into_iter().any(|issuer| actual == Some(issuer))
}

fn jwt_audience_matches(jwt: &RuntimeJwtAuth, claims: &Value) -> bool {
    let expected = jwt_expected_audiences(jwt);
    if expected.is_empty() {
        return true;
    }

    let Some(claim_aud) = claims.get("aud") else {
        return false;
    };
    match claim_aud {
        Value::String(value) => expected.contains(&value.as_str()),
        Value::Array(values) => values.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| expected.contains(&value))
        }),
        _ => false,
    }
}

fn jwt_expected_issuers(jwt: &RuntimeJwtAuth) -> Vec<&str> {
    if let Some(issuer) = jwt.issuer.as_deref() {
        vec![issuer]
    } else {
        jwt.issuers.iter().map(String::as_str).collect()
    }
}

fn jwt_expected_audiences(jwt: &RuntimeJwtAuth) -> Vec<&str> {
    if let Some(audience) = jwt.audience.as_deref() {
        vec![audience]
    } else {
        jwt.audiences.iter().map(String::as_str).collect()
    }
}

pub(super) fn jwt_claims_satisfy_rbac(policy: &RuntimeUpstreamPolicy, claims: &Value) -> bool {
    let scopes = jwt_string_claim_values(claims, &["scope", "scp"]);
    let roles = jwt_string_claim_values(claims, &["roles", "role"]);
    policy
        .upstream_auth
        .required_scopes
        .iter()
        .all(|required| scopes.contains(required))
        && policy
            .upstream_auth
            .required_roles
            .iter()
            .all(|required| roles.contains(required))
}

fn overloaded(reason: OverloadDecisionReason, retry_after_seconds: u32) -> OverloadDecision {
    match overload_decision(reason, retry_after_seconds) {
        AdmissionPolicyDecision::Overloaded(decision) => decision,
        _ => unreachable!("overload decision helper always returns overloaded"),
    }
}

fn overload_from_route_queue_rejection(
    rejection: RouteQueueRejection,
    retry_after_seconds: u32,
) -> OverloadDecision {
    match overload_decision_for_route_queue_rejection(rejection, retry_after_seconds) {
        AdmissionPolicyDecision::Overloaded(decision) => decision,
        _ => unreachable!("route queue overload helper always returns overloaded"),
    }
}

fn jwt_string_claim_values(claims: &Value, claim_names: &[&str]) -> HashSet<String> {
    let mut values = HashSet::new();
    for claim_name in claim_names {
        let Some(value) = claims.get(*claim_name) else {
            continue;
        };
        match value {
            Value::String(value) => {
                for item in value.split_whitespace() {
                    if !item.is_empty() {
                        values.insert(item.to_string());
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    if let Some(item) = item.as_str()
                        && !item.is_empty()
                    {
                        values.insert(item.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc, time::Duration};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use boring::{
        hash::MessageDigest,
        pkey::{PKey, Private},
        rsa::{Padding, Rsa},
        sign::Signer,
    };
    use bytes::Bytes;
    use http_body_util::Full;
    use spooky_config::{
        config::{
            Backend, Config, ForwardedHeaderPolicy, HealthCheck, JwksStartupBehavior, JwtAlgorithm,
            JwtAuth, Listen, LoadBalancing, Resilience, RouteAuth, RouteMatch, ScopedRateLimit,
            ScopedRateLimitScope, Tls, Upstream, UpstreamHostPolicy,
        },
        runtime::{RuntimeApiKeyAuth, RuntimeAuthPolicy, RuntimeConfig, RuntimeJwtAuth},
    };
    use tokio::sync::Semaphore;

    use super::*;
    use crate::resilience::runtime::RuntimeResilience;

    fn test_policy_with_api_key() -> RuntimeUpstreamPolicy {
        RuntimeUpstreamPolicy {
            upstream_auth: RuntimeAuthPolicy {
                api_key: Some(RuntimeApiKeyAuth {
                    header_name: "x-api-key".to_string(),
                    keys: vec!["secret".to_string()],
                }),
                jwt: None,
                external_auth: None,
                required_scopes: Vec::new(),
                required_roles: Vec::new(),
            },
            host: Default::default(),
            forwarded_headers: Default::default(),
            protocol: Default::default(),
        }
    }

    fn test_policy_with_jwt(
        secret: &str,
        required_scopes: Vec<&str>,
        required_roles: Vec<&str>,
    ) -> RuntimeUpstreamPolicy {
        RuntimeUpstreamPolicy {
            upstream_auth: RuntimeAuthPolicy {
                api_key: None,
                jwt: Some(RuntimeJwtAuth {
                    secret: secret.to_string(),
                    issuer: Some("issuer-1".to_string()),
                    audience: Some("aud-1".to_string()),
                    clock_skew: Duration::from_secs(30),
                    ..RuntimeJwtAuth::default()
                }),
                external_auth: None,
                required_scopes: required_scopes.into_iter().map(str::to_string).collect(),
                required_roles: required_roles.into_iter().map(str::to_string).collect(),
            },
            host: Default::default(),
            forwarded_headers: Default::default(),
            protocol: Default::default(),
        }
    }

    fn test_hs256_jwt(secret: &str, claims: serde_json::Value, alg: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({ "alg": alg, "typ": "JWT" }))
                .expect("serialize header"),
        );
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("serialize claims"));
        let signing_input = format!("{header}.{payload}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("mac");
        mac.update(signing_input.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{signing_input}.{signature}")
    }

    fn test_rs256_jwt(key: &PKey<Private>, kid: &str, claims: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "alg": "RS256",
                "typ": "JWT",
                "kid": kid,
            }))
            .expect("serialize header"),
        );
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("serialize claims"));
        let signing_input = format!("{header}.{payload}");
        let mut signer = Signer::new(MessageDigest::sha256(), key).expect("rsa signer");
        signer.set_rsa_padding(Padding::PKCS1).expect("rsa padding");
        signer
            .update(signing_input.as_bytes())
            .expect("rsa signing input");
        let signature = URL_SAFE_NO_PAD.encode(signer.sign_to_vec().expect("rsa signature"));
        format!("{signing_input}.{signature}")
    }

    fn test_rsa_public_jwk(key: &PKey<Private>, kid: &str) -> Value {
        let rsa = key.rsa().expect("rsa key");
        serde_json::json!({
            "kty": "RSA",
            "kid": kid,
            "alg": "RS256",
            "use": "sig",
            "key_ops": ["verify"],
            "n": URL_SAFE_NO_PAD.encode(rsa.n().to_vec()),
            "e": URL_SAFE_NO_PAD.encode(rsa.e().to_vec()),
        })
    }

    fn test_runtime_resilience(
        configure: impl FnOnce(&mut Resilience),
        global_limit: usize,
    ) -> RuntimeResilience {
        let mut config = Resilience::default();
        configure(&mut config);
        RuntimeResilience::from_config(&config, global_limit)
    }

    fn test_upstream_pool() -> Arc<RwLock<UpstreamPool>> {
        let mut upstreams = HashMap::new();
        upstreams.insert(
            "api".to_string(),
            Upstream {
                load_balancing: LoadBalancing {
                    lb_type: "round-robin".to_string(),
                    key: None,
                },
                auth: RouteAuth::default(),
                host_policy: UpstreamHostPolicy::default(),
                forwarded_headers: ForwardedHeaderPolicy::default(),
                tls: None,
                route: RouteMatch {
                    host: None,
                    path_prefix: Some("/".to_string()),
                    method: None,
                },
                backends: vec![Backend {
                    id: "a".to_string(),
                    address: "http://127.0.0.1:8080".to_string(),
                    weight: 1,
                    health_check: Some(HealthCheck {
                        path: "/health".to_string(),
                        interval: 1,
                        timeout_ms: 1000,
                        failure_threshold: 1,
                        success_threshold: 1,
                        cooldown_ms: 0,
                    }),
                }],
            },
        );

        let runtime = RuntimeConfig::from_config(&Config {
            version: 1,
            listen: Listen {
                protocol: "http1".to_string(),
                tls: Tls {
                    cert: "/tmp/test-cert.pem".to_string(),
                    key: "/tmp/test-key.pem".to_string(),
                    ..Tls::default()
                },
                ..Listen::default()
            },
            listeners: Vec::new(),
            upstream: upstreams,
            load_balancing: None,
            upstream_tls: Default::default(),
            log: Default::default(),
            performance: Default::default(),
            observability: Default::default(),
            resilience: Default::default(),
            security: Default::default(),
        })
        .expect("runtime config");

        Arc::new(RwLock::new(
            UpstreamPool::from_runtime_upstream(runtime.upstreams.get("api").expect("upstream"))
                .expect("pool"),
        ))
    }

    fn test_upstream_inflight() -> HashMap<String, Arc<Semaphore>> {
        HashMap::from([(String::from("api"), Arc::new(Semaphore::new(1)))])
    }

    fn test_runtime_config_with_jwks_auth(jwt: JwtAuth) -> RuntimeConfig {
        RuntimeConfig::from_config(&Config {
            version: 1,
            listen: Listen {
                protocol: "http1".to_string(),
                tls: Tls {
                    cert: "/tmp/test-cert.pem".to_string(),
                    key: "/tmp/test-key.pem".to_string(),
                    ..Tls::default()
                },
                ..Listen::default()
            },
            listeners: Vec::new(),
            upstream: HashMap::from([(
                "api".to_string(),
                Upstream {
                    load_balancing: LoadBalancing {
                        lb_type: "round-robin".to_string(),
                        key: None,
                    },
                    auth: RouteAuth {
                        api_key: None,
                        jwt: Some(jwt),
                        external_auth: None,
                        required_scopes: Vec::new(),
                        required_roles: Vec::new(),
                    },
                    host_policy: UpstreamHostPolicy::default(),
                    forwarded_headers: ForwardedHeaderPolicy::default(),
                    tls: None,
                    route: RouteMatch {
                        host: None,
                        path_prefix: Some("/".to_string()),
                        method: None,
                    },
                    backends: vec![Backend {
                        id: "a".to_string(),
                        address: "http://127.0.0.1:8080".to_string(),
                        weight: 1,
                        health_check: None,
                    }],
                },
            )]),
            load_balancing: None,
            upstream_tls: Default::default(),
            log: Default::default(),
            performance: Default::default(),
            observability: Default::default(),
            resilience: Default::default(),
            security: Default::default(),
        })
        .expect("runtime config")
    }

    mod admission_rejection_contracts {
        use super::*;

        #[test]
        fn overload_reasons_map_to_stable_metrics_labels_and_response_bodies() {
            let cases = [
                (
                    OverloadDecisionReason::Brownout,
                    "brownout",
                    b"brownout active, non-core route shed\n".as_slice(),
                ),
                (
                    OverloadDecisionReason::AdaptiveAdmission,
                    "adaptive_admission",
                    b"adaptive admission overload\n".as_slice(),
                ),
                (
                    OverloadDecisionReason::RouteCap,
                    "route_cap",
                    b"route queue cap exceeded\n".as_slice(),
                ),
                (
                    OverloadDecisionReason::RouteGlobalCap,
                    "route_global_cap",
                    b"global queue cap exceeded\n".as_slice(),
                ),
                (
                    OverloadDecisionReason::GlobalInflight,
                    "global_inflight",
                    b"overloaded, retry later\n".as_slice(),
                ),
                (
                    OverloadDecisionReason::UpstreamInflight,
                    "upstream_inflight",
                    b"upstream overloaded, retry later\n".as_slice(),
                ),
            ];

            for (reason, label, body) in cases {
                assert_eq!(reason.metrics_reason().reason_label(), label);
                assert_eq!(reason.response_body(), body);
            }
        }

        #[test]
        fn unauthorized_and_overload_rejections_have_distinct_outward_mapping() {
            let policy = test_policy_with_api_key();
            let unauthorized =
                admission_rejection_response(&evaluate_forwarding_pre_admission_policy(
                    &policy,
                    None,
                    &BrownoutController::new(false, 100, 50, Vec::new()),
                    0,
                    "api",
                    7,
                    &ScopedRateLimiters::new(&[]),
                    |_| None,
                ))
                .expect("unauthorized response");
            assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);
            assert_eq!(unauthorized.body, b"unauthorized\n");
            assert_eq!(unauthorized.www_authenticate, Some("ApiKey"));
            assert_eq!(unauthorized.retry_after_seconds, None);

            let overload = admission_rejection_response(&evaluate_brownout_policy(
                &BrownoutController::new(true, 50, 25, vec![String::from("core")]),
                90,
                "api",
                7,
            ))
            .expect("overload response");
            assert_eq!(overload.status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(overload.body, b"brownout active, non-core route shed\n");
            assert_eq!(overload.www_authenticate, None);
            assert_eq!(overload.retry_after_seconds, Some(7));
        }
    }

    mod local_auth_policy_contracts {
        use super::*;

        #[test]
        fn local_auth_policy_returns_api_key_challenge_when_key_is_missing() {
            let decision = evaluate_local_auth_policy(&test_policy_with_api_key(), None);

            assert_eq!(
                decision,
                AdmissionPolicyDecision::Unauthorized(UnauthorizedDecision {
                    challenge: AuthChallengeKind::ApiKey,
                    status: StatusCode::UNAUTHORIZED,
                    body: b"unauthorized\n",
                })
            );
        }

        #[test]
        fn local_auth_policy_admits_when_api_key_matches() {
            let headers = HashMap::from([("x-api-key".to_string(), "secret".to_string())]);
            let lookup = |name: &str| headers.get(&name.to_ascii_lowercase()).cloned();

            let decision = evaluate_local_auth_policy(&test_policy_with_api_key(), Some(&lookup));

            assert_eq!(decision, AdmissionPolicyDecision::AdmitReady);
        }

        #[test]
        fn local_auth_policy_returns_bearer_challenge_for_missing_or_invalid_jwt() {
            let policy = test_policy_with_jwt("jwt-secret", vec!["payments:read"], vec!["admin"]);

            let missing = evaluate_local_auth_policy(&policy, None);
            assert_eq!(
                missing,
                AdmissionPolicyDecision::Unauthorized(UnauthorizedDecision {
                    challenge: AuthChallengeKind::Bearer,
                    status: StatusCode::UNAUTHORIZED,
                    body: b"unauthorized\n",
                })
            );

            let invalid_headers = HashMap::from([(
                "authorization".to_string(),
                "Bearer invalid.jwt.token".to_string(),
            )]);
            let invalid_lookup =
                |name: &str| invalid_headers.get(&name.to_ascii_lowercase()).cloned();

            let invalid = evaluate_local_auth_policy(&policy, Some(&invalid_lookup));
            assert_eq!(
                invalid,
                AdmissionPolicyDecision::Unauthorized(UnauthorizedDecision {
                    challenge: AuthChallengeKind::Bearer,
                    status: StatusCode::UNAUTHORIZED,
                    body: b"unauthorized\n",
                })
            );
        }

        #[test]
        fn local_auth_policy_admits_valid_jwt_when_claims_satisfy_rbac() {
            let token = test_hs256_jwt(
                "jwt-secret",
                serde_json::json!({
                    "iss": "issuer-1",
                    "aud": "aud-1",
                    "exp": 4_000_000_000u64,
                    "scope": "payments:read transfers:write",
                    "roles": ["admin", "ops"],
                }),
                "HS256",
            );
            let headers = HashMap::from([("authorization".to_string(), format!("Bearer {token}"))]);
            let lookup = |name: &str| headers.get(&name.to_ascii_lowercase()).cloned();

            let decision = evaluate_local_auth_policy(
                &test_policy_with_jwt("jwt-secret", vec!["payments:read"], vec!["admin"]),
                Some(&lookup),
            );

            assert_eq!(decision, AdmissionPolicyDecision::AdmitReady);
        }
    }

    mod scoped_rate_limit_policy_contracts {
        use super::*;

        fn test_scoped_rate_limits() -> ScopedRateLimiters {
            ScopedRateLimiters::new(&[ScopedRateLimit {
                name: "tenant-cap".to_string(),
                scope: ScopedRateLimitScope::Tenant,
                requests_per_sec: 1,
                burst: 1,
                key: Some("header:x-tenant-id".to_string()),
                route_allowlist: vec!["payments".to_string()],
                idle_ttl_secs: 60,
            }])
        }

        #[test]
        fn scoped_rate_limit_policy_admits_when_rule_does_not_reject() {
            let rate_limits = test_scoped_rate_limits();

            let allowed = evaluate_scoped_rate_limit_policy(&rate_limits, "payments", |_| {
                Some("tenant-a".to_string())
            });
            let no_key = evaluate_scoped_rate_limit_policy(&rate_limits, "payments", |_| None);
            let wrong_route = evaluate_scoped_rate_limit_policy(&rate_limits, "admin", |_| {
                Some("tenant-a".to_string())
            });

            assert_eq!(allowed, AdmissionPolicyDecision::AdmitReady);
            assert_eq!(no_key, AdmissionPolicyDecision::AdmitReady);
            assert_eq!(wrong_route, AdmissionPolicyDecision::AdmitReady);
        }

        #[test]
        fn scoped_rate_limit_policy_returns_typed_rejection_for_exhausted_bucket() {
            let rate_limits = test_scoped_rate_limits();

            let first = evaluate_scoped_rate_limit_policy(&rate_limits, "payments", |_| {
                Some("tenant-a".to_string())
            });
            let second = evaluate_scoped_rate_limit_policy(&rate_limits, "payments", |_| {
                Some("tenant-a".to_string())
            });

            assert_eq!(first, AdmissionPolicyDecision::AdmitReady);
            assert_eq!(
                second,
                AdmissionPolicyDecision::RateLimited(RateLimitedDecision {
                    rule_name: "tenant-cap".to_string(),
                    route: "payments".to_string(),
                    status: StatusCode::TOO_MANY_REQUESTS,
                    body: b"request rate limited\n",
                    retry_after_seconds: 1,
                })
            );
        }
    }

    mod brownout_and_overload_policy_contracts {
        use super::*;

        #[test]
        fn brownout_policy_sheds_non_core_routes_and_preserves_core_routes() {
            let brownout = BrownoutController::new(true, 50, 25, vec![String::from("core")]);

            let core = evaluate_brownout_policy(&brownout, 90, "core", 9);
            let non_core = evaluate_brownout_policy(&brownout, 90, "payments", 9);

            assert_eq!(core, AdmissionPolicyDecision::AdmitReady);
            assert_eq!(
                non_core,
                AdmissionPolicyDecision::Overloaded(OverloadDecision {
                    reason: OverloadDecisionReason::Brownout,
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    body: b"brownout active, non-core route shed\n",
                    retry_after_seconds: 9,
                })
            );
        }

        #[test]
        fn overload_mapping_preserves_route_queue_reasons_and_shared_response_shape() {
            let route_cap =
                overload_decision_for_route_queue_rejection(RouteQueueRejection::RouteCap, 0);
            let global_cap =
                overload_decision_for_route_queue_rejection(RouteQueueRejection::GlobalCap, 3);

            assert_eq!(
                route_cap,
                AdmissionPolicyDecision::Overloaded(OverloadDecision {
                    reason: OverloadDecisionReason::RouteCap,
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    body: b"route queue cap exceeded\n",
                    retry_after_seconds: 1,
                })
            );
            assert_eq!(
                global_cap,
                AdmissionPolicyDecision::Overloaded(OverloadDecision {
                    reason: OverloadDecisionReason::RouteGlobalCap,
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    body: b"global queue cap exceeded\n",
                    retry_after_seconds: 3,
                })
            );

            let mapped = admission_rejection_response(&global_cap).expect("rejection mapping");
            assert_eq!(mapped.status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(mapped.body, b"global queue cap exceeded\n");
            assert_eq!(mapped.www_authenticate, None);
            assert_eq!(mapped.retry_after_seconds, Some(3));
        }
    }

    mod post_auth_admission_execution {
        use super::*;

        fn execute_post_auth_for_api(
            resilience: &RuntimeResilience,
            upstream_pool: Option<&Arc<RwLock<UpstreamPool>>>,
            backend_index: Option<usize>,
            upstream_inflight: &HashMap<String, Arc<Semaphore>>,
            global_inflight: Arc<Semaphore>,
        ) -> PostAuthAdmissionExecution {
            execute_forwarding_post_auth_admission(
                resilience,
                "api",
                upstream_pool,
                backend_index,
                0,
                upstream_inflight,
                global_inflight,
                Duration::ZERO,
            )
        }

        #[test]
        fn post_auth_admission_rejects_adaptive_admission_overload() {
            let resilience = test_runtime_resilience(
                |config| {
                    config.adaptive_admission.enabled = true;
                    config.adaptive_admission.min_limit = 1;
                    config.adaptive_admission.max_limit = Some(1);
                },
                8,
            );
            let _held = resilience
                .adaptive_admission
                .clone()
                .try_acquire()
                .expect("held permit");

            let result = execute_post_auth_for_api(
                &resilience,
                Some(&test_upstream_pool()),
                Some(0),
                &test_upstream_inflight(),
                Arc::new(Semaphore::new(1)),
            );

            assert!(matches!(
                result,
                PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Overloaded(
                    OverloadDecision {
                        reason: OverloadDecisionReason::AdaptiveAdmission,
                        ..
                    }
                ))
            ));
        }

        #[test]
        fn post_auth_admission_rejects_route_cap_and_global_cap_with_distinct_reasons() {
            let route_cap = test_runtime_resilience(
                |config| {
                    config.route_queue.default_cap = 2;
                    config.route_queue.global_cap = 4;
                    config.route_queue.caps.insert(String::from("api"), 1);
                },
                8,
            );
            let _route_held = route_cap
                .route_queue
                .clone()
                .try_acquire("api")
                .expect("route permit");
            let route_result = execute_post_auth_for_api(
                &route_cap,
                Some(&test_upstream_pool()),
                Some(0),
                &test_upstream_inflight(),
                Arc::new(Semaphore::new(1)),
            );
            assert!(matches!(
                route_result,
                PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Overloaded(
                    OverloadDecision {
                        reason: OverloadDecisionReason::RouteCap,
                        ..
                    }
                ))
            ));

            let global_cap = test_runtime_resilience(
                |config| {
                    config.route_queue.default_cap = 4;
                    config.route_queue.global_cap = 1;
                },
                8,
            );
            let _global_route_held = global_cap
                .route_queue
                .clone()
                .try_acquire("other")
                .expect("global route permit");
            let global_result = execute_post_auth_for_api(
                &global_cap,
                Some(&test_upstream_pool()),
                Some(0),
                &test_upstream_inflight(),
                Arc::new(Semaphore::new(1)),
            );
            assert!(matches!(
                global_result,
                PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Overloaded(
                    OverloadDecision {
                        reason: OverloadDecisionReason::RouteGlobalCap,
                        ..
                    }
                ))
            ));
        }

        #[test]
        fn post_auth_admission_rejects_global_and_upstream_inflight_with_distinct_reasons() {
            let resilience = test_runtime_resilience(|_| {}, 8);
            let global_inflight = Arc::new(Semaphore::new(1));
            let _global_held = global_inflight
                .clone()
                .try_acquire_owned()
                .expect("global permit");
            let global_result = execute_post_auth_for_api(
                &resilience,
                Some(&test_upstream_pool()),
                Some(0),
                &test_upstream_inflight(),
                Arc::clone(&global_inflight),
            );
            assert!(matches!(
                global_result,
                PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Overloaded(
                    OverloadDecision {
                        reason: OverloadDecisionReason::GlobalInflight,
                        ..
                    }
                ))
            ));

            let resilience = test_runtime_resilience(|_| {}, 8);
            let upstream_inflight = test_upstream_inflight();
            let _upstream_held = upstream_inflight
                .get("api")
                .expect("api semaphore")
                .clone()
                .try_acquire_owned()
                .expect("upstream permit");
            let upstream_result = execute_post_auth_for_api(
                &resilience,
                Some(&test_upstream_pool()),
                Some(0),
                &upstream_inflight,
                Arc::new(Semaphore::new(1)),
            );
            assert!(matches!(
                upstream_result,
                PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Overloaded(
                    OverloadDecision {
                        reason: OverloadDecisionReason::UpstreamInflight,
                        ..
                    }
                ))
            ));
        }

        #[test]
        fn missing_upstream_limiter_preserves_overload_reason_in_failure_mapping() {
            let resilience = test_runtime_resilience(|_| {}, 8);

            let result = execute_post_auth_for_api(
                &resilience,
                Some(&test_upstream_pool()),
                Some(0),
                &HashMap::new(),
                Arc::new(Semaphore::new(1)),
            );

            assert!(matches!(
                result,
                PostAuthAdmissionExecution::Rejected(PostAuthAdmissionRejection::Failed(
                    PostAuthAdmissionFailure {
                        status: StatusCode::SERVICE_UNAVAILABLE,
                        overload_reason: Some(OverloadDecisionReason::UpstreamInflight),
                        route_outcome: Some(RouteOutcome::OverloadShed),
                        observe_adaptive_overload: true,
                        ..
                    }
                ))
            ));
        }

        #[test]
        fn post_auth_admission_ready_result_reports_no_wait_when_permits_are_immediate() {
            let resilience = test_runtime_resilience(|_| {}, 8);
            let pool = test_upstream_pool();

            let (permit, waited) =
                try_acquire_owned_with_micro_wait(Arc::new(Semaphore::new(1)), Duration::ZERO)
                    .expect("permit");
            assert!(!waited);
            drop(permit);

            let result = execute_post_auth_for_api(
                &resilience,
                Some(&pool),
                Some(0),
                &test_upstream_inflight(),
                Arc::new(Semaphore::new(1)),
            );

            match result {
                PostAuthAdmissionExecution::Ready(ready) => {
                    assert_eq!(ready.backend_index, 0);
                    assert!(!ready.waited_for_global_permit);
                    assert!(!ready.waited_for_upstream_permit);
                }
                PostAuthAdmissionExecution::Rejected(_) => {
                    panic!("expected successful admission without waits")
                }
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn jwks_body_collection_rejects_oversized_documents() {
        let oversized_body = Full::new(Bytes::from(vec![b'x'; MAX_JWKS_BODY_BYTES + 1]));

        let failure = collect_jwks_body_bounded(oversized_body)
            .await
            .expect_err("oversized jwks body must be rejected");

        assert_eq!(failure.reason, JwtJwksFetchFailureReason::MalformedDocument);
        assert!(failure.to_string().contains(&format!(
            "jwks document exceeded {} bytes",
            MAX_JWKS_BODY_BYTES
        )));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn startup_jwks_refresh_populates_active_key_set() {
        let jwks_url = "https://issuer.example.com/startup-jwks.json";
        clear_jwks_cache_for_test(jwks_url);
        let rsa = Rsa::generate(2048).expect("rsa key");
        let key = PKey::from_rsa(rsa).expect("rsa pkey");
        let source = JwtJwksSourceConfig {
            source_identity: jwks_url.to_string(),
            jwks_url: jwks_url.to_string(),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            refresh_interval: Duration::from_secs(60),
            request_timeout: Duration::from_secs(1),
            cache_ttl: Duration::from_secs(60),
            stale_if_error: Duration::from_secs(60),
            startup_behavior: JwksStartupBehavior::RequireReady,
        };
        JwtJwksSharedCache::shared().register_source(source.clone());
        script_jwks_fetches_for_test(
            jwks_url,
            vec![Ok(serde_json::json!({
                "keys": [test_rsa_public_jwk(&key, "startup-key")]
            }))],
        );

        refresh_jwks_source_once(source.clone(), "startup")
            .await
            .expect("startup refresh");

        let snapshot = JwtJwksSharedCache::shared()
            .snapshot(&source.source_identity, Instant::now())
            .expect("cache snapshot");
        assert_eq!(snapshot.state, JwtJwksCacheState::Fresh);
        assert_eq!(snapshot.active_keys.len(), 1);
    }

    #[test]
    fn require_ready_startup_preflight_fails_when_jwks_is_unreachable() {
        let jwks_url = "https://issuer.example.com/require-ready-failure.json";
        clear_jwks_cache_for_test(jwks_url);
        script_jwks_fetches_for_test(
            jwks_url,
            vec![Err(JwtJwksFetchFailure::request_failed(
                "scripted startup outage".to_string(),
            ))],
        );
        let runtime_config = test_runtime_config_with_jwks_auth(JwtAuth {
            secret: String::new(),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            jwks_url: Some(jwks_url.to_string()),
            jwks_request_timeout_ms: 1000,
            jwks_refresh_interval_secs: 60,
            jwks_cache_ttl_secs: 60,
            jwks_stale_if_error_secs: 60,
            jwks_startup_behavior: JwksStartupBehavior::RequireReady,
            ..JwtAuth::default()
        });

        let error =
            QUICListener::initialize_jwks_startup(&runtime_config).expect_err("startup must fail");

        assert!(error.to_string().contains("startup_behavior=require_ready"));
        clear_jwks_cache_for_test(jwks_url);
    }

    #[test]
    fn stale_jwks_cache_beyond_configured_limit_rejects_requests() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let jwks_url = "https://issuer.example.com/expired-stale-jwks.json";
        clear_jwks_cache_for_test(jwks_url);

        let key = PKey::from_rsa(Rsa::generate(2048).expect("rsa")).expect("pkey");
        let source = JwtJwksSourceConfig {
            source_identity: jwks_url.to_string(),
            jwks_url: jwks_url.to_string(),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            refresh_interval: Duration::from_secs(60),
            request_timeout: Duration::from_secs(1),
            cache_ttl: Duration::from_secs(60),
            stale_if_error: Duration::from_secs(60),
            startup_behavior: JwksStartupBehavior::AllowDegraded,
        };
        let cache_now = Instant::now();
        JwtJwksSharedCache::shared().upsert(
            jwks_url,
            JwtJwksCacheEntry {
                source,
                state: JwtJwksCacheState::Stale,
                active_keys: vec![JwtJwksActiveKey {
                    key: RuntimeJwtVerificationKey::Jwk {
                        kid: Some("expired-kid".to_string()),
                        alg: Some(JwtAlgorithm::Rs256),
                        jwk: test_rsa_public_jwk(&key, "expired-kid").to_string(),
                    },
                    retained_until: Some(cache_now - Duration::from_secs(1)),
                }],
                refresh_in_flight: false,
                last_refresh_started_at: Some(cache_now - Duration::from_secs(121)),
                last_refresh_started_wall: Some(SystemTime::now() - Duration::from_secs(121)),
                last_refresh_completed_at: Some(cache_now - Duration::from_secs(121)),
                last_refresh_completed_wall: Some(SystemTime::now() - Duration::from_secs(121)),
                last_success_at: Some(cache_now - Duration::from_secs(121)),
                last_success_wall: Some(SystemTime::now() - Duration::from_secs(121)),
                last_failure_at: Some(cache_now - Duration::from_secs(1)),
                last_failure_wall: Some(SystemTime::now() - Duration::from_secs(1)),
                last_error: Some("request_failed: jwks refresh kept failing".to_string()),
                last_failure_reason: Some(JwtJwksFetchFailureReason::RequestFailed),
                next_on_demand_refresh_at: None,
            },
        );

        let token = test_rs256_jwt(
            &key,
            "expired-kid",
            serde_json::json!({
                "iss": "issuer-1",
                "aud": "aud-1",
                "exp": 4_000_000_000u64,
            }),
        );
        let jwt = RuntimeJwtAuth {
            issuer: Some("issuer-1".to_string()),
            audience: Some("aud-1".to_string()),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            require_kid: true,
            jwks_url: Some(jwks_url.to_string()),
            clock_skew: Duration::from_secs(30),
            ..RuntimeJwtAuth::default()
        };

        let failure = validate_jwt_token(token.as_str(), &jwt, now)
            .expect_err("stale jwks beyond configured limit must reject");
        assert_eq!(
            failure.reason,
            JwtValidationFailureReason::KeySourceUnavailable
        );

        clear_jwks_cache_for_test(jwks_url);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unknown_kid_rejects_current_request_and_triggers_refresh_hint() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let jwks_url = "https://issuer.example.com/on-demand-jwks.json";
        clear_jwks_cache_for_test(jwks_url);

        let first_rsa = Rsa::generate(2048).expect("first rsa");
        let first_key = PKey::from_rsa(first_rsa).expect("first pkey");
        let second_rsa = Rsa::generate(2048).expect("second rsa");
        let second_key = PKey::from_rsa(second_rsa).expect("second pkey");

        prime_jwks_cache_for_test(
            jwks_url,
            false,
            vec![RuntimeJwtVerificationKey::Jwk {
                kid: Some("known-kid".to_string()),
                alg: Some(JwtAlgorithm::Rs256),
                jwk: serde_json::json!({
                    "kty": "RSA",
                    "kid": "known-kid",
                    "alg": "RS256",
                    "use": "sig",
                    "key_ops": ["verify"],
                    "n": URL_SAFE_NO_PAD.encode(first_key.rsa().expect("rsa").n().to_vec()),
                    "e": URL_SAFE_NO_PAD.encode(first_key.rsa().expect("rsa").e().to_vec()),
                })
                .to_string(),
            }],
        );
        script_jwks_fetches_for_test(
            jwks_url,
            vec![Ok(serde_json::json!({
                "keys": [
                    test_rsa_public_jwk(&first_key, "known-kid"),
                    test_rsa_public_jwk(&second_key, "rotated-kid"),
                ]
            }))],
        );

        let jwt = RuntimeJwtAuth {
            issuer: Some("issuer-1".to_string()),
            audience: Some("aud-1".to_string()),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            require_kid: true,
            jwks_url: Some(jwks_url.to_string()),
            jwks_refresh_interval: Duration::from_secs(60),
            jwks_request_timeout: Duration::from_secs(1),
            jwks_cache_ttl: Duration::from_secs(60),
            jwks_stale_if_error: Duration::from_secs(60),
            clock_skew: Duration::from_secs(30),
            ..RuntimeJwtAuth::default()
        };
        let token = test_rs256_jwt(
            &second_key,
            "rotated-kid",
            serde_json::json!({
                "iss": "issuer-1",
                "aud": "aud-1",
                "exp": 4_000_000_000u64,
            }),
        );

        let failure = validate_jwt_token(token.as_str(), &jwt, now)
            .expect_err("current request must reject unknown kid");
        assert_eq!(
            failure.reason,
            JwtValidationFailureReason::MissingVerificationKey
        );

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if validate_jwt_token(token.as_str(), &jwt, now).is_ok() {
                clear_jwks_cache_for_test(jwks_url);
                return;
            }
        }

        panic!("on-demand jwks refresh did not publish the rotated kid");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refresh_retains_temporarily_dropped_old_key_during_rollover_overlap() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let jwks_url = "https://issuer.example.com/rollover-overlap-jwks.json";
        clear_jwks_cache_for_test(jwks_url);

        let old_key = PKey::from_rsa(Rsa::generate(2048).expect("old rsa")).expect("old pkey");
        let new_key = PKey::from_rsa(Rsa::generate(2048).expect("new rsa")).expect("new pkey");
        let source = JwtJwksSourceConfig {
            source_identity: jwks_url.to_string(),
            jwks_url: jwks_url.to_string(),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            refresh_interval: Duration::from_secs(60),
            request_timeout: Duration::from_secs(1),
            cache_ttl: Duration::from_secs(60),
            stale_if_error: Duration::from_secs(60),
            startup_behavior: JwksStartupBehavior::AllowDegraded,
        };
        JwtJwksSharedCache::shared().register_source(source.clone());
        script_jwks_fetches_for_test(
            jwks_url,
            vec![
                Ok(serde_json::json!({ "keys": [test_rsa_public_jwk(&old_key, "old-kid")] })),
                Ok(serde_json::json!({ "keys": [test_rsa_public_jwk(&new_key, "new-kid")] })),
            ],
        );

        refresh_jwks_source_once(source.clone(), "startup")
            .await
            .expect("initial refresh");
        refresh_jwks_source_once(source.clone(), "periodic")
            .await
            .expect("rollover refresh");

        let jwt = RuntimeJwtAuth {
            issuer: Some("issuer-1".to_string()),
            audience: Some("aud-1".to_string()),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            require_kid: true,
            jwks_url: Some(jwks_url.to_string()),
            clock_skew: Duration::from_secs(30),
            ..RuntimeJwtAuth::default()
        };
        let old_token = test_rs256_jwt(
            &old_key,
            "old-kid",
            serde_json::json!({ "iss": "issuer-1", "aud": "aud-1", "exp": 4_000_000_000u64 }),
        );
        let new_token = test_rs256_jwt(
            &new_key,
            "new-kid",
            serde_json::json!({ "iss": "issuer-1", "aud": "aud-1", "exp": 4_000_000_000u64 }),
        );

        assert!(validate_jwt_token(old_token.as_str(), &jwt, now).is_ok());
        assert!(validate_jwt_token(new_token.as_str(), &jwt, now).is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refresh_replaces_key_material_when_issuer_reuses_existing_kid() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let jwks_url = "https://issuer.example.com/reused-kid-jwks.json";
        clear_jwks_cache_for_test(jwks_url);

        let old_key = PKey::from_rsa(Rsa::generate(2048).expect("old rsa")).expect("old pkey");
        let new_key = PKey::from_rsa(Rsa::generate(2048).expect("new rsa")).expect("new pkey");
        let source = JwtJwksSourceConfig {
            source_identity: jwks_url.to_string(),
            jwks_url: jwks_url.to_string(),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            refresh_interval: Duration::from_secs(60),
            request_timeout: Duration::from_secs(1),
            cache_ttl: Duration::from_secs(60),
            stale_if_error: Duration::from_secs(60),
            startup_behavior: JwksStartupBehavior::AllowDegraded,
        };
        JwtJwksSharedCache::shared().register_source(source.clone());
        script_jwks_fetches_for_test(
            jwks_url,
            vec![
                Ok(serde_json::json!({ "keys": [test_rsa_public_jwk(&old_key, "shared-kid")] })),
                Ok(serde_json::json!({ "keys": [test_rsa_public_jwk(&new_key, "shared-kid")] })),
            ],
        );

        refresh_jwks_source_once(source.clone(), "startup")
            .await
            .expect("initial refresh");
        refresh_jwks_source_once(source.clone(), "periodic")
            .await
            .expect("replacement refresh");

        let jwt = RuntimeJwtAuth {
            issuer: Some("issuer-1".to_string()),
            audience: Some("aud-1".to_string()),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            require_kid: true,
            jwks_url: Some(jwks_url.to_string()),
            clock_skew: Duration::from_secs(30),
            ..RuntimeJwtAuth::default()
        };
        let old_token = test_rs256_jwt(
            &old_key,
            "shared-kid",
            serde_json::json!({ "iss": "issuer-1", "aud": "aud-1", "exp": 4_000_000_000u64 }),
        );
        let new_token = test_rs256_jwt(
            &new_key,
            "shared-kid",
            serde_json::json!({ "iss": "issuer-1", "aud": "aud-1", "exp": 4_000_000_000u64 }),
        );

        let old_failure = validate_jwt_token(old_token.as_str(), &jwt, now)
            .expect_err("old material under reused kid must stop validating");
        assert_eq!(
            old_failure.reason,
            JwtValidationFailureReason::SignatureInvalid
        );
        assert!(validate_jwt_token(new_token.as_str(), &jwt, now).is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_or_broken_refresh_quarantines_replacement_and_retains_last_known_good_keys() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let jwks_url = "https://issuer.example.com/quarantine-jwks.json";
        clear_jwks_cache_for_test(jwks_url);

        let key = PKey::from_rsa(Rsa::generate(2048).expect("rsa")).expect("pkey");
        let source = JwtJwksSourceConfig {
            source_identity: jwks_url.to_string(),
            jwks_url: jwks_url.to_string(),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            refresh_interval: Duration::from_secs(60),
            request_timeout: Duration::from_secs(1),
            cache_ttl: Duration::from_secs(60),
            stale_if_error: Duration::from_secs(60),
            startup_behavior: JwksStartupBehavior::AllowDegraded,
        };
        JwtJwksSharedCache::shared().register_source(source.clone());
        script_jwks_fetches_for_test(
            jwks_url,
            vec![
                Ok(serde_json::json!({ "keys": [test_rsa_public_jwk(&key, "stable-kid")] })),
                Ok(serde_json::json!({ "keys": [] })),
                Err(JwtJwksFetchFailure::malformed_document(
                    "duplicate or broken jwks",
                )),
            ],
        );

        refresh_jwks_source_once(source.clone(), "startup")
            .await
            .expect("initial refresh");
        refresh_jwks_source_once(source.clone(), "periodic")
            .await
            .expect("empty replacement refresh");
        let after_empty = JwtJwksSharedCache::shared()
            .snapshot(&source.source_identity, Instant::now())
            .expect("snapshot after empty");
        assert_eq!(after_empty.state, JwtJwksCacheState::QuarantinedRetained);
        assert_eq!(after_empty.active_keys.len(), 1);

        let _ = refresh_jwks_source_once(source.clone(), "periodic").await;
        let after_broken = JwtJwksSharedCache::shared()
            .snapshot(&source.source_identity, Instant::now())
            .expect("snapshot after broken");
        assert!(matches!(
            after_broken.state,
            JwtJwksCacheState::QuarantinedRetained | JwtJwksCacheState::RefreshFailedRetained
        ));

        let jwt = RuntimeJwtAuth {
            issuer: Some("issuer-1".to_string()),
            audience: Some("aud-1".to_string()),
            allowed_algorithms: vec![JwtAlgorithm::Rs256],
            require_kid: true,
            jwks_url: Some(jwks_url.to_string()),
            clock_skew: Duration::from_secs(30),
            ..RuntimeJwtAuth::default()
        };
        let token = test_rs256_jwt(
            &key,
            "stable-kid",
            serde_json::json!({ "iss": "issuer-1", "aud": "aud-1", "exp": 4_000_000_000u64 }),
        );
        assert!(validate_jwt_token(token.as_str(), &jwt, now).is_ok());
    }
}
