use crate::backend_pool::BackendPool;

pub const DEFAULT_REPLICAS: u32 = 64;
pub const FNV_OFFSET: u64 = 0xcbf29ce484222325;
pub const FNV_PRIME: u64 = 0x00000100000001b3;

pub fn expected_ring_entries(pool: &BackendPool, replicas: u32) -> usize {
    pool.healthy
        .iter()
        .map(|&idx| replicas.saturating_mul(pool.backends[idx].weight()) as usize)
        .sum()
}

pub fn hash_backend_replica(address: &str, replica: u32) -> u64 {
    let mut hash = FNV_OFFSET;
    for &byte in address.as_bytes() {
        hash = hash64_update(hash, byte);
    }
    hash = hash64_update(hash, b'-');

    let mut digits = [0u8; 10];
    let mut value = replica;
    let mut cursor = digits.len();
    loop {
        cursor -= 1;
        digits[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }

    for &digit in &digits[cursor..] {
        hash = hash64_update(hash, digit);
    }

    hash
}

pub fn hash64(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in data {
        hash = hash64_update(hash, *byte);
    }
    hash
}

pub fn hash64_update(hash: u64, byte: u8) -> u64 {
    (hash ^ byte as u64).wrapping_mul(FNV_PRIME)
}

#[cfg(test)]
mod tests {
    use super::{FNV_OFFSET, hash_backend_replica, hash64, hash64_update};

    #[test]
    fn hash64_matches_incremental_update_for_same_key_material() {
        let data = b"tenant=acme:path=/api/v1/orders";
        let incremental = data.iter().copied().fold(FNV_OFFSET, hash64_update);

        assert_eq!(hash64(data), incremental);
    }

    #[test]
    fn hash_backend_replica_is_stable_and_replica_sensitive() {
        let first = hash_backend_replica("http://backend-a:8080", 12);
        let second = hash_backend_replica("http://backend-a:8080", 12);
        let different_replica = hash_backend_replica("http://backend-a:8080", 13);
        let different_backend = hash_backend_replica("http://backend-b:8080", 12);

        assert_eq!(first, second);
        assert_ne!(first, different_replica);
        assert_ne!(first, different_backend);
    }
}
