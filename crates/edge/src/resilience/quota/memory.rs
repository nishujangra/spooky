use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use super::{
    DistributedQuotaCounterBackend, QuotaCounterBackendError, QuotaCounterBackendErrorKind,
    QuotaCounterBackendMetadata, QuotaCounterEvalFuture, QuotaCounterEvaluationDecision,
    QuotaCounterEvaluationOutcome, QuotaCounterEvaluationRequest, QuotaCounterResult,
    QuotaDenyReason, QuotaWindowPolicy, QuotaWindowUsage,
};

pub const IN_MEMORY_QUOTA_PROTOCOL_VERSION: &str = "memory-fixed-window/v1";

const IN_MEMORY_BACKEND_KIND: &str = "in_memory";
const IN_MEMORY_KEY_PROTOCOL_TAG: &str = "qmem1";
const IN_MEMORY_KEY_TTL_GRACE_MS: u64 = 1_000;
pub(crate) const DEFAULT_IN_MEMORY_QUOTA_MAX_ENTRIES: usize = 4_096;
type TimeSource = dyn Fn() -> u64 + Send + Sync;

#[derive(Debug, Clone)]
struct InMemoryBucketState {
    consumed: u64,
    expires_at_unix_ms: u64,
}

#[derive(Debug, Clone)]
struct InMemoryQuotaWindowSpec {
    kind: &'static str,
    limit: u64,
    window_ms: u64,
    bucket_started_at_unix_ms: u64,
    reset_at_unix_ms: u64,
    storage_key: String,
    ttl_ms: u64,
}

#[derive(Debug, Clone)]
struct EvaluatedWindow {
    spec: InMemoryQuotaWindowSpec,
    current: u64,
    projected: u64,
}

pub struct InMemoryDistributedQuotaCounterStore {
    key_prefix: String,
    max_entries: Option<usize>,
    buckets: Mutex<HashMap<String, InMemoryBucketState>>,
    time_source: Arc<TimeSource>,
}

impl InMemoryDistributedQuotaCounterStore {
    pub fn new(key_prefix: &str) -> Self {
        Self::with_limits_and_time_source(key_prefix, None, unix_now_ms)
    }

    pub fn bounded(key_prefix: &str, max_entries: usize) -> Self {
        Self::with_limits_and_time_source(key_prefix, Some(max_entries.max(1)), unix_now_ms)
    }

