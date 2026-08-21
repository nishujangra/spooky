use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::SystemTime,
};

use impulse_edge::runtime::backend::{
    event::{BackendLifecycleMutation, BackendRefreshOutcome},
    resolution::{RuntimeBackendAddressKind, RuntimeBackendResolution},
    store::RuntimeBackendResolutionStore,
};

#[test]
fn hostname_entries_exclude_ip_literal_backends() {
    let store = RuntimeBackendResolutionStore::new([
        RuntimeBackendResolution::hostname(
            "api.internal:443".to_string(),
            "api.internal".to_string(),
            443,
        ),
        RuntimeBackendResolution::ip_literal(
            "10.0.0.10:8443".to_string(),
            "10.0.0.10".to_string(),
            8443,
            vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10)),
                8443,
            )],
        ),
    ]);

    let entries = store.hostname_backends();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].identity.backend_addr, "api.internal:443");
    assert_eq!(
        entries[0].resolution.address_kind,
        RuntimeBackendAddressKind::Hostname
    );
}

#[test]
fn store_snapshot_preserves_seeded_resolution_state() {
    let store = RuntimeBackendResolutionStore::new([RuntimeBackendResolution::ip_literal(
        "127.0.0.1:8080".to_string(),
        "127.0.0.1".to_string(),
        8080,
        vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)],
    )]);

    let snapshot = store.snapshot();
    let entry = snapshot.get("127.0.0.1:8080").expect("entry");
    assert_eq!(entry.resolution.authority_host, "127.0.0.1");
    assert_eq!(entry.resolution.authority_port, 8080);
    assert_eq!(entry.resolution.resolved_addrs.len(), 1);
}

#[test]
fn hostname_resolution_update_canonicalizes_and_tracks_generation() {
    let store = RuntimeBackendResolutionStore::new([RuntimeBackendResolution::hostname(
        "api.internal:443".to_string(),
        "api.internal".to_string(),
        443,
    )]);

    let mutation = store
        .apply_resolution_refresh(
            "api.internal:443",
            vec![
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 443),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 443),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 443),
            ],
            SystemTime::UNIX_EPOCH,
        )
        .expect("update");

    match mutation {
        BackendLifecycleMutation::ResolutionUpdated { result, .. } => match result.outcome {
            BackendRefreshOutcome::Updated {
                current_addrs,
                refresh_generation,
                ..
            } => {
                assert_eq!(refresh_generation, 1);
                assert_eq!(
                    current_addrs,
                    vec![
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 443),
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 443)
                    ]
                );
            }
            other => panic!("expected updated refresh outcome, got {other:?}"),
        },
        other => panic!("expected resolution update mutation, got {other:?}"),
    }
}
