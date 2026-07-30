use std::{
    collections::VecDeque,
    sync::{Arc, RwLock},
    time::Duration,
};

use log::{info, warn};
use spooky_config::runtime::{ListenerRuntimeConfig, RuntimeConfig};
use spooky_errors::ProxyError;

use crate::runtime::{
    generation::{
        RuntimeGenerationState, RuntimeGenerationView, RuntimeSharedServices,
        StartupOwnedRuntimeState,
    },
    policy::RuntimeLifecycleState,
    shared_state::SharedRuntimeState,
};

/// The fully assembled runtime generation published to listener workers.
///
/// A bundle is the canonical boundary between configuration/build time and live
/// worker execution: startup-owned state, generation-owned state, and shared
/// services are packaged together and swapped as one unit.
#[derive(Clone)]
pub struct RuntimeBundle {
    pub generation: u64,
    pub startup: StartupOwnedRuntimeState,
    pub runtime_config: RuntimeConfig,
    pub shared_state: Arc<SharedRuntimeState>,
}

impl RuntimeBundle {
    pub fn startup(&self) -> &StartupOwnedRuntimeState {
        &self.startup
    }

    pub fn generation_view(&self) -> RuntimeGenerationView<'_> {
        RuntimeGenerationView {
            generation: self.generation,
            startup: &self.startup,
            runtime_config: &self.runtime_config,
            shared: self.shared_state.shared_services(),
            state: self.shared_state.generation_state(),
        }
    }

    pub fn listener_runtime_config(&self, label: &str) -> Option<ListenerRuntimeConfig> {
        self.generation_view().listener_runtime_config(label)
    }
}

/// Read-only view of the generation currently installed in the process.
///
/// This wrapper gives callers stable access to the active bundle without
/// exposing the swap mechanism itself.
#[derive(Clone)]
pub struct ActiveRuntimeGeneration {
    bundle: Arc<RuntimeBundle>,
}

impl ActiveRuntimeGeneration {
    pub fn bundle(&self) -> &RuntimeBundle {
        self.bundle.as_ref()
    }

    pub fn generation(&self) -> u64 {
        self.bundle.generation
    }

    pub fn startup(&self) -> &StartupOwnedRuntimeState {
        &self.bundle.startup
    }

    pub fn runtime_config(&self) -> &RuntimeConfig {
        &self.bundle.runtime_config
    }

    pub fn shared_services(&self) -> &RuntimeSharedServices {
        self.bundle.shared_state.shared_services()
    }

    pub fn state(&self) -> &RuntimeGenerationState {
        self.bundle.shared_state.generation_state()
    }

    pub fn view(&self) -> RuntimeGenerationView<'_> {
        self.bundle.generation_view()
    }

    pub fn listener_runtime_config(&self, label: &str) -> Option<ListenerRuntimeConfig> {
        self.bundle.listener_runtime_config(label)
    }
}

const MAX_ARCHIVED_RUNTIME_GENERATIONS: usize = 8;

/// Runtime-owned lifecycle status for retained generations.
///
/// This is intentionally narrower than the control-plane activation contract.
/// It answers one ownership question only: how the runtime handle currently
/// classifies a retained generation for rollback/history purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeGenerationRecordStatus {
    Active,
    Previous,
    FailedPrepare,
    RolledBack,
    Superseded,
}

impl RuntimeGenerationRecordStatus {
    #[must_use]
    pub fn is_rollback_candidate(self) -> bool {
        matches!(self, Self::Previous | Self::RolledBack | Self::Superseded)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Previous => "previous",
            Self::FailedPrepare => "failed_prepare",
            Self::RolledBack => "rolled_back",
            Self::Superseded => "superseded",
        }
    }
}

/// Retained runtime generation metadata owned by [`RuntimeBundleHandle`].
#[derive(Clone)]
pub struct RuntimeGenerationRecord {
    generation: u64,
    status: RuntimeGenerationRecordStatus,
    bundle: Option<Arc<RuntimeBundle>>,
    note: Option<String>,
}

impl RuntimeGenerationRecord {
    fn active(bundle: Arc<RuntimeBundle>) -> Self {
        Self {
            generation: bundle.generation,
            status: RuntimeGenerationRecordStatus::Active,
            bundle: Some(bundle),
            note: None,
        }
    }

