use std::{net::SocketAddr, time::SystemTime};

use crate::runtime::backend::{
    event::BackendRefreshResult,
    resolution::RuntimeBackendAddressKind,
    state::{BackendIdentity, BackendResolutionState},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBackendResolutionUpdate {
    pub backend_addr: String,
    pub authority_host: String,
    pub authority_port: u16,
    pub address_kind: RuntimeBackendAddressKind,
    pub previous_addrs: Vec<SocketAddr>,
    pub current_addrs: Vec<SocketAddr>,
    pub last_refresh_success_at: Option<SystemTime>,
    pub refresh_generation: u64,
}

impl RuntimeBackendResolutionUpdate {
    pub fn changed(&self) -> bool {
        self.previous_addrs != self.current_addrs
    }

    pub fn cleared(&self) -> bool {
        self.current_addrs.is_empty()
    }

    pub fn identity(&self) -> BackendIdentity {
        BackendIdentity::new(self.backend_addr.clone())
    }

    pub fn resolution_state(&self) -> BackendResolutionState {
        BackendResolutionState {
            authority_host: self.authority_host.clone(),
            authority_port: self.authority_port,
            address_kind: self.address_kind,
            resolved_addrs: self.current_addrs.clone(),
            last_refresh_success_at: self.last_refresh_success_at,
            refresh_generation: self.refresh_generation,
        }
    }

    pub fn refresh_result(&self) -> BackendRefreshResult {
        BackendRefreshResult::from(self)
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::SystemTime};

    use super::*;
    use crate::runtime::backend::{
        event::BackendRefreshOutcome, resolution::RuntimeBackendAddressKind,
    };

    fn sample_update() -> RuntimeBackendResolutionUpdate {
        RuntimeBackendResolutionUpdate {
            backend_addr: "https://backend.internal:8443".to_string(),
            authority_host: "backend.internal".to_string(),
            authority_port: 8443,
            address_kind: RuntimeBackendAddressKind::Hostname,
            previous_addrs: vec!["10.0.0.10:8443".parse::<SocketAddr>().expect("addr")],
            current_addrs: vec!["10.0.0.11:8443".parse::<SocketAddr>().expect("addr")],
            last_refresh_success_at: Some(SystemTime::UNIX_EPOCH),
            refresh_generation: 3,
        }
    }

    #[test]
    fn resolution_update_preserves_identity_generation_and_current_resolution() {
        let update = sample_update();

        assert!(update.changed());
        assert!(!update.cleared());
        assert_eq!(update.identity().backend_addr, update.backend_addr);

        let resolution = update.resolution_state();
        assert_eq!(resolution.authority_host, "backend.internal");
        assert_eq!(resolution.authority_port, 8443);
        assert_eq!(resolution.resolved_addrs, update.current_addrs);
        assert_eq!(resolution.refresh_generation, 3);
    }

    #[test]
    fn resolution_update_refresh_result_marks_unchanged_without_rebuilding_identity() {
        let mut update = sample_update();
        update.current_addrs = update.previous_addrs.clone();

        let result = update.refresh_result();

        assert_eq!(result.identity.backend_addr, update.backend_addr);
        assert!(matches!(
            result.outcome,
            BackendRefreshOutcome::Unchanged {
                refresh_generation: 3,
                ..
            }
        ));
    }
}
