use std::{future::Future, sync::Arc, time::Duration};

use impulse_config::runtime::ListenerRuntimeConfig;
use impulse_errors::ProxyError;
use tokio::{net::TcpListener, runtime::Handle};

use super::state::{BootstrapStartupState, build_bootstrap_startup_state};
use crate::{
    quic_listener::{QUICListener, runtime_handle, spawn_supervised_async_task},
    runtime::listener::ShutdownSignal,
    runtime::{bundle::RuntimeBundleHandle, shared_state::SharedRuntimeState},
};

pub(in crate::quic_listener) struct PreparedBootstrapListenerStartup {
    pub(in crate::quic_listener) bind: String,
    pub(in crate::quic_listener) alt_svc_value: String,
    pub(in crate::quic_listener) max_connections: usize,
    pub(in crate::quic_listener) connection_timeout: Duration,
    pub(in crate::quic_listener) listener_label: String,
    pub(in crate::quic_listener) listener: TcpListener,
    pub(in crate::quic_listener) runtime_handle: Handle,
    pub(in crate::quic_listener) runtime_bundle: Option<Arc<RuntimeBundleHandle>>,
    pub(in crate::quic_listener) shutdown_signal: Option<ShutdownSignal>,
    pub(in crate::quic_listener) startup_state: BootstrapStartupState,
}

pub(in crate::quic_listener) fn prepare_bootstrap_listener_startup(
    config: &ListenerRuntimeConfig,
    shared_state: &SharedRuntimeState,
    runtime_bundle: Option<Arc<RuntimeBundleHandle>>,
    shutdown_signal: Option<ShutdownSignal>,
) -> Result<PreparedBootstrapListenerStartup, ProxyError> {
    let transport_policy = &config.policies.transport;
    let timeout_policy = &config.policies.timeouts;
    let bind = format!(
        "{}:{}",
        config.listen.listen.address, config.listen.listen.port
    );
    let alt_svc_value = format!("h3=\":{}\"; ma=86400", config.listen.listen.port);
    let max_connections = transport_policy
        .connection_limits
        .max_active_connections
        .max(1);
    let connection_timeout = timeout_policy.client_body_idle;
    let listener_label = QUICListener::listener_label(config);
    shared_state
        .shared_services()
        .listener_tls_store
        .bootstrap_server_config(&listener_label)
        .ok_or_else(|| {
            ProxyError::Tls(format!(
                "failed to initialize bootstrap TLS listener config for '{}': missing reload state",
                listener_label
            ))
        })?;

    let runtime_handle = runtime_handle().ok_or_else(|| {
        ProxyError::Transport(
            "failed to start bootstrap TLS listener: no Tokio runtime available".to_string(),
        )
    })?;

    let std_listener = std::net::TcpListener::bind(&bind).map_err(|err| {
        ProxyError::Transport(format!(
            "failed to bind bootstrap TLS listener on {}: {}",
            bind, err
        ))
    })?;
    if let Err(err) = std_listener.set_nonblocking(true) {
        return Err(ProxyError::Transport(format!(
            "failed to set bootstrap TLS listener nonblocking ({}): {}",
            bind, err
        )));
    }
    let listener = {
        let _guard = runtime_handle.enter();
        TcpListener::from_std(std_listener).map_err(|err| {
            ProxyError::Transport(format!(
                "failed to register bootstrap TLS listener {}: {}",
                bind, err
            ))
        })?
    };

    Ok(PreparedBootstrapListenerStartup {
        bind,
        alt_svc_value,
        max_connections,
        connection_timeout,
        listener_label,
        listener,
        runtime_handle,
        runtime_bundle,
        shutdown_signal,
        startup_state: build_bootstrap_startup_state(config, shared_state),
    })
}

pub(in crate::quic_listener) fn spawn_bootstrap_listener_task<F>(runtime_handle: &Handle, task: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    spawn_supervised_async_task(runtime_handle, "bootstrap-tls-listener", None, task);
}