    pub fn protocol_version(&self) -> &'static str {
        IN_MEMORY_QUOTA_PROTOCOL_VERSION
    }

    pub fn key_prefix(&self) -> &str {
        &self.key_prefix
    }

    #[cfg(test)]
    fn with_time_source<F>(key_prefix: &str, time_source: F) -> Self
    where
        F: Fn() -> u64 + Send + Sync + 'static,
    {
        Self::with_limits_and_time_source(key_prefix, None, time_source)
    }

    fn with_limits_and_time_source<F>(
        key_prefix: &str,
        max_entries: Option<usize>,
        time_source: F,
    ) -> Self
    where
        F: Fn() -> u64 + Send + Sync + 'static,
    {
        Self {
            key_prefix: key_prefix.trim().to_string(),
            max_entries,
            buckets: Mutex::new(HashMap::new()),
            time_source: Arc::new(time_source),
        }
    }

    fn evaluate_request(
        &self,
        request: QuotaCounterEvaluationRequest,
    ) -> Result<QuotaCounterEvaluationOutcome, QuotaCounterBackendError> {
        let now_ms = (self.time_source)();
        let windows = build_window_specs(&self.key_prefix, &request, now_ms);
        if windows.is_empty() {
            return Err(QuotaCounterBackendError {
                policy_name: Some(request.policy_name.clone()),
                composite_key: Some(request.composite_key.key.clone()),
                kind: QuotaCounterBackendErrorKind::Error,
                detail: Some(
                    "in-memory quota evaluation requires at least one configured window"
                        .to_string(),
                ),
            });
        }

        let mut buckets = self.buckets.lock().map_err(|_| QuotaCounterBackendError {
            policy_name: Some(request.policy_name.clone()),
            composite_key: Some(request.composite_key.key.clone()),
            kind: QuotaCounterBackendErrorKind::Unavailable,
            detail: Some("in-memory quota store lock is poisoned".to_string()),
        })?;
        buckets.retain(|_, state| state.expires_at_unix_ms > now_ms);

        let mut evaluated = Vec::with_capacity(windows.len());
        let mut deny_reason = None;

        for window in windows {
            let current = buckets
                .get(&window.storage_key)
                .map(|state| state.consumed)
                .unwrap_or(0);
            let projected = current.saturating_add(request.cost);
            if projected > window.limit && deny_reason.is_none() {
                deny_reason = Some(match window.kind {
                    "burst" => QuotaDenyReason::BurstQuotaExhausted,
                    _ => QuotaDenyReason::SustainedQuotaExhausted,
                });
            }
            evaluated.push(EvaluatedWindow {
                spec: window,
                current,
                projected,
            });
        }

        let allowed = deny_reason.is_none();
        if allowed {
            let additional_entries = evaluated
                .iter()
                .filter(|window| !buckets.contains_key(&window.spec.storage_key))
                .count();
            if let Some(max_entries) = self.max_entries
                && buckets.len().saturating_add(additional_entries) > max_entries
            {
                return Err(QuotaCounterBackendError {
                    policy_name: Some(request.policy_name.clone()),
                    composite_key: Some(request.composite_key.key.clone()),
                    kind: QuotaCounterBackendErrorKind::Unavailable,
                    detail: Some("in-memory quota store capacity exhausted".to_string()),
                });
            }
            for window in &evaluated {
                buckets.insert(
                    window.spec.storage_key.clone(),
                    InMemoryBucketState {
                        consumed: window.projected,
                        expires_at_unix_ms: now_ms.saturating_add(window.spec.ttl_ms),
                    },
                );
            }
        }

        let mut burst = None;
        let mut sustained = None;

        for window in evaluated {
            let consumed = if allowed {
                window.projected
            } else {
                window.current
            };
            let usage = QuotaWindowUsage {
                limit: window.spec.limit,
                consumed,
                remaining: window.spec.limit.saturating_sub(consumed),
                window: Duration::from_millis(window.spec.window_ms),
                reset_after: Some(Duration::from_millis(
                    window.spec.reset_at_unix_ms.saturating_sub(now_ms),
                )),
                bucket_started_at_unix_ms: Some(window.spec.bucket_started_at_unix_ms),
                reset_at_unix_ms: Some(window.spec.reset_at_unix_ms),
                storage_key: Some(window.spec.storage_key),
            };

            match window.spec.kind {
                "burst" => burst = Some(usage),
                "sustained" => sustained = Some(usage),
                _ => {
                    return Err(QuotaCounterBackendError {
                        policy_name: Some(request.policy_name.clone()),
                        composite_key: Some(request.composite_key.key.clone()),
                        kind: QuotaCounterBackendErrorKind::Error,
                        detail: Some("unknown in-memory quota window kind".to_string()),
                    });
                }
            }
        }

        Ok(QuotaCounterEvaluationOutcome {
            matched_policy: request.policy_name,
            composite_key: request.composite_key,
            decision: deny_reason
                .map(QuotaCounterEvaluationDecision::Denied)
                .unwrap_or(QuotaCounterEvaluationDecision::Allowed),
            counter: QuotaCounterResult { burst, sustained },
            backend_metadata: QuotaCounterBackendMetadata {
                backend_kind: IN_MEMORY_BACKEND_KIND.to_string(),
                protocol_version: IN_MEMORY_QUOTA_PROTOCOL_VERSION.to_string(),
                evaluated_at_unix_ms: Some(now_ms),
            },
        })
    }
}

impl DistributedQuotaCounterBackend for InMemoryDistributedQuotaCounterStore {
    fn evaluate<'a>(
        &'a self,
        request: QuotaCounterEvaluationRequest,
    ) -> QuotaCounterEvalFuture<'a> {
        Box::pin(async move { self.evaluate_request(request) })
    }
}

fn build_window_specs(
    key_prefix: &str,
    request: &QuotaCounterEvaluationRequest,
    now_ms: u64,
) -> Vec<InMemoryQuotaWindowSpec> {
    let mut windows = Vec::new();

    if let Some(window) = request.burst.as_ref() {
        windows.push(build_window_spec(
            key_prefix, request, "burst", window, now_ms,
        ));
    }
    if let Some(window) = request.sustained.as_ref() {
        windows.push(build_window_spec(
            key_prefix,
            request,
            "sustained",
            window,
            now_ms,
        ));
    }

    windows
}

