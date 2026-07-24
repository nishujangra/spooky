use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use spooky_config::runtime::{ListenerRuntimeConfig, RuntimeConfig};
use spooky_errors::ProxyError;

use crate::runtime::{
    generation::{
        RuntimeGenerationState, RuntimeGenerationView, RuntimeSharedServices,
        StartupOwnedRuntimeState,
    },
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
}

impl RuntimeBundleHandle {
    pub fn new(bundle: RuntimeBundle) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Arc::new(bundle))),
        }
    }

    pub fn current(&self) -> Arc<RuntimeBundle> {
        self.inner
            .read()
            .map(|bundle| Arc::clone(&*bundle))
            .unwrap_or_else(|_| panic!("runtime bundle lock poisoned"))
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
    pub fn replace(&self, bundle: RuntimeBundle) -> Result<u64, ProxyError> {
        let generation = bundle.generation;
        let next_tasks = Arc::clone(&bundle.shared_state.generation_state().generation_tasks);
        let previous = {
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
        Ok(generation)
    }
}
