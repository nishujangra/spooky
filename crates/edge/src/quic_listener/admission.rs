use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use http::StatusCode;
use serde_json::Value;
use sha2::Sha256;
use spooky_config::runtime::{RuntimeJwtAuth, RuntimeUpstreamPolicy};
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
    let Some(claims) = validated_hs256_jwt_claims(token.as_str(), jwt, SystemTime::now()) else {
        return false;
    };
    jwt_claims_satisfy_rbac(policy, &claims)
}

pub(super) fn validated_hs256_jwt_claims(
    token: &str,
    jwt: &RuntimeJwtAuth,
    now: SystemTime,
) -> Option<Value> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(signature_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let Ok(header_bytes) = URL_SAFE_NO_PAD.decode(header_b64) else {
        return None;
    };
    let Ok(payload_bytes) = URL_SAFE_NO_PAD.decode(payload_b64) else {
        return None;
    };
    let Ok(signature) = URL_SAFE_NO_PAD.decode(signature_b64) else {
        return None;
    };
    let Ok(header) = serde_json::from_slice::<Value>(&header_bytes) else {
        return None;
    };
    if header.get("alg").and_then(Value::as_str) != Some("HS256") {
        return None;
    }

    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(jwt.secret.as_bytes()) else {
        return None;
    };
    mac.update(format!("{header_b64}.{payload_b64}").as_bytes());
    let expected = mac.finalize().into_bytes();
    if expected.len() != signature.len()
        || !bool::from(expected.as_slice().ct_eq(signature.as_slice()))
    {
        return None;
    }

    let Ok(claims) = serde_json::from_slice::<Value>(&payload_bytes) else {
        return None;
    };
    let Ok(now_secs) = now.duration_since(UNIX_EPOCH).map(|value| value.as_secs()) else {
        return None;
    };
    let clock_skew_secs = jwt.clock_skew.as_secs();
    let exp = claims.get("exp").and_then(Value::as_u64)?;
    if now_secs > exp.saturating_add(clock_skew_secs) {
        return None;
    }
    if claims
        .get("nbf")
        .and_then(Value::as_u64)
        .is_some_and(|nbf| now_secs.saturating_add(clock_skew_secs) < nbf)
    {
        return None;
    }
    if claims
        .get("iat")
        .and_then(Value::as_u64)
        .is_some_and(|iat| now_secs.saturating_add(clock_skew_secs) < iat)
    {
        return None;
    }
    if jwt
        .issuer
        .as_deref()
        .is_some_and(|issuer| claims.get("iss").and_then(Value::as_str) != Some(issuer))
    {
        return None;
    }
    if let Some(audience) = jwt.audience.as_deref() {
        let claim_aud = claims.get("aud")?;
        match claim_aud {
            Value::String(value) if value == audience => {}
            Value::Array(values)
                if values
                    .iter()
                    .any(|value| value.as_str().is_some_and(|value| value == audience)) => {}
            _ => return None,
        }
    }

    Some(claims)
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
