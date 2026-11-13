//! FNV-1a, for hashes that end up in files (traces, cached scripts, coverage
//! tables): a fixed algorithm, so they are the same across Rust versions,
//! unlike `DefaultHasher`'s. See:
//! https://en.wikipedia.org/wiki/Fowler%E2%80%93Noll%E2%80%93Vo_hash_function

pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

pub fn fnv1a64(bytes: impl IntoIterator<Item = u8>) -> u64 {
    fnv1a64_from(FNV_OFFSET, bytes)
}

/// Continues the hash `hash` with `bytes`.
pub fn fnv1a64_from(mut hash: u64, bytes: impl IntoIterator<Item = u8>) -> u64 {
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_known_vectors() {
        assert_eq!(fnv1a64([]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(*b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(*b"foobar"), 0x8594_4171_f739_67e8);
    }
}
