//! Canonical transport execution façade.
//!
//! This module owns runtime-selected backend protocol dispatch, transport-level
//! timeout application, connection reuse, and backend client rotation. Callers
//! should hand it a backend identity plus a canonical request and avoid
//! reconstructing H1/H2 selection logic themselves.

use std::{collections::HashMap, convert::Infallible, time::Duration};

use http_body_util::combinators::BoxBody;
use hyper::{
    Request,
    body::{Bytes, Incoming},
};
use spooky_config::runtime::{
    RuntimeBackendConnectionPolicy, RuntimeBackendTransportKind, RuntimeUpstream,
};
use spooky_errors::{PoolError, ProxyError};

use crate::{
    client_rotation::BackendClientRotation,
    h1_pool::H1Pool,
    h2_client::{ConnectObserver, SharedDnsResolver, TlsClientConfig},
    h2_pool::H2Pool,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendTransportEntry {
    Http1,
    H2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportClientRotation {
    rotation: BackendClientRotation,
}

impl TransportClientRotation {
    /// Returns true when transport rotated or recreated the backend client.
    pub fn rotated(self) -> bool {
        self.rotation.changed()
    }

    /// Returns generation movement for protocols that track reusable client generations.
    pub fn generations(self) -> Option<(u64, u64)> {
        self.rotation.generations()
    }
}

/// Canonical transport façade used by edge/runtime code for backend execution.
pub struct UpstreamTransportPool {
    backend_entries: HashMap<String, BackendTransportEntry>,
    h1_pool: H1Pool,
    h2_pool: H2Pool,
    execution_timeout: Duration,
}

impl UpstreamTransportPool {
    /// Execute a canonical upstream request against the resolved backend target.
    pub async fn send_backend_request(
        &self,
        backend: &str,
        req: Request<BoxBody<Bytes, Infallible>>,
    ) -> Result<hyper::Response<Incoming>, ProxyError> {
        self.execute(backend, req).await
    }

    /// Build a transport pool from already-interpreted backend transport entries.
    pub fn new_from_runtime_backends<I>(
        backends: I,
        backend_tls: HashMap<String, TlsClientConfig>,
        connection_policy: RuntimeBackendConnectionPolicy,
        dns_resolver: SharedDnsResolver,
    ) -> Result<Self, String>
    where
        I: IntoIterator<Item = (String, RuntimeBackendTransportKind)>,
    {
        Self::new_runtime_with_observer(
            backends,
            backend_tls,
            connection_policy.max_inflight,
            connection_policy.max_idle_per_backend,
            connection_policy.pool_idle_timeout,
            connection_policy.connect_timeout,
            connection_policy.execution_timeout,
            dns_resolver,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_runtime_with_observer<I>(
        backends: I,
        backend_tls: HashMap<String, TlsClientConfig>,
        max_inflight: usize,
        max_idle_per_backend: usize,
        pool_idle_timeout: Duration,
        connect_timeout: Duration,
        execution_timeout: Duration,
        dns_resolver: SharedDnsResolver,
        connect_observer: Option<ConnectObserver>,
    ) -> Result<Self, String>
    where
        I: IntoIterator<Item = (String, RuntimeBackendTransportKind)>,
    {
        let mut backend_entries = HashMap::new();
        let mut h1_backends = Vec::new();
        let mut h2_backends = Vec::new();

        for (backend, runtime_transport) in backends {
            let entry = Self::resolve_runtime_transport(runtime_transport);
            backend_entries.insert(backend.clone(), entry);
            match entry {
                BackendTransportEntry::Http1 => h1_backends.push(backend),
                BackendTransportEntry::H2 => h2_backends.push(backend),
            }
        }

        let h1_pool = H1Pool::new_with_observer(
            h1_backends,
            max_inflight,
            max_idle_per_backend,
            pool_idle_timeout,
            connect_timeout,
            dns_resolver.clone(),
            connect_observer.clone(),
        );
        let h2_pool = H2Pool::new_with_observer(
            h2_backends,
            backend_tls,
            max_inflight,
            max_idle_per_backend,
            pool_idle_timeout,
            connect_timeout,
            dns_resolver,
            connect_observer,
        )?;

        Ok(Self {
            backend_entries,
            h1_pool,
            h2_pool,
            execution_timeout,
        })
    }

    fn resolve_runtime_transport(transport: RuntimeBackendTransportKind) -> BackendTransportEntry {
        match transport {
            RuntimeBackendTransportKind::Http1 => BackendTransportEntry::Http1,
            RuntimeBackendTransportKind::H2 => BackendTransportEntry::H2,
        }
    }

    /// Build the canonical transport façade directly from runtime upstream definitions.
    pub fn from_runtime_upstreams<'a, I>(
        upstreams: I,
        connection_policy: &RuntimeBackendConnectionPolicy,
        dns_resolver: SharedDnsResolver,
        connect_observer: Option<ConnectObserver>,
    ) -> Result<Self, String>
    where
        I: IntoIterator<Item = &'a RuntimeUpstream>,
    {
        let mut backends = Vec::new();
        let mut backend_tls = HashMap::new();

        for upstream in upstreams {
            for backend in &upstream.backends {
                let backend_addr = backend.backend.address.clone();
                backends.push((backend_addr.clone(), backend.endpoint.transport_kind));
                if matches!(
                    backend.endpoint.transport_kind,
                    RuntimeBackendTransportKind::H2
                ) {
                    backend_tls.insert(
                        backend_addr,
                        TlsClientConfig::from(upstream.backend_tls_policy()),
                    );
                }
            }
        }

        Self::new_runtime_with_observer(
            backends,
            backend_tls,
            connection_policy.max_inflight,
            connection_policy.max_idle_per_backend,
            connection_policy.pool_idle_timeout,
            connection_policy.connect_timeout,
            connection_policy.execution_timeout,
            dns_resolver,
            connect_observer,
        )
    }

    fn backend_entry(&self, backend: &str) -> Option<BackendTransportEntry> {
        self.backend_entries.get(backend).copied()
    }

    async fn execute(
        &self,
        backend: &str,
        req: Request<BoxBody<Bytes, Infallible>>,
    ) -> Result<hyper::Response<Incoming>, ProxyError> {
        match self.backend_entry(backend) {
            Some(BackendTransportEntry::Http1) => {
                self.execute_with_timeout(backend, self.h1_pool.send(backend, req))
                    .await
            }
            Some(BackendTransportEntry::H2) => {
                self.execute_with_timeout(backend, self.h2_pool.send(backend, req))
                    .await
            }
            None => Err(ProxyError::Pool(PoolError::UnknownBackend(
                backend.to_string(),
            ))),
        }
    }

    pub fn rotate_backend_client(&self, backend: &str) -> Result<TransportClientRotation, String> {
        match self.backend_entry(backend) {
            Some(BackendTransportEntry::Http1) => self
                .h1_pool
                .rotate_backend_client(backend)
                .map(Self::transport_rotation),
            Some(BackendTransportEntry::H2) => self
                .h2_pool
                .rotate_backend_client(backend)
                .map(Self::transport_rotation),
            None => Ok(TransportClientRotation {
                rotation: BackendClientRotation::missing_backend(),
            }),
        }
    }

    fn transport_rotation(rotation: BackendClientRotation) -> TransportClientRotation {
        TransportClientRotation { rotation }
    }

    async fn execute_with_timeout<F>(
        &self,
        _backend: &str,
        send: F,
    ) -> Result<hyper::Response<Incoming>, ProxyError>
    where
        F: std::future::Future<Output = Result<hyper::Response<Incoming>, PoolError>>,
    {
        tokio::time::timeout(self.execution_timeout, send)
            .await
            .map_err(|_| ProxyError::Timeout)?
            .map_err(ProxyError::Pool)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, convert::Infallible, time::Duration};

    use http_body_util::{BodyExt, Empty, combinators::BoxBody};
    use hyper::{Request, body::Bytes};
    use spooky_config::{
        config::{
            Backend, ClientAuth, Config, ForwardedHeaderPolicy, ForwardedHeaderPolicyMode, Listen,
            LoadBalancing, Log, Observability, Performance, Resilience, RouteMatch, Security, Tls,
            Upstream, UpstreamHostPolicy, UpstreamHostPolicyMode, UpstreamTls,
        },
        runtime::{RuntimeBackendConnectionPolicy, RuntimeBackendTransportKind, RuntimeConfig},
    };
    use spooky_errors::{PoolError, ProxyError};

    use super::{BackendTransportEntry, UpstreamTransportPool};
    use crate::h2_client::SharedDnsResolver;

    fn connection_policy() -> RuntimeBackendConnectionPolicy {
        RuntimeBackendConnectionPolicy {
            max_inflight: 8,
            max_idle_per_backend: 4,
            pool_idle_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(2),
            execution_timeout: Duration::from_secs(5),
        }
    }

    fn runtime_config_with_backends(backends: &[(&str, &str)]) -> RuntimeConfig {
        let mut upstreams = HashMap::new();
        for (idx, (name, address)) in backends.iter().enumerate() {
            upstreams.insert(
                (*name).to_string(),
                Upstream {
                    load_balancing: LoadBalancing {
                        lb_type: "round-robin".to_string(),
                        key: None,
                    },
                    auth: Default::default(),
                    host_policy: UpstreamHostPolicy {
                        mode: UpstreamHostPolicyMode::Rewrite,
                        host: Some(format!("{name}.internal")),
                    },
                    forwarded_headers: ForwardedHeaderPolicy {
                        mode: ForwardedHeaderPolicyMode::Append,
                    },
                    tls: None,
                    route: RouteMatch {
                        host: Some(format!("{name}.example.com")),
                        path_prefix: Some("/".to_string()),
                        method: None,
                    },
                    backends: vec![Backend {
                        id: format!("backend-{idx}"),
                        address: (*address).to_string(),
                        weight: 100,
                        health_check: None,
                    }],
                },
            );
        }

        RuntimeConfig::from_config(&Config {
            version: 1,
            listen: Listen {
                protocol: "http3".to_string(),
                port: 443,
                address: "0.0.0.0".to_string(),
                tls: Tls {
                    cert: "/tmp/tls/default.pem".to_string(),
                    key: "/tmp/tls/default.key".to_string(),
                    certificates: Vec::new(),
                    client_auth: ClientAuth::default(),
                },
            },
            listeners: Vec::new(),
            upstream: upstreams,
            load_balancing: None,
            upstream_tls: UpstreamTls::default(),
            log: Log::default(),
            performance: Performance::default(),
            observability: Observability::default(),
            resilience: Resilience::default(),
            security: Security::default(),
        })
        .expect("runtime config")
    }

    fn request() -> Request<BoxBody<Bytes, Infallible>> {
        Request::builder()
            .method("GET")
            .uri("https://example.com/")
            .body(Empty::<Bytes>::new().boxed())
            .expect("request")
    }

    #[test]
    fn from_runtime_upstreams_selects_backend_protocols_inside_the_transport_facade() {
        let runtime = runtime_config_with_backends(&[
            ("http1-api", "http://127.0.0.1:8080"),
            ("h2-api", "https://api.internal:8443"),
        ]);

        let pool = UpstreamTransportPool::from_runtime_upstreams(
            runtime.upstreams.values(),
            &runtime.policies.transport.backend_connections,
            SharedDnsResolver::new(),
            None,
        )
        .expect("transport pool");

        assert_eq!(
            pool.backend_entry("http://127.0.0.1:8080"),
            Some(BackendTransportEntry::Http1)
        );
        assert_eq!(
            pool.backend_entry("https://api.internal:8443"),
            Some(BackendTransportEntry::H2)
        );
    }

    #[test]
    fn callers_can_use_one_rotation_api_without_protocol_specific_branching() {
        let pool = UpstreamTransportPool::new_from_runtime_backends(
            [
                (
                    "http://127.0.0.1:8080".to_string(),
                    RuntimeBackendTransportKind::Http1,
                ),
                (
                    "https://api.internal:8443".to_string(),
                    RuntimeBackendTransportKind::H2,
                ),
            ],
            HashMap::new(),
            connection_policy(),
            SharedDnsResolver::new(),
        )
        .expect("transport pool");

        let http1_rotation = pool
            .rotate_backend_client("http://127.0.0.1:8080")
            .expect("http1 rotation");
        let h2_rotation = pool
            .rotate_backend_client("https://api.internal:8443")
            .expect("h2 rotation");

        assert!(http1_rotation.rotated());
        assert_eq!(http1_rotation.generations(), None);
        assert!(h2_rotation.rotated());
        assert_eq!(h2_rotation.generations(), Some((0, 1)));
    }

    #[tokio::test]
    async fn h1_rotation_results_stay_effective_without_generation_movement() {
        let pool = UpstreamTransportPool::new_from_runtime_backends(
            [(
                "http://127.0.0.1:8080".to_string(),
                RuntimeBackendTransportKind::Http1,
            )],
            HashMap::new(),
            connection_policy(),
            SharedDnsResolver::new(),
        )
        .expect("transport pool");

        let first_rotation = pool
            .rotate_backend_client("http://127.0.0.1:8080")
            .expect("first h1 rotation");
        let second_rotation = pool
            .rotate_backend_client("http://127.0.0.1:8080")
            .expect("second h1 rotation");

        assert!(first_rotation.rotated());
        assert!(second_rotation.rotated());
        assert_eq!(first_rotation.generations(), None);
        assert_eq!(second_rotation.generations(), None);
    }

    #[test]
    fn h2_rotation_results_report_generation_movement_on_each_rotation() {
        let pool = UpstreamTransportPool::new_from_runtime_backends(
            [(
                "https://api.internal:8443".to_string(),
                RuntimeBackendTransportKind::H2,
            )],
            HashMap::new(),
            connection_policy(),
            SharedDnsResolver::new(),
        )
        .expect("transport pool");

        let first_rotation = pool
            .rotate_backend_client("https://api.internal:8443")
            .expect("first h2 rotation");
        let second_rotation = pool
            .rotate_backend_client("https://api.internal:8443")
            .expect("second h2 rotation");

        assert!(first_rotation.rotated());
        assert!(second_rotation.rotated());
        assert_eq!(first_rotation.generations(), Some((0, 1)));
        assert_eq!(second_rotation.generations(), Some((1, 2)));
    }

    #[tokio::test]
    async fn unknown_backend_send_fails_at_the_transport_facade_boundary() {
        let pool = UpstreamTransportPool::new_from_runtime_backends(
            [(
                "https://api.internal:8443".to_string(),
                RuntimeBackendTransportKind::H2,
            )],
            HashMap::new(),
            connection_policy(),
            SharedDnsResolver::new(),
        )
        .expect("transport pool");

        let err = pool
            .send_backend_request("missing-backend", request())
            .await
            .expect_err("missing backend should fail");

        assert!(matches!(
            err,
            ProxyError::Pool(PoolError::UnknownBackend(ref backend))
                if backend == "missing-backend"
        ));
    }

    #[test]
    fn backend_not_found_rotation_is_a_no_op_facade_result() {
        let pool = UpstreamTransportPool::new_from_runtime_backends(
            [(
                "https://api.internal:8443".to_string(),
                RuntimeBackendTransportKind::H2,
            )],
            HashMap::new(),
            connection_policy(),
            SharedDnsResolver::new(),
        )
        .expect("transport pool");

        let rotation = pool
            .rotate_backend_client("missing-backend")
            .expect("missing backend rotation should not error");

        assert!(!rotation.rotated());
        assert_eq!(rotation.generations(), None);
    }
}
