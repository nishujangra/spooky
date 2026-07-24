use std::{
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

#[derive(Clone)]
pub struct RuntimeBundleHandle {
    inner: Arc<RwLock<Arc<RuntimeBundle>>>,
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
}
