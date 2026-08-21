use std::sync::{Arc, atomic::AtomicBool};

use impulse_config::runtime::ListenerRuntimeConfig;
use impulse_errors::ProxyError;

use super::bootstrap::spawn_bootstrap_tls_listener;
use crate::runtime::{bundle::RuntimeBundleHandle, shared_state::SharedRuntimeState};

impl super::QUICListener {
    pub fn spawn_bootstrap_tls_listener(
        config: &ListenerRuntimeConfig,
        shared_state: &SharedRuntimeState,
        runtime_bundle: Option<Arc<RuntimeBundleHandle>>,
        shutdown_signal: Option<Arc<AtomicBool>>,
    ) -> Result<(), ProxyError> {
        spawn_bootstrap_tls_listener(config, shared_state, runtime_bundle, shutdown_signal)
    }
}
