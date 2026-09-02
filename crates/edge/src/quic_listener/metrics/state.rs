use super::*;

#[derive(Clone)]
pub(in crate::quic_listener) struct MetricsEndpointState {
    pub(in crate::quic_listener) endpoint: MetricsEndpoint,
    pub(in crate::quic_listener) metrics: Arc<Metrics>,
}

impl MetricsServiceCtx {
    pub(in crate::quic_listener) fn current_state(&self) -> MetricsEndpointState {
        let runtime = self.runtime.current_view();
        MetricsEndpointState {
            endpoint: runtime.runtime_config().observability.metrics.clone(),
            metrics: runtime.metrics(),
        }
    }

    pub(in crate::quic_listener) fn current_tls_config(
        &self,
    ) -> Result<Option<Arc<RustlsServerConfig>>, ProxyError> {
        let runtime = self.runtime.current_view();
        if !runtime
            .runtime_config()
            .observability
            .metrics
            .allow_non_loopback
        {
            return Ok(None);
        }
        let listener = runtime
            .runtime_config()
            .primary_listener_runtime_config()
            .ok_or_else(|| {
                ProxyError::Transport("no effective listeners configured".to_string())
            })?;
        let security = runtime.control_api_security();
        QUICListener::build_control_api_server_tls_config(&listener, &security.client_auth)
            .map(Some)
    }
}

impl QUICListener {
    pub(in crate::quic_listener) fn current_metrics_endpoint_state(
        service_ctx: &MetricsServiceCtx,
    ) -> MetricsEndpointState {
        service_ctx.current_state()
    }
}