    fn archived(
        generation: u64,
        status: RuntimeGenerationRecordStatus,
        bundle: Option<Arc<RuntimeBundle>>,
        note: Option<String>,
    ) -> Self {
        Self {
            generation,
            status,
            bundle,
            note,
        }
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn status(&self) -> RuntimeGenerationRecordStatus {
        self.status
    }

    #[must_use]
    pub fn bundle(&self) -> Option<&Arc<RuntimeBundle>> {
        self.bundle.as_ref()
    }

    #[must_use]
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    #[must_use]
    pub fn has_bundle(&self) -> bool {
        self.bundle.is_some()
    }
}

/// Atomic handle for publishing and reading the active runtime generation.
///
/// Ownership of reload lifecycle transitions stays here. Callers that only need
/// runtime data should prefer [`Self::current_view`] or [`Self::with_current_view`].
#[derive(Clone)]
pub struct RuntimeBundleHandle {
    inner: Arc<RwLock<Arc<RuntimeBundle>>>,
    history: Arc<RwLock<VecDeque<RuntimeGenerationRecord>>>,
    lifecycle: Arc<RuntimeLifecycleState>,
}

impl RuntimeBundleHandle {
    pub fn new(bundle: RuntimeBundle) -> Self {
        // The handle is constructed only once the first generation has been built
        // and is about to be published, so the process is `Running` from the
        // handle's perspective. Drain/shutdown transitions move it forward from
        // here (see `begin_drain`/`begin_shutdown`).
        let lifecycle = RuntimeLifecycleState::new();
        let _ = lifecycle.mark_running();
        Self {
            inner: Arc::new(RwLock::new(Arc::new(bundle))),
            history: Arc::new(RwLock::new(VecDeque::new())),
            lifecycle: Arc::new(lifecycle),
        }
    }

    /// The shared process lifecycle state machine (Phase 6). All clones of this
    /// handle observe the same phase.
    pub fn lifecycle(&self) -> &RuntimeLifecycleState {
        &self.lifecycle
    }

    /// Return the active generation's bundle.
    ///
    /// # Poisoning policy (Phase 5)
    ///
    /// This is on the hot poll path and cannot return an error to its callers, so
    /// it must not panic on a poisoned lock. Read-lock poisoning is recovered from
    /// rather than propagated, which is safe here by construction:
    ///
    /// - The only writer is [`Self::replace`], whose critical section is a single
    ///   `mem::replace` of an `Arc<RuntimeBundle>` — it runs no fallible or
    ///   panic-prone code while holding the write guard, so a panic *inside* the
    ///   critical section cannot occur.
    /// - A poisoned guard still references a fully-constructed, immutable
    ///   `Arc<RuntimeBundle>`; recovering it yields the last consistently-published
    ///   generation. There is no torn or partially-written state to observe.
    ///
    /// Recovering therefore preserves liveness (the data plane keeps serving the
    /// active generation) without masking a real corruption risk.
    pub fn current(&self) -> Arc<RuntimeBundle> {
        let guard = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(&guard)
    }

