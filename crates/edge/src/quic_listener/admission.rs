use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
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
use hmac::{Hmac, Mac};
use http::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
use spooky_config::config::JwtAlgorithm;
use spooky_config::runtime::{RuntimeJwtAuth, RuntimeJwtVerificationKey, RuntimeUpstreamPolicy};
use spooky_lb::upstream_pool::UpstreamPool;
use subtle::ConstantTimeEq;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

use super::{LbHeaderLookup, QUICListener};
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
            log::debug!(
                "JWT validation rejected request: reason={}",
                failure.reason.as_str()
            );
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

pub(super) fn validate_jwt_token(
    token: &str,
    jwt: &RuntimeJwtAuth,
    now: SystemTime,
) -> Result<ValidatedJwt, JwtValidationFailure> {
    let parsed = parse_compact_jwt(token)?;
    let header = parse_jose_header(&parsed.header_bytes)?;
    let algorithm = validate_jwt_algorithm_policy(jwt, header.algorithm)?;
    let key = resolve_jwt_verification_key(jwt, algorithm, header.kid.as_deref())?;
    verify_jwt_signature(&parsed, algorithm, key)?;
    let claims = parse_jwt_claims(&parsed.payload_bytes)?;
    validate_jwt_registered_claims(jwt, &claims, now)?;

    Ok(ValidatedJwt { algorithm, claims })
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

fn resolve_jwt_verification_key<'a>(
    jwt: &'a RuntimeJwtAuth,
    algorithm: JwtAlgorithm,
    kid: Option<&str>,
) -> Result<JwtVerificationKey<'a>, JwtValidationFailure> {
    if jwt.require_kid && kid.is_none() {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::MissingKid,
        ));
    }

    match algorithm {
        JwtAlgorithm::Hs256 => {
            if jwt.secret.is_empty() {
                return Err(JwtValidationFailure::new(
                    JwtValidationFailureReason::MissingVerificationKey,
                ));
            }
            Ok(JwtVerificationKey::Hs256Secret(jwt.secret.as_str()))
        }
        JwtAlgorithm::Rs256 | JwtAlgorithm::Es256 => {
            resolve_static_asymmetric_key(jwt, algorithm, kid)
        }
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

fn resolve_static_asymmetric_key(
    jwt: &RuntimeJwtAuth,
    algorithm: JwtAlgorithm,
    requested_kid: Option<&str>,
) -> Result<JwtVerificationKey<'static>, JwtValidationFailure> {
    let mut candidates = Vec::new();
    for key in &jwt.static_keys {
        let metadata = static_key_metadata(key)?;
        let effective_kid = metadata
            .kid
            .as_deref()
            .or_else(|| static_key_config_kid(key));
        if let Some(requested_kid) = requested_kid
            && effective_kid != Some(requested_kid)
        {
            continue;
        }
        if let Some(key_alg) = metadata.alg.or_else(|| static_key_config_alg(key))
            && key_alg != algorithm
        {
            continue;
        }
        candidates.push((key, metadata));
    }

    if candidates.is_empty() {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::MissingVerificationKey,
        ));
    }
    // Reject ambiguity even when the token carries a `kid`: two static keys can
    // resolve to the same effective kid when it is declared inside a JWK body
    // rather than the config field, which config validation cannot catch.
    if candidates.len() > 1 {
        return Err(JwtValidationFailure::new(
            JwtValidationFailureReason::AmbiguousVerificationKey,
        ));
    }

    parse_static_verification_key(candidates[0].0, algorithm)
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
    let expected = if let Some(issuer) = jwt.issuer.as_deref() {
        vec![issuer]
    } else {
        jwt.issuers.iter().map(String::as_str).collect()
    };
    if expected.is_empty() {
        return true;
    }
    let actual = claims.get("iss").and_then(Value::as_str);
    expected.into_iter().any(|issuer| actual == Some(issuer))
}

fn jwt_audience_matches(jwt: &RuntimeJwtAuth, claims: &Value) -> bool {
    let expected = if let Some(audience) = jwt.audience.as_deref() {
        vec![audience]
    } else {
        jwt.audiences.iter().map(String::as_str).collect()
    };
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

    use spooky_config::{
        config::{
            Backend, Config, ForwardedHeaderPolicy, HealthCheck, Listen, LoadBalancing, Resilience,
            RouteAuth, RouteMatch, ScopedRateLimit, ScopedRateLimitScope, Tls, Upstream,
            UpstreamHostPolicy,
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
}
