use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use impulse_lb::{HealthTransition, upstream_pool::UpstreamPool};
use impulse_transport::{SharedDnsResolver, UpstreamTransportPool};

use crate::runtime::backend::{event, resolution::RuntimeBackendAddressKind};

use crate::runtime::backend::{
    resolution::RuntimeBackendResolution,
    state::{
        BackendHealthState, BackendIdentity, BackendLifecycleInventorySnapshot,
        BackendLifecycleSnapshot, BackendMembershipState, BackendPoolPlacementSnapshot,
        BackendResolutionState, CanonicalBackendLifecycleSnapshot,
    },
    store::RuntimeBackendResolutionStore,
};

use super::{
    dns::{BackendDnsRefreshApplication, apply_backend_dns_refresh},
    health::apply_backend_health_observation,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBackendLifecycleState {
    pub identity: BackendIdentity,
    pub resolution: BackendResolutionState,
    pub health: BackendHealthState,
    pub membership: BackendMembershipState,
}

impl RuntimeBackendLifecycleState {
    pub fn new(
        identity: BackendIdentity,
        resolution: BackendResolutionState,
        health: BackendHealthState,
        membership: BackendMembershipState,
    ) -> Self {
        Self {
            identity,
            resolution,
            health,
            membership,
        }
    }

    pub fn from_resolution_seed(resolution: &RuntimeBackendResolution) -> Self {
        Self {
            identity: BackendIdentity::from(resolution),
            resolution: BackendResolutionState::from(resolution),
            health: BackendHealthState::Unknown,
            membership: BackendMembershipState::Active,
        }
    }

    pub fn snapshot(&self) -> BackendLifecycleSnapshot {
        BackendLifecycleSnapshot {
            identity: self.identity.clone(),
            resolution: self.resolution.clone(),
            health: self.health.clone(),
            membership: self.membership,
        }
    }
}

impl From<&RuntimeBackendResolution> for RuntimeBackendLifecycleState {
    fn from(value: &RuntimeBackendResolution) -> Self {
        Self::from_resolution_seed(value)
    }
}

impl From<&RuntimeBackendLifecycleState> for BackendLifecycleSnapshot {
    fn from(value: &RuntimeBackendLifecycleState) -> Self {
        value.snapshot()
    }
}

#[derive(Debug, Clone)]
pub struct BackendLifecycleCoordinator {
    resolution_store: Arc<RuntimeBackendResolutionStore>,
}

impl BackendLifecycleCoordinator {
    pub fn new(resolution_store: Arc<RuntimeBackendResolutionStore>) -> Self {
        Self { resolution_store }
    }

    pub fn backend(&self, backend_addr: &str) -> Option<RuntimeBackendLifecycleState> {
        self.resolution_store.backend(backend_addr)
    }

    pub fn hostname_backends(&self) -> Vec<RuntimeBackendLifecycleState> {
        self.resolution_store.hostname_backends()
    }

    pub fn snapshot_backend(&self, backend_addr: &str) -> Option<BackendLifecycleSnapshot> {
        self.backend(backend_addr).map(|backend| backend.snapshot())
    }

    pub fn snapshot_all(&self) -> HashMap<String, BackendLifecycleSnapshot> {
        self.resolution_store.snapshot()
    }

    pub fn snapshot_inventory(
        &self,
        upstream_pools: &HashMap<String, Arc<RwLock<UpstreamPool>>>,
    ) -> BackendLifecycleInventorySnapshot {
        let mut snapshots = self
            .snapshot_all()
            .into_values()
            .map(|snapshot| {
                (
                    snapshot.identity.backend_addr.clone(),
                    CanonicalBackendLifecycleSnapshot {
                        identity: snapshot.identity,
                        resolution: snapshot.resolution,
                        health: snapshot.health,
                        membership: snapshot.membership,
                        placements: Vec::new(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        for (upstream_name, pool) in upstream_pools {
            let Ok(guard) = pool.read() else {
                continue;
            };
            let membership_summary = guard.membership_summary();
            for backend_index in guard.backend_indices() {
                let Some(backend_addr) = guard.backend_address(backend_index) else {
                    continue;
                };
                let Some(backend) = guard.backend_runtime_state(backend_index) else {
                    continue;
                };
                let entry = snapshots
                    .entry(backend_addr.to_string())
                    .or_insert_with(|| CanonicalBackendLifecycleSnapshot {
                        identity: BackendIdentity::new(backend_addr.to_string()),
                        resolution: BackendResolutionState {
                            authority_host: backend_addr.to_string(),
                            authority_port: 0,
                            address_kind: RuntimeBackendAddressKind::IpLiteral,
                            resolved_addrs: Vec::new(),
                            last_refresh_success_at: None,
                            refresh_generation: 0,
                        },
                        health: BackendHealthState::Unknown,
                        membership: BackendMembershipState::Removed,
                        placements: Vec::new(),
                    });
                entry.placements.push(BackendPoolPlacementSnapshot {
                    upstream_name: upstream_name.clone(),
                    backend_index,
                    healthy: backend.healthy,
                    active_requests: backend.active_requests,
                    ewma_latency_ms: backend.ewma_latency_ms,
                    membership_epoch: membership_summary.membership_epoch,
                });
            }
        }

        let mut backends = snapshots.into_values().collect::<Vec<_>>();
        for backend in &mut backends {
            if backend.placements.is_empty() {
                backend.membership = BackendMembershipState::Removed;
                continue;
            }

            backend.membership = BackendMembershipState::Active;
            backend.health = if backend.placements.iter().all(|placement| placement.healthy) {
                BackendHealthState::Healthy
            } else {
                BackendHealthState::Unhealthy { reason: None }
            };
            backend.placements.sort_by(|left, right| {
                left.upstream_name
                    .cmp(&right.upstream_name)
                    .then(left.backend_index.cmp(&right.backend_index))
            });
        }
        backends
            .sort_by(|left, right| left.identity.backend_addr.cmp(&right.identity.backend_addr));

        BackendLifecycleInventorySnapshot { backends }
    }

    pub(crate) fn apply_refresh(
        &self,
        backend: &RuntimeBackendLifecycleState,
        resolved_addrs: Result<Vec<std::net::SocketAddr>, String>,
        backend_dns_resolver: &SharedDnsResolver,
        transport_pool: &UpstreamTransportPool,
    ) -> BackendDnsRefreshApplication {
        apply_backend_dns_refresh(
            backend,
            resolved_addrs,
            self.resolution_store.as_ref(),
            backend_dns_resolver,
            transport_pool,
        )
    }

    pub(crate) fn apply_health_observation(
        &self,
        upstream_pool: Option<&Arc<RwLock<UpstreamPool>>>,
        backend_index: Option<usize>,
        observation: &event::BackendHealthObservation,
    ) -> Option<HealthTransition> {
        apply_backend_health_observation(upstream_pool, backend_index, observation)
    }
}