fn build_window_spec(
    key_prefix: &str,
    request: &QuotaCounterEvaluationRequest,
    kind: &'static str,
    window: &QuotaWindowPolicy,
    now_ms: u64,
) -> InMemoryQuotaWindowSpec {
    let window_ms = window.window.as_millis().max(1) as u64;
    let bucket_started_at_unix_ms = now_ms - (now_ms % window_ms);
    let reset_at_unix_ms = bucket_started_at_unix_ms.saturating_add(window_ms);
    let ttl_ms = reset_at_unix_ms
        .saturating_sub(now_ms)
        .max(1)
        .saturating_add(IN_MEMORY_KEY_TTL_GRACE_MS);
    let digest = composite_key_digest(&request.composite_key.key);

    InMemoryQuotaWindowSpec {
        kind,
        limit: window.requests,
        window_ms,
        bucket_started_at_unix_ms,
        reset_at_unix_ms,
        storage_key: format!(
            "{}:{}:{}:{}:{}:{}:{}",
            key_prefix,
            IN_MEMORY_KEY_PROTOCOL_TAG,
            encode_key_component(&request.policy_name),
            kind,
            window_ms,
            bucket_started_at_unix_ms,
            digest
        ),
        ttl_ms,
    }
}

fn composite_key_digest(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn encode_key_component(value: &str) -> String {
    format!("{}:{}", value.len(), value)
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    use super::*;
    use crate::resilience::quota::{
        QuotaCompositeKey, QuotaIdentityLabels, QuotaPolicyRuntime, QuotaSelectorDimensions,
        QuotaSelectorMatcher,
    };

    fn sample_request() -> QuotaCounterEvaluationRequest {
        let policy = QuotaPolicyRuntime {
            name: "tenant-quota".to_string(),
            route_allowlist: HashSet::from(["api".to_string()]),
            selector: QuotaSelectorMatcher {
                route: true,
                tenant: None,
                token: None,
                client: None,
            },
            burst: Some(QuotaWindowPolicy {
                requests: 2,
                window: Duration::from_secs(1),
            }),
            sustained: Some(QuotaWindowPolicy {
                requests: 5,
                window: Duration::from_secs(60),
            }),
        };
        let composite_key = QuotaCompositeKey {
            policy_name: "tenant-quota".to_string(),
            key: "policy=12:tenant-quota|route=3:api|tenant=4:acme|".to_string(),
            labels: QuotaIdentityLabels {
                route: Some("api".to_string()),
                tenant: Some("acme".to_string()),
                token: None,
                client: None,
            },
            dimensions: QuotaSelectorDimensions {
                route: true,
                tenant: true,
                token: false,
                client: false,
            },
        };

        policy.counter_request(composite_key)
    }

    #[test]
    fn in_memory_backend_supports_atomic_multi_window_evaluation() {
        let now_ms = Arc::new(AtomicU64::new(10_250));
        let store = InMemoryDistributedQuotaCounterStore::with_time_source("impulse:quota", {
            let now_ms = Arc::clone(&now_ms);
            move || now_ms.load(Ordering::Relaxed)
        });

        let first = store
            .evaluate_request(sample_request())
            .expect("first request should succeed");
        assert_eq!(first.decision, QuotaCounterEvaluationDecision::Allowed);
        assert_eq!(
            first.counter.burst.as_ref().map(|window| window.consumed),
            Some(1)
        );
        assert_eq!(
            first
                .counter
                .sustained
                .as_ref()
                .map(|window| window.consumed),
            Some(1)
        );

        let second = store
            .evaluate_request(sample_request())
            .expect("second request should succeed");
        assert_eq!(second.decision, QuotaCounterEvaluationDecision::Allowed);
        assert_eq!(
            second.counter.burst.as_ref().map(|window| window.consumed),
            Some(2)
        );

        let denied = store
            .evaluate_request(sample_request())
            .expect("third request should return a decision");
        assert_eq!(
            denied.decision,
            QuotaCounterEvaluationDecision::Denied(QuotaDenyReason::BurstQuotaExhausted)
        );
        assert_eq!(
            denied.counter.burst.as_ref().map(|window| window.consumed),
            Some(2)
        );
        assert_eq!(
            denied
                .counter
                .sustained
                .as_ref()
                .map(|window| window.consumed),
            Some(2),
            "denied evaluations must not partially increment sustained state"
        );

        now_ms.store(11_250, Ordering::Relaxed);
        let after_burst_reset = store
            .evaluate_request(sample_request())
            .expect("new burst bucket should allow request");
        assert_eq!(
            after_burst_reset.decision,
            QuotaCounterEvaluationDecision::Allowed
        );
        assert_eq!(
            after_burst_reset
                .counter
                .burst
                .as_ref()
                .map(|window| window.consumed),
            Some(1)
        );
        assert_eq!(
            after_burst_reset
                .counter
                .sustained
                .as_ref()
                .map(|window| window.consumed),
            Some(3)
        );
        assert_eq!(
            after_burst_reset.backend_metadata.protocol_version,
            IN_MEMORY_QUOTA_PROTOCOL_VERSION
        );
    }

    #[test]
    fn in_memory_backend_uses_explicit_local_protocol_keys() {
        let request = sample_request();
        let windows = build_window_specs("impulse:quota", &request, 10_250);

        assert_eq!(windows.len(), 2);
        assert!(windows[0].storage_key.starts_with("impulse:quota:qmem1:"));
        assert!(windows[0].storage_key.contains(":burst:1000:10000:"));
        assert!(windows[1].storage_key.contains(":sustained:60000:0:"));
        assert_eq!(windows[0].ttl_ms, 1_750);
    }

    #[test]
    fn bounded_in_memory_backend_rejects_capacity_exhaustion() {
        let store = InMemoryDistributedQuotaCounterStore::bounded("impulse:quota:fallback", 2);

        let first = store
            .evaluate_request(sample_request())
            .expect("first composite key should fit within bounded fallback");
        assert_eq!(first.decision, QuotaCounterEvaluationDecision::Allowed);

        let err = store
            .evaluate_request(QuotaCounterEvaluationRequest {
                composite_key: QuotaCompositeKey {
                    policy_name: "tenant-quota".to_string(),
                    key: "policy=12:tenant-quota|route=7:private|tenant=4:beta|".to_string(),
                    labels: QuotaIdentityLabels {
                        route: Some("private".to_string()),
                        tenant: Some("beta".to_string()),
                        token: None,
                        client: None,
                    },
                    dimensions: QuotaSelectorDimensions {
                        route: true,
                        tenant: true,
                        token: false,
                        client: false,
                    },
                },
                ..sample_request()
            })
            .expect_err("bounded fallback must reject new keys beyond configured capacity");

        assert_eq!(err.kind, QuotaCounterBackendErrorKind::Unavailable);
        assert_eq!(
            err.detail.as_deref(),
            Some("in-memory quota store capacity exhausted")
        );
    }

    #[test]
    fn in_memory_backend_enforces_sustained_window_after_burst_resets() {
        let now_ms = Arc::new(AtomicU64::new(10_250));
        let store =
            InMemoryDistributedQuotaCounterStore::with_time_source("impulse:quota:sustained", {
                let now_ms = Arc::clone(&now_ms);
                move || now_ms.load(Ordering::Relaxed)
            });

        let policy = QuotaPolicyRuntime {
            name: "tenant-quota".to_string(),
            route_allowlist: HashSet::from(["api".to_string()]),
            selector: QuotaSelectorMatcher {
                route: true,
                tenant: None,
                token: None,
                client: None,
            },
            burst: Some(QuotaWindowPolicy {
                requests: 2,
                window: Duration::from_secs(1),
            }),
            sustained: Some(QuotaWindowPolicy {
                requests: 3,
                window: Duration::from_secs(60),
            }),
        };
        let composite_key = QuotaCompositeKey {
            policy_name: "tenant-quota".to_string(),
            key: "policy=12:tenant-quota|route=3:api|tenant=4:acme|".to_string(),
            labels: QuotaIdentityLabels {
                route: Some("api".to_string()),
                tenant: Some("acme".to_string()),
                token: None,
                client: None,
            },
            dimensions: QuotaSelectorDimensions {
                route: true,
                tenant: true,
                token: false,
                client: false,
            },
        };
        let request = policy.counter_request(composite_key);

        assert_eq!(
            store
                .evaluate_request(request.clone())
                .expect("first request")
                .decision,
            QuotaCounterEvaluationDecision::Allowed
        );
        assert_eq!(
            store
                .evaluate_request(request.clone())
                .expect("second request")
                .decision,
            QuotaCounterEvaluationDecision::Allowed
        );
        assert_eq!(
            store
                .evaluate_request(request.clone())
                .expect("third request must burst deny")
                .decision,
            QuotaCounterEvaluationDecision::Denied(QuotaDenyReason::BurstQuotaExhausted)
        );

        now_ms.store(11_250, Ordering::Relaxed);
        assert_eq!(
            store
                .evaluate_request(request.clone())
                .expect("fourth request after burst reset")
                .decision,
            QuotaCounterEvaluationDecision::Allowed
        );

        now_ms.store(12_250, Ordering::Relaxed);
        let denied = store
            .evaluate_request(request)
            .expect("sustained exhaustion should return a decision");
        assert_eq!(
            denied.decision,
            QuotaCounterEvaluationDecision::Denied(QuotaDenyReason::SustainedQuotaExhausted)
        );
        assert_eq!(
            denied
                .counter
                .sustained
                .as_ref()
                .map(|window| window.consumed),
            Some(3)
        );
    }
}
