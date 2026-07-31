use std::sync::atomic::AtomicUsize;

use super::{
    context::{ConnectionSlotGuard, ControlApiListenerBinding},
    state::ControlApiState,
    *,
};
use crate::quic_listener::runtime_state::ControlPlaneBootstrap;

impl QUICListener {
    pub(in crate::quic_listener) fn spawn_control_api_endpoint(
        bootstrap: &ControlPlaneBootstrap<'_>,
    ) -> Result<(), ProxyError> {
        let state = bootstrap.control_api_service_ctx();
        let startup_state = state.current_service_state();
        if bootstrap.runtime_bundle.is_none() && !startup_state.endpoint.enabled {
            return Ok(());
        }
        let required = startup_state.endpoint.required;
        let listener_config = startup_state
            .runtime
            .runtime_config()
            .primary_listener_runtime_config()
            .ok_or_else(|| {
                ProxyError::Transport("no effective listeners configured".to_string())
            })?;
        let primary_listener_label = Self::listener_label(&listener_config);
        if startup_state.endpoint.enabled
            && startup_state
                .listener_tls_store()
                .bootstrap_server_config(&primary_listener_label)
                .is_none()
        {
            let msg = format!(
                "failed to initialize control API TLS config: missing reload state for listener '{}'",
                primary_listener_label
            );
            if required {
                return Err(ProxyError::Tls(msg));
            }
            error!("{}", msg);
            return Ok(());
        }

        let handle = match runtime_handle() {
            Some(handle) => handle,
            None => {
                let msg = "control API disabled (no Tokio runtime available)".to_string();
                if required {
                    return Err(ProxyError::Transport(msg));
                }
                error!("{}", msg);
                return Ok(());
            }
        };

        let initial_binding = if startup_state.endpoint.enabled {
            let bind = format!(
                "{}:{}",
                startup_state.endpoint.address, startup_state.endpoint.port
            );
            match Self::bind_tcp_listener(&bind, Some(&handle), "control API endpoint") {
                Ok(listener) => Some(ControlApiListenerBinding {
                    bind,
                    listener,
                    active_connections: Arc::new(AtomicUsize::new(0)),
                }),
                Err(msg) => {
                    if required {
                        return Err(ProxyError::Transport(msg));
                    }
                    error!("{}", msg);
                    None
                }
            }
        } else {
            None
        };

        spawn_supervised_async_task(
            &handle,
            "control-api-endpoint",
            Some(startup_state.metrics()),
            async move {
                let mut listener_binding = initial_binding;

                loop {
                    let runtime_state = state.current_service_state();
                    let endpoint = &runtime_state.endpoint;
                    let desired_bind = format!("{}:{}", endpoint.address, endpoint.port);

                    if !endpoint.enabled {
                        if let Some(binding) = listener_binding.take() {
                            info!(
                                "Control API endpoint disabled via runtime reload on {}",
                                binding.bind
                            );
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }

                    let needs_rebind = match listener_binding.as_ref() {
                        Some(binding) => binding.bind != desired_bind,
                        None => true,
                    };
                    if needs_rebind {
                        match Self::bind_tcp_listener(&desired_bind, None, "control API endpoint") {
                            Ok(listener) => {
                                info!("Control API endpoint ready bind=https://{}", desired_bind);
                                info!(
                                    "Control API endpoint paths bind={} health={} ready={} runtime={} reload_certs={}",
                                    desired_bind,
                                    runtime_state.paths.health_path,
                                    runtime_state.paths.ready_path,
                                    runtime_state.paths.runtime_path,
                                    runtime_state.paths.reload_certs_path,
                                );
                                info!(
                                    "Control API endpoint limits bind={} max_connections={} connection_timeout_ms={}",
                                    desired_bind,
                                    endpoint.max_connections.max(1),
                                    endpoint.connection_timeout_ms.max(1)
                                );
                                listener_binding = Some(ControlApiListenerBinding {
                                    bind: desired_bind.clone(),
                                    listener,
                                    active_connections: Arc::new(AtomicUsize::new(0)),
                                });
                            }
                            Err(err) => {
                                error!("{}", err);
                                tokio::time::sleep(Duration::from_millis(200)).await;
                                continue;
                            }
                        }
                    }

                    let Some(binding) = listener_binding.as_mut() else {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    };

                    let accept_result = tokio::select! {
                        accept = binding.listener.accept() => Some(accept),
                        _ = tokio::time::sleep(Duration::from_millis(200)) => None,
                    };
                    let Some(accept_result) = accept_result else {
                        continue;
                    };
                    let (stream, peer) = match accept_result {
                        Ok(v) => v,
                        Err(err) => {
                            error!("Control API endpoint accept failed: {}", err);
                            continue;
                        }
                    };
                    let state = state.clone();
                    let active_connections = Arc::clone(&binding.active_connections);
                    let max_connections = endpoint.max_connections.max(1);
                    if !Self::try_claim_control_api_connection_slot(
                        &active_connections,
                        max_connections,
                    ) {
                        runtime_state
                            .metrics()
                            .inc_control_api_connection_limit_drop();
                        warn!(
                            "Control API endpoint dropped connection from {} due to max connection limit ({})",
                            peer, max_connections
                        );
                        continue;
                    }

                    tokio::spawn(async move {
                        Self::serve_control_api_connection(state, active_connections, stream, peer)
                            .await;
                    });
                }
            },
        );
        Ok(())
    }

    fn try_claim_control_api_connection_slot(
        active_connections: &Arc<AtomicUsize>,
        max_connections: usize,
    ) -> bool {
        loop {
            let current = active_connections.load(Ordering::Relaxed);
            if current >= max_connections {
                return false;
            }
            if active_connections
                .compare_exchange(
                    current,
                    current.saturating_add(1),
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return true;
            }
        }
    }

    async fn serve_control_api_connection(
        state: ControlApiState,
        active_connections: Arc<AtomicUsize>,
        stream: tokio::net::TcpStream,
        peer: SocketAddr,
    ) {
        let _connection_guard = ConnectionSlotGuard::new(active_connections);
        let runtime_state = state.current_service_state();
        let timeout = Duration::from_millis(runtime_state.endpoint.connection_timeout_ms.max(1));
        let listener_tls_store = runtime_state.listener_tls_store();
        let Some(primary_listener_label) = runtime_state.primary_listener_label else {
            error!("Control API endpoint missing live primary listener label for TLS selection");
            return;
        };
        let Some(server_config) =
            listener_tls_store.bootstrap_server_config(&primary_listener_label)
        else {
            error!(
                "Control API endpoint missing live TLS config for listener {}",
                primary_listener_label
            );
            return;
        };
        let acceptor = TlsAcceptor::from(server_config);
        let tls_stream = match acceptor.accept(stream).await {
            Ok(stream) => stream,
            Err(err) => {
                error!(
                    "Control API endpoint TLS handshake failed from {}: {}",
                    peer, err
                );
                return;
            }
        };
        let io = TokioIo::new(tls_stream);
        let service = service_fn(move |req: Request<Incoming>| {
            let state = state.clone();
            async move { Ok::<_, hyper::Error>(Self::handle_control_api_request(req, &state).await) }
        });

        let serve = http1::Builder::new().serve_connection(io, service);
        match tokio::time::timeout(timeout, serve).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                error!("Control API endpoint connection failed: {}", err);
            }
            Err(_) => {
                debug!("Control API endpoint connection timed out");
            }
        }
    }
}
