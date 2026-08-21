//! Stable-hash helpers used for load-balancing key derivation.

use core::net::SocketAddr;

use impulse_edge::{stable_hash_socket_addr, stable_hash64};

#[test]
fn stable_hash64_is_deterministic() {
    let first = stable_hash64(b"/api/orders");
    let second = stable_hash64(b"/api/orders");
    assert_eq!(first, second);
}

#[test]
fn stable_hash_socket_addr_distinguishes_addresses() {
    let addr_one: SocketAddr = "127.0.0.1:9889".parse().expect("addr one");
    let addr_two: SocketAddr = "127.0.0.2:9889".parse().expect("addr two");

    assert_ne!(
        stable_hash_socket_addr(&addr_one),
        stable_hash_socket_addr(&addr_two)
    );
}
