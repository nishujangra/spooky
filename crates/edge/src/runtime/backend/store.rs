use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::SystemTime,
};

use crate::runtime::backend::{
    event::{BackendLifecycleMutation, BackendRefreshOutcome, BackendRefreshResult},
    lifecycle::RuntimeBackendLifecycleState,
    resolution::RuntimeBackendResolution,
    state::{BackendLifecycleSnapshot, BackendResolutionState},
};

#[derive(Debug, Clone, Default)]
pub struct RuntimeBackendResolutionStore {
    entries: Arc<RwLock<HashMap<String, RuntimeBackendLifecycleState>>>,
}

impl RuntimeBackendResolutionStore {
    pub fn new<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = RuntimeBackendResolution>,
    {
        let entries = entries
            .into_iter()
            .map(|entry| {
                let state = RuntimeBackendLifecycleState::from(&entry);
                (state.identity.backend_addr.clone(), state)
            })
            .collect();
        Self {
            entries: Arc::new(RwLock::new(entries)),
        }
    }

    pub fn backend(&self, backend_addr: &str) -> Option<RuntimeBackendLifecycleState> {
        self.entries
            .read()
            .ok()
            .and_then(|guard| guard.get(backend_addr).cloned())
    }

    pub fn resolution_state(&self, backend_addr: &str) -> Option<BackendResolutionState> {
        self.backend(backend_addr).map(|backend| backend.resolution)
    }

    pub fn snapshot(&self) -> HashMap<String, BackendLifecycleSnapshot> {
        self.entries
            .read()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(backend_addr, state)| (backend_addr.clone(), state.snapshot()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn hostname_backends(&self) -> Vec<RuntimeBackendLifecycleState> {
        self.entries
            .read()
            .map(|guard| {
                guard
                    .values()
                    .filter(|state| state.resolution.is_hostname())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn apply_resolution_refresh(
        &self,
        backend_addr: &str,
        resolved_addrs: Vec<SocketAddr>,
        refreshed_at: SystemTime,
    ) -> Option<BackendLifecycleMutation> {
        let resolved_addrs = canonicalize_socket_addrs(resolved_addrs);
        let mut guard = self.entries.write().ok()?;
        let entry = guard.get_mut(backend_addr)?;
        if !entry.resolution.is_hostname() {
            return None;
        }

        let previous_addrs =
            std::mem::replace(&mut entry.resolution.resolved_addrs, resolved_addrs.clone());
        entry.resolution.last_refresh_success_at = Some(refreshed_at);
        entry.resolution.refresh_generation = entry.resolution.refresh_generation.saturating_add(1);

        let result = if previous_addrs != resolved_addrs {
            BackendRefreshResult {
                identity: entry.identity.clone(),
                outcome: BackendRefreshOutcome::Updated {
                    previous_addrs,
                    current_addrs: resolved_addrs,
                    refreshed_at: entry.resolution.last_refresh_success_at,
                    refresh_generation: entry.resolution.refresh_generation,
                },
            }
        } else {
            BackendRefreshResult {
                identity: entry.identity.clone(),
                outcome: BackendRefreshOutcome::Unchanged {
                    current_addrs: resolved_addrs,
                    refreshed_at: entry.resolution.last_refresh_success_at,
                    refresh_generation: entry.resolution.refresh_generation,
                },
            }
        };

        Some(BackendLifecycleMutation::ResolutionUpdated {
            identity: entry.identity.clone(),
            state: entry.resolution.clone(),
            result,
        })
    }
}

fn canonicalize_socket_addrs(mut addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    addrs.sort_unstable();
    addrs.dedup();
    addrs
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;
    use crate::runtime::backend::event::BackendRefreshOutcome;

    #[test]
    fn lifecycle_store_snapshots_backend_resolution_state() {
        let store = RuntimeBackendResolutionStore::new([RuntimeBackendResolution::hostname(
            "https://backend.internal:8443".to_string(),
            "backend.internal".to_string(),
            8443,
        )]);

        let snapshot = store.snapshot();
        let backend = snapshot
            .get("https://backend.internal:8443")
            .expect("backend snapshot");

        assert_eq!(
            backend.identity.backend_addr,
            "https://backend.internal:8443"
        );
        assert!(backend.resolution.is_hostname());
    }

    #[test]
    fn lifecycle_store_returns_refresh_mutation_for_hostname_backend() {
        let store = RuntimeBackendResolutionStore::new([RuntimeBackendResolution::hostname(
            "https://backend.internal:8443".to_string(),
            "backend.internal".to_string(),
            8443,
        )]);

        let mutation = store
            .apply_resolution_refresh(
                "https://backend.internal:8443",
                vec!["10.0.0.10:8443".parse::<SocketAddr>().expect("addr")],
                SystemTime::UNIX_EPOCH,
            )
            .expect("refresh mutation");

        assert!(matches!(
            mutation,
            BackendLifecycleMutation::ResolutionUpdated {
                result: BackendRefreshResult {
                    outcome: BackendRefreshOutcome::Updated { .. },
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn lifecycle_store_returns_unchanged_refresh_mutation_and_increments_generation() {
        let backend_addr = "https://backend.internal:8443";
        let refreshed_at = SystemTime::UNIX_EPOCH;
        let resolved_addrs = vec!["10.0.0.10:8443".parse::<SocketAddr>().expect("addr")];
        let store = RuntimeBackendResolutionStore::new([RuntimeBackendResolution::hostname(
            backend_addr.to_string(),
            "backend.internal".to_string(),
            8443,
        )]);

        let first_mutation = store
            .apply_resolution_refresh(backend_addr, resolved_addrs.clone(), refreshed_at)
            .expect("first refresh mutation");
        assert!(matches!(
            first_mutation,
            BackendLifecycleMutation::ResolutionUpdated {
                result: BackendRefreshResult {
                    outcome: BackendRefreshOutcome::Updated {
                        refresh_generation: 1,
                        ..
                    },
                    ..
                },
                ..
            }
        ));

        let second_mutation = store
            .apply_resolution_refresh(backend_addr, resolved_addrs.clone(), refreshed_at)
            .expect("second refresh mutation");
        assert!(matches!(
            second_mutation,
            BackendLifecycleMutation::ResolutionUpdated {
                result: BackendRefreshResult {
                    outcome: BackendRefreshOutcome::Unchanged {
                        refresh_generation: 2,
                        ..
                    },
                    ..
                },
                state,
                ..
            } if state.resolved_addrs == resolved_addrs && state.refresh_generation == 2
        ));
    }
}
