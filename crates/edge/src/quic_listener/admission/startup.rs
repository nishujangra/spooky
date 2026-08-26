use super::*;

impl QUICListener {
    pub(crate) fn register_jwt_jwks_metrics(metrics: &Arc<crate::Metrics>) {
        if let Ok(mut sink) = jwt_jwks_metrics_sink().write() {
            *sink = Arc::downgrade(metrics);
        }
    }

    pub(crate) fn initialize_jwks_startup(
        config: &RuntimeConfig,
    ) -> Result<(), impulse_errors::ProxyError> {
        Self::preflight_require_ready_jwks(config, "startup_preflight")
    }

    pub(crate) fn preflight_require_ready_jwks(
        config: &RuntimeConfig,
        trigger: &'static str,
    ) -> Result<(), impulse_errors::ProxyError> {
        let sources = runtime_jwks_sources(config);
        if sources.is_empty() {
            return Ok(());
        }

        // Preflight can run for startup checks, preview, or validation using a
        // candidate config. Those paths must not evict unrelated live JWKS
        // entries from the process-global cache.
        JwtJwksSharedCache::shared().register_sources(sources.iter());

        let require_ready = sources
            .into_iter()
            .filter(|source| matches!(source.startup_behavior, JwksStartupBehavior::RequireReady))
            .collect::<Vec<_>>();
        if require_ready.is_empty() {
            return Ok(());
        }

        std::thread::spawn(move || -> Result<(), String> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| format!("failed to create JWKS startup runtime: {err}"))?;
            runtime.block_on(async move {
                for source in require_ready {
                    refresh_jwks_source_once(source.clone(), trigger)
                        .await
                        .map_err(|failure| {
                            format!(
                                "jwks require-ready preflight failed source_id={} endpoint={} trigger={} startup_behavior=require_ready detail={}",
                                source.source_identity,
                                source.public_endpoint(),
                                trigger,
                                failure
                            )
                        })?;
                    let snapshot = JwtJwksSharedCache::shared()
                        .snapshot(&source.source_identity, Instant::now())
                        .ok_or_else(|| {
                            format!(
                                "jwks require-ready preflight failed source_id={} endpoint={} trigger={} startup_behavior=require_ready detail=missing_cache_snapshot",
                                source.source_identity,
                                source.public_endpoint(),
                                trigger
                            )
                        })?;
                    if !jwt_jwks_cache_state_usable(snapshot.state) {
                        return Err(format!(
                            "jwks require-ready preflight failed source_id={} endpoint={} trigger={} startup_behavior=require_ready state={} detail={}",
                            source.source_identity,
                            source.public_endpoint(),
                            trigger,
                            jwt_jwks_cache_state_name(snapshot.state),
                            snapshot.last_error.unwrap_or_else(|| {
                                "jwks source has no usable keys after startup preflight"
                                    .to_string()
                            })
                        ));
                    }
                }
                Ok(())
            })
        })
        .join()
        .map_err(|_| {
            impulse_errors::ProxyError::Transport(
                "jwks startup preflight thread panicked".to_string(),
            )
        })?
        .map_err(impulse_errors::ProxyError::Transport)
    }
}
impl QUICListener {
    pub(crate) fn spawn_jwks_refresh(
        config: &RuntimeConfig,
        task_registry: Arc<RuntimeTaskRegistry>,
    ) {
        let sources = runtime_jwks_sources(config);
        if sources.is_empty() {
            JwtJwksSharedCache::shared().reconcile_sources([].iter().copied());
            return;
        }

        JwtJwksSharedCache::shared().reconcile_sources(sources.iter());
        let Some(handle) = runtime_handle() else {
            log::error!("JWKS refresh disabled: no Tokio runtime available");
            return;
        };

        for source in sources {
            let task_source = source.clone();
            let registration = spawn_supervised_async_task(
                &handle,
                "jwks-refresh",
                None,
                async move {
                    let _ = refresh_jwks_source_once(task_source.clone(), "startup").await;
                    if matches!(
                        task_source.startup_behavior,
                        JwksStartupBehavior::RequireReady
                    ) {
                        let snapshot = JwtJwksSharedCache::shared()
                            .snapshot(&task_source.source_identity, Instant::now());
                        if !matches!(
                            snapshot.as_ref().map(|entry| entry.state),
                            Some(
                                JwtJwksCacheState::Fresh
                                    | JwtJwksCacheState::Stale
                                    | JwtJwksCacheState::RefreshFailedRetained
                            )
                        ) {
                            log::warn!(
                                "JWKS source not ready after startup refresh source_id={} endpoint={} startup_behavior=require_ready",
                                task_source.source_identity,
                                task_source.public_endpoint()
                            );
                        }
                    }

                    let mut ticker = tokio::time::interval(
                        task_source.refresh_interval.max(Duration::from_secs(1)),
                    );
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        ticker.tick().await;
                        let _ = refresh_jwks_source_once(task_source.clone(), "periodic").await;
                    }
                },
            );
            task_registry.register(registration);
        }
    }
}