    /// Test-only: deliberately poison the internal lock so poison-recovery paths
    /// can be exercised. Poisoning leaves the stored `Arc<RuntimeBundle>` intact.
    #[cfg(test)]
    pub fn poison_for_test(&self) {
        let inner = Arc::clone(&self.inner);
        // Panic while holding the write guard, in a thread whose unwind we catch,
        // so the process survives but the lock is marked poisoned.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = inner.write().expect("lock not yet poisoned");
            panic!("intentional poison for test");
        }));
    }

    pub fn current_view(&self) -> ActiveRuntimeGeneration {
        ActiveRuntimeGeneration {
            bundle: self.current(),
        }
    }

    pub fn current_generation(&self) -> u64 {
        self.current_view().generation()
    }

    pub fn with_current_generation<R>(&self, f: impl FnOnce(ActiveRuntimeGeneration) -> R) -> R {
        f(self.current_view())
    }

    pub fn with_current_view<R>(&self, f: impl FnOnce(RuntimeGenerationView<'_>) -> R) -> R {
        self.with_current_generation(|current| f(current.view()))
    }

    /// Snapshot the active and retained generations known to the runtime.
    ///
    /// The first entry is always the live active generation. Archived entries are
    /// ordered newest-first behind it and capped in memory.
    pub fn generation_history(&self) -> Vec<RuntimeGenerationRecord> {
        let active = RuntimeGenerationRecord::active(self.current());
        let archived = self
            .history
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut history = Vec::with_capacity(archived.len() + 1);
        history.push(active);
        history.extend(archived);
        history
    }

    /// Return a retained rollback candidate by generation, if still available.
    pub fn rollback_candidate(&self, generation: u64) -> Option<Arc<RuntimeBundle>> {
        let current = self.current();
        if current.generation == generation {
            return Some(current);
        }

        self.history
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|entry| {
                entry.generation == generation && entry.status.is_rollback_candidate()
            })
            .and_then(|entry| entry.bundle.clone())
    }

    pub fn generation_record(&self, generation: u64) -> Option<RuntimeGenerationRecord> {
        let current = self.current();
        if current.generation == generation {
            return Some(RuntimeGenerationRecord::active(current));
        }

        self.history
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|entry| entry.generation == generation)
            .cloned()
    }

    pub(crate) fn record_failed_prepare(&self, generation: u64, note: impl Into<String>) {
        let mut history = self
            .history
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        history.retain(|entry| entry.generation != generation);
        history.push_front(RuntimeGenerationRecord::archived(
            generation,
            RuntimeGenerationRecordStatus::FailedPrepare,
            None,
            Some(note.into()),
        ));
        trim_runtime_generation_history(&mut history);
    }

    /// Atomically install `bundle` as the active generation, returning its
    /// generation number.
    ///
    /// # Ownership boundary (Phase 3)
    ///
    /// The swap replaces the whole [`RuntimeBundle`] pointer. By ownership class
    /// (see [`crate::runtime::policy::OwnedRuntimeState`]):
    ///
    /// - **Generation-owned** ([`RuntimeGenerationState`]): fully replaced. The
    ///   old generation's state is retired below.
    /// - **Startup-owned** ([`StartupOwnedRuntimeState`]): the caller
    ///   (`build_runtime_reload_plan`) carries the current values forward, and the
    ///   reload validator has already rejected any startup-owned change as
    ///   restart-required, so the swap must not observe a change here.
    /// - **Process-shared** ([`RuntimeSharedServices`]): logically one instance
    ///   per process; in the current implementation the reload rebuilds it (see
    ///   the note on `RuntimeSharedServices`).
    ///
    /// The active generation is only mutated at the single `mem::replace`; every
    /// failure path before it leaves the running generation intact.
    ///
    /// # Lifecycle gate (Phase 6)
    ///
    /// A reload commit is only legal while the process is `Running`. If a drain or
    /// shutdown has begun, the swap is rejected before touching the active
    /// generation, so a reload cannot race a shutdown into ambiguous state.
    pub fn replace(&self, bundle: RuntimeBundle) -> Result<u64, ProxyError> {
        self.replace_with_archive_status(bundle, RuntimeGenerationRecordStatus::Previous)
    }

    pub(crate) fn replace_with_archive_status(
        &self,
        bundle: RuntimeBundle,
        previous_status: RuntimeGenerationRecordStatus,
    ) -> Result<u64, ProxyError> {
        let generation = bundle.generation;
        let next_tasks = Arc::clone(&bundle.shared_state.generation_state().generation_tasks);

        if let Some(rejection) = self.lifecycle.check_reload_allowed().rejection() {
            next_tasks.abort_generation();
            warn!(
                "runtime reload commit rejected in lifecycle phase {:?}: refusing to install generation {}",
                self.lifecycle.phase(),
                generation
            );
            return Err(ProxyError::Transport(rejection.to_string()));
        }

        let previous_generation = self.current().generation;
        let previous = {
            // Poisoning cannot actually occur (the critical section runs no
            // panic-prone code — see `current()`), but the reload path can safely
            // return an error, so fail closed here rather than recover: abort the
            // staged generation's tasks and leave the active generation intact.
            let mut guard = match self.inner.write() {
                Ok(guard) => guard,
                Err(_) => {
                    next_tasks.abort_generation();
                    return Err(ProxyError::Transport(
                        "runtime bundle lock poisoned".to_string(),
                    ));
                }
            };
            // --- old-generation teardown boundary: BEGIN ---
            // The previous bundle's generation-owned state is now unreachable to
            // new readers; existing readers keep their clone until it drains.
            std::mem::replace(&mut *guard, Arc::new(bundle))
        };
        // Retire only the previous generation's generation-owned background tasks.
        // Startup-owned and process-shared resources are not torn down here.
        self.archive_previous_generation(previous.clone(), previous_status);
        previous
            .shared_state
            .generation_state()
            .generation_tasks
            .retire_generation(Duration::from_secs(5));
        // --- old-generation teardown boundary: END ---
        info!(
            "runtime generation swap committed: {} -> {} (lifecycle phase {:?})",
            previous_generation,
            generation,
            self.lifecycle.phase()
        );
        Ok(generation)
    }

    fn archive_previous_generation(
        &self,
        previous: Arc<RuntimeBundle>,
        status: RuntimeGenerationRecordStatus,
    ) {
        let mut history = self
            .history
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in history.iter_mut() {
            if entry.status == RuntimeGenerationRecordStatus::Previous {
                entry.status = RuntimeGenerationRecordStatus::Superseded;
            }
        }
        history.retain(|entry| entry.generation != previous.generation);
        history.push_front(RuntimeGenerationRecord::archived(
            previous.generation,
            status,
            Some(previous),
            None,
        ));
        trim_runtime_generation_history(&mut history);
    }
}

