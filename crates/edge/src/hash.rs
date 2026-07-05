use core::net::SocketAddr;
use std::sync::atomic::AtomicU64;

pub(crate) static REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
const FNV_OFFSET_BASIS_64: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME_64: u64 = 0x0000_0100_0000_01b3;

pub fn stable_hash64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS_64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME_64);
    }
    hash
}

pub fn stable_hash_socket_addr(addr: &SocketAddr) -> u64 {
    match addr {
        SocketAddr::V4(v4) => {
            let mut bytes = [0u8; 7];
            bytes[0] = 4;
            bytes[1..5].copy_from_slice(&v4.ip().octets());
            bytes[5..7].copy_from_slice(&v4.port().to_be_bytes());
            stable_hash64(&bytes)
        }
        SocketAddr::V6(v6) => {
            let mut bytes = [0u8; 31];
            bytes[0] = 6;
            bytes[1..17].copy_from_slice(&v6.ip().octets());
            bytes[17..19].copy_from_slice(&v6.port().to_be_bytes());
            bytes[19..23].copy_from_slice(&v6.flowinfo().to_be_bytes());
            bytes[23..27].copy_from_slice(&v6.scope_id().to_be_bytes());
            stable_hash64(&bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn test_stable_hash64_empty() {
        let data = b"";
        let hash = stable_hash64(data);
        // FNV-1a empty debería devolver el valor base
        assert_eq!(hash, 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn test_stable_hash64_hello() {
        let data = b"hello";
        let hash = stable_hash64(data);
        // Valor conocido para "hello" con FNV-1a
        assert_eq!(hash, 0xa430_d846_80aa_bd0b);
    }

    #[test]
    fn test_stable_hash64_consistent() {
        let data1 = b"test";
        let data2 = b"test";
        assert_eq!(stable_hash64(data1), stable_hash64(data2));
    }

    #[test]
    fn test_stable_hash64_different() {
        let data1 = b"test";
        let data2 = b"test2";
        assert_ne!(stable_hash64(data1), stable_hash64(data2));
    }

    #[test]
    fn test_stable_hash64_large_data() {
        let data = b"this is a longer string to test the hash function with more bytes";
        let hash = stable_hash64(data);
        assert!(hash > 0);
    }

    #[test]
    fn test_stable_hash_socket_addr_ipv4() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let hash = stable_hash_socket_addr(&addr);
        assert!(hash > 0);
    }

    #[test]
    fn test_stable_hash_socket_addr_ipv6() {
        let addr: SocketAddr = "[::1]:8080".parse().unwrap();
        let hash = stable_hash_socket_addr(&addr);
        assert!(hash > 0);
    }

    #[test]
    fn test_stable_hash_socket_addr_ipv4_loopback() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let hash = stable_hash_socket_addr(&addr);
        assert!(hash > 0);
    }

    #[test]
    fn test_stable_hash_socket_addr_ipv6_loopback() {
        let addr: SocketAddr = "[::1]:0".parse().unwrap();
        let hash = stable_hash_socket_addr(&addr);
        assert!(hash > 0);
    }

    #[test]
    fn test_stable_hash_socket_addr_consistent() {
        let addr1: SocketAddr = "192.168.1.1:9000".parse().unwrap();
        let addr2: SocketAddr = "192.168.1.1:9000".parse().unwrap();
        assert_eq!(
            stable_hash_socket_addr(&addr1),
            stable_hash_socket_addr(&addr2)
        );
    }

    #[test]
    fn test_stable_hash_socket_addr_different_ip() {
        let addr1: SocketAddr = "192.168.1.1:9000".parse().unwrap();
        let addr2: SocketAddr = "192.168.1.2:9000".parse().unwrap();
        assert_ne!(
            stable_hash_socket_addr(&addr1),
            stable_hash_socket_addr(&addr2)
        );
    }

    #[test]
    fn test_stable_hash_socket_addr_different_port() {
        let addr1: SocketAddr = "192.168.1.1:9000".parse().unwrap();
        let addr2: SocketAddr = "192.168.1.1:9001".parse().unwrap();
        assert_ne!(
            stable_hash_socket_addr(&addr1),
            stable_hash_socket_addr(&addr2)
        );
    }

    #[test]
    fn test_stable_hash_socket_addr_ipv4_mapped() {
        let addr1: SocketAddr = "[::ffff:127.0.0.1]:8080".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        // IPv6 mapeado debería ser equivalente a IPv4
        assert_eq!(
            stable_hash_socket_addr(&addr1),
            stable_hash_socket_addr(&addr2)
        );
    }

    #[test]
    fn test_stable_hash_socket_addr_ipv4_different_ips() {
        let addr1: SocketAddr = "10.0.0.1:8080".parse().unwrap();
        let addr2: SocketAddr = "10.0.0.2:8080".parse().unwrap();
        assert_ne!(
            stable_hash_socket_addr(&addr1),
            stable_hash_socket_addr(&addr2)
        );
    }

    #[test]
    fn test_stable_hash_socket_addr_ipv6_different_ips() {
        let addr1: SocketAddr = "[2001:db8::1]:8080".parse().unwrap();
        let addr2: SocketAddr = "[2001:db8::2]:8080".parse().unwrap();
        assert_ne!(
            stable_hash_socket_addr(&addr1),
            stable_hash_socket_addr(&addr2)
        );
    }
}