fn trim_runtime_generation_history(history: &mut VecDeque<RuntimeGenerationRecord>) {
    while history.len() > MAX_ARCHIVED_RUNTIME_GENERATIONS {
        history.pop_back();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        future,
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use rcgen::{Certificate, CertificateParams, SanType};
    use spooky_config::{
        config::{
            Backend, ClientAuth, Config as SpookyConfigConfig, Listen, LoadBalancing, Log, LogFile,
            LogFormat, Observability, Performance, Resilience, RouteMatch, Security, Tls, Upstream,
            UpstreamTls,
        },
        runtime::RuntimeConfig,
    };
    use tempfile::tempdir;
    use tokio::sync::oneshot;

    use super::*;
    use crate::runtime::{listener::QUICListener, tasks::RuntimeTaskRegistration};

    fn write_test_cert_for_name(dir: &Path, cert_name: &str, dns_name: &str) -> (String, String) {
        let mut params = CertificateParams::new(vec![dns_name.to_string()]);
        params
            .subject_alt_names
            .push(SanType::DnsName(dns_name.to_string()));
        let cert = Certificate::from_params(params).expect("failed to build cert");

        let cert_path = dir.join(format!("{cert_name}.pem"));
        let key_path = dir.join(format!("{cert_name}.key.pem"));
        std::fs::write(&cert_path, cert.serialize_pem().expect("serialize cert"))
            .expect("write cert");
        std::fs::write(&key_path, cert.serialize_private_key_pem()).expect("write key");
        (
            cert_path.to_string_lossy().to_string(),
            key_path.to_string_lossy().to_string(),
        )
    }

    fn test_config(cert: String, key: String, backend_addr: &str) -> SpookyConfigConfig {
        let mut upstreams = HashMap::new();
        upstreams.insert(
            "api".to_string(),
            Upstream {
                load_balancing: LoadBalancing {
                    lb_type: "round-robin".to_string(),
                    key: None,
                },
                auth: Default::default(),
                host_policy: Default::default(),
                forwarded_headers: Default::default(),
                tls: None,
                route: RouteMatch {
                    path_prefix: Some("/".to_string()),
                    ..Default::default()
                },
                backends: vec![Backend {
                    id: "b1".to_string(),
                    address: backend_addr.to_string(),
                    weight: 1,
                    health_check: None,
                }],
            },
        );

        SpookyConfigConfig {
            version: 1,
            listen: Listen {
                protocol: "http3".to_string(),
                port: 9889,
                address: "127.0.0.1".to_string(),
                tls: Tls {
                    cert,
                    key,
                    certificates: vec![],
                    client_auth: ClientAuth::default(),
                },
            },
            listeners: vec![],
            upstream: upstreams,
            load_balancing: Some(LoadBalancing {
                lb_type: "round-robin".to_string(),
                key: None,
            }),
            upstream_tls: UpstreamTls::default(),
            log: Log {
                level: "info".to_string(),
                file: LogFile {
                    enabled: false,
                    path: String::new(),
                },
                format: LogFormat::Plain,
            },
            performance: Performance::default(),
            observability: Observability::default(),
            resilience: Resilience::default(),
            security: Security::default(),
        }
    }

    fn runtime_bundle_from_config(
        generation: u64,
        config_path: &str,
        config: &SpookyConfigConfig,
    ) -> RuntimeBundle {
        let runtime_config = RuntimeConfig::from_config(config).expect("runtime config");
        let mut bundle = QUICListener::build_runtime_bundle(
            config_path.to_string(),
            config.log.clone(),
            &runtime_config,
        )
        .expect("runtime bundle");
        bundle.generation = generation;
        bundle
    }

    fn runtime_bundle_pair(
        dir: &Path,
        startup_path: &str,
        startup_backend_addr: &str,
        next_generation: u64,
        next_path: &str,
        next_backend_addr: &str,
    ) -> (RuntimeBundle, RuntimeBundle) {
        let (cert, key) = write_test_cert_for_name(dir, "server", "api.example.com");
        let startup = test_config(cert.clone(), key.clone(), startup_backend_addr);
        let reloaded = test_config(cert, key, next_backend_addr);
        (
            runtime_bundle_from_config(1, startup_path, &startup),
            runtime_bundle_from_config(next_generation, next_path, &reloaded),
        )
    }

    struct CompletionSignal(Option<oneshot::Sender<()>>);

    impl Drop for CompletionSignal {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    fn spawn_generation_task(
        bundle: &RuntimeBundle,
        completed: Arc<AtomicBool>,
    ) -> tokio::task::AbortHandle {
        let (completion_tx, completion_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _completion = CompletionSignal(Some(completion_tx));
            future::pending::<()>().await;
        });
        let abort = task.abort_handle();
        let join = task;
        tokio::spawn(async move {
            let _ = join.await;
            completed.store(true, Ordering::Release);
        });
        bundle
            .shared_state
            .generation_state()
            .generation_tasks
            .register(RuntimeTaskRegistration::new(abort.clone(), completion_rx));
        abort
    }

    mod runtime_generation_view_ownership {
        use super::*;

        #[test]
        fn current_generation_helpers_share_one_canonical_active_view() {
            let dir = tempdir().expect("tempdir");
            let (startup_bundle, _) = runtime_bundle_pair(
                dir.path(),
                "runtime.yaml",
                "http://127.0.0.1:7001",
                8,
                "unused.yaml",
                "http://127.0.0.1:7002",
            );
            let mut bundle = startup_bundle;
            bundle.generation = 7;
            let handle = RuntimeBundleHandle::new(bundle);

            let current = handle.current_view();
            let via_generation = handle.with_current_generation(|active| {
                (
                    active.generation(),
                    active.startup().config_path.clone(),
                    active
                        .runtime_config()
                        .upstreams
                        .get("api")
                        .expect("upstream")
                        .backends[0]
                        .backend
                        .address
                        .clone(),
                )
            });
            let via_view = handle.with_current_view(|view| {
                (
                    view.generation,
                    view.startup.config_path.clone(),
                    view.runtime_config
                        .upstreams
                        .get("api")
                        .expect("upstream")
                        .backends[0]
                        .backend
                        .address
                        .clone(),
                )
            });

            assert_eq!(handle.current_generation(), 7);
            assert_eq!(current.generation(), 7);
            assert_eq!(
                via_generation,
                (
                    7,
                    "runtime.yaml".to_string(),
                    "http://127.0.0.1:7001".to_string()
                )
            );
            assert_eq!(via_generation, via_view);
        }

        #[test]
        fn startup_owned_state_stays_stable_while_generation_owned_runtime_changes() {
            let dir = tempdir().expect("tempdir");
            let (current_bundle, next_bundle) = runtime_bundle_pair(
                dir.path(),
                "spooky.yaml",
                "http://127.0.0.1:7001",
                2,
                "spooky.yaml",
                "http://127.0.0.1:7002",
            );
            let handle = RuntimeBundleHandle::new(current_bundle);

            handle.replace(next_bundle).expect("replace");

            let active = handle.current_view();
            assert_eq!(active.generation(), 2);
            assert_eq!(active.startup().config_path, "spooky.yaml");
            assert_eq!(
                active
                    .runtime_config()
                    .upstreams
                    .get("api")
                    .expect("active upstream")
                    .backends[0]
                    .backend
                    .address,
                "http://127.0.0.1:7002"
            );
        }

        #[test]
        fn current_view_helpers_follow_the_live_generation_after_replacement() {
            let dir = tempdir().expect("tempdir");
            let (current_bundle, next_bundle) = runtime_bundle_pair(
                dir.path(),
                "startup.yaml",
                "http://127.0.0.1:7001",
                2,
                "reloaded.yaml",
                "http://127.0.0.1:7002",
            );
            let handle = RuntimeBundleHandle::new(current_bundle);

            handle.replace(next_bundle).expect("replace");

            let via_generation = handle.with_current_generation(|active| {
                (
                    active.generation(),
                    active.startup().config_path.clone(),
                    active
                        .runtime_config()
                        .upstreams
                        .get("api")
                        .expect("active upstream")
                        .backends[0]
                        .backend
                        .address
                        .clone(),
                )
            });
            let via_view = handle.with_current_view(|view| {
                (
                    view.generation,
                    view.startup.config_path.clone(),
                    view.runtime_config
                        .upstreams
                        .get("api")
                        .expect("active upstream")
                        .backends[0]
                        .backend
                        .address
                        .clone(),
                )
            });

            assert_eq!(handle.current_generation(), 2);
            assert_eq!(
                via_generation,
                (
                    2,
                    "reloaded.yaml".to_string(),
                    "http://127.0.0.1:7002".to_string()
                )
            );
            assert_eq!(via_generation, via_view);
        }

        #[test]
        fn stale_generation_views_keep_the_original_runtime_snapshot_after_replacement() {
            let dir = tempdir().expect("tempdir");
            let (current_bundle, next_bundle) = runtime_bundle_pair(
                dir.path(),
                "startup.yaml",
                "http://127.0.0.1:7001",
                2,
                "reloaded.yaml",
                "http://127.0.0.1:7002",
            );
            let handle = RuntimeBundleHandle::new(current_bundle.clone());

            let stale = handle.current_view();
            let installed = handle.replace(next_bundle).expect("replace");

            assert_eq!(installed, 2);
            assert_eq!(handle.current_generation(), 2);
            assert_eq!(stale.generation(), 1);
            assert_eq!(stale.startup().config_path, "startup.yaml");
            assert_eq!(
                stale
                    .runtime_config()
                    .upstreams
                    .get("api")
                    .expect("stale upstream")
                    .backends[0]
                    .backend
                    .address,
                "http://127.0.0.1:7001"
            );
            assert_eq!(
                handle
                    .current()
                    .runtime_config
                    .upstreams
                    .get("api")
                    .expect("current upstream")
                    .backends[0]
                    .backend
                    .address,
                "http://127.0.0.1:7002"
            );
        }

        #[tokio::test]
        async fn bundle_replacement_retires_only_previous_generation_tasks() {
            let dir = tempdir().expect("tempdir");
            let (current_bundle, next_bundle) = runtime_bundle_pair(
                dir.path(),
                "spooky.yaml",
                "http://127.0.0.1:7001",
                2,
                "spooky.yaml",
                "http://127.0.0.1:7002",
            );

            let retired_completed = Arc::new(AtomicBool::new(false));
            let active_completed = Arc::new(AtomicBool::new(false));
            let _retired_task =
                spawn_generation_task(&current_bundle, Arc::clone(&retired_completed));
            let active_task = spawn_generation_task(&next_bundle, Arc::clone(&active_completed));

            let handle = RuntimeBundleHandle::new(current_bundle);
            handle.replace(next_bundle).expect("replace");
            tokio::time::sleep(Duration::from_millis(20)).await;

            assert!(
                retired_completed.load(Ordering::Acquire),
                "previous generation tasks should retire when the bundle is replaced"
            );
            assert!(
                !active_completed.load(Ordering::Acquire),
                "new generation tasks should remain active after replacement"
            );

            active_task.abort();
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(active_completed.load(Ordering::Acquire));
        }
    }

    mod runtime_generation_history {
        use super::*;

        #[test]
        fn replacement_retains_previous_generation_as_rollback_candidate() {
            let dir = tempdir().expect("tempdir");
            let (current_bundle, next_bundle) = runtime_bundle_pair(
                dir.path(),
                "startup.yaml",
                "http://127.0.0.1:7001",
                2,
                "reloaded.yaml",
                "http://127.0.0.1:7002",
            );
            let previous_generation = current_bundle.generation;
            let handle = RuntimeBundleHandle::new(current_bundle);

            handle.replace(next_bundle).expect("replace");

            let history = handle.generation_history();
            assert_eq!(history[0].generation(), 2);
            assert_eq!(history[0].status(), RuntimeGenerationRecordStatus::Active);
            assert_eq!(history[1].generation(), previous_generation);
            assert_eq!(
                history[1].status(),
                RuntimeGenerationRecordStatus::Previous
            );
            let rollback = handle
                .rollback_candidate(previous_generation)
                .expect("previous generation should be retained");
            assert_eq!(rollback.generation, previous_generation);
        }

        #[test]
        fn replacement_marks_older_retained_generations_as_superseded() {
            let dir = tempdir().expect("tempdir");
            let (bundle_one, bundle_two) = runtime_bundle_pair(
                dir.path(),
                "one.yaml",
                "http://127.0.0.1:7001",
                2,
                "two.yaml",
                "http://127.0.0.1:7002",
            );
            let (_, mut bundle_three) = runtime_bundle_pair(
                dir.path(),
                "unused.yaml",
                "http://127.0.0.1:7003",
                3,
                "three.yaml",
                "http://127.0.0.1:7004",
            );
            bundle_three.generation = 3;
            let handle = RuntimeBundleHandle::new(bundle_one);

            handle.replace(bundle_two).expect("first replace");
            handle.replace(bundle_three).expect("second replace");

            let history = handle.generation_history();
            assert_eq!(history[0].generation(), 3);
            assert_eq!(history[0].status(), RuntimeGenerationRecordStatus::Active);
            assert_eq!(history[1].generation(), 2);
            assert_eq!(
                history[1].status(),
                RuntimeGenerationRecordStatus::Previous
            );
            assert_eq!(history[2].generation(), 1);
            assert_eq!(
                history[2].status(),
                RuntimeGenerationRecordStatus::Superseded
            );
            assert!(
                handle.rollback_candidate(1).is_some(),
                "superseded known-good generations should remain rollback candidates"
            );
        }

        #[test]
        fn failed_prepare_history_is_retained_without_mutating_active_generation() {
            let dir = tempdir().expect("tempdir");
            let (current_bundle, _) = runtime_bundle_pair(
                dir.path(),
                "runtime.yaml",
                "http://127.0.0.1:7001",
                2,
                "unused.yaml",
                "http://127.0.0.1:7002",
            );
            let handle = RuntimeBundleHandle::new(current_bundle);

            handle.record_failed_prepare(2, "could not prepare runtime generation");

            let history = handle.generation_history();
            assert_eq!(history[0].generation(), 1);
            assert_eq!(history[0].status(), RuntimeGenerationRecordStatus::Active);
            assert_eq!(history[1].generation(), 2);
            assert_eq!(
                history[1].status(),
                RuntimeGenerationRecordStatus::FailedPrepare
            );
            assert_eq!(
                history[1].note(),
                Some("could not prepare runtime generation")
            );
            assert!(history[1].bundle().is_none());
            assert_eq!(handle.current_generation(), 1);
            assert!(handle.rollback_candidate(2).is_none());
        }
    }
}
