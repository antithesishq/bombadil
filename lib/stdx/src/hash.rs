//! FNV-1a, for hashes that end up in files (traces, cached scripts, coverage
//! tables): a fixed algorithm, so they are the same across Rust versions,
//! unlike `DefaultHasher`'s. See:
//! https://en.wikipedia.org/wiki/Fowler%E2%80%93Noll%E2%80%93Vo_hash_function

use std::hash::Hasher;

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

/// FNV-1a, with integers written little-endian and `usize` as 64 bits, so
/// that hashes are the same on every platform. What a value writes still
/// comes from std's `Hash` impls (string terminators, length prefixes),
/// which Rust does not promise to keep.
pub struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        StableHasher(FNV_OFFSET)
    }
}

impl Hasher for StableHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0 = fnv1a64_from(self.0, bytes.iter().copied());
    }

    fn write_u16(&mut self, value: u16) {
        self.write(&value.to_le_bytes());
    }

    fn write_u32(&mut self, value: u32) {
        self.write(&value.to_le_bytes());
    }

    fn write_u64(&mut self, value: u64) {
        self.write(&value.to_le_bytes());
    }

    fn write_u128(&mut self, value: u128) {
        self.write(&value.to_le_bytes());
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    fn write_isize(&mut self, value: isize) {
        self.write_u64(value as i64 as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::TestCase;
    use hegel::generators::integers;
    use std::hash::Hash;

    #[test]
    fn fnv1a64_known_vectors() {
        assert_eq!(fnv1a64([]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(*b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(*b"foobar"), 0x8594_4171_f739_67e8);
    }

    fn stable_hash(value: impl Hash) -> u64 {
        let mut hasher = StableHasher::default();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[hegel::test]
    fn stable_hasher_writes_integers_little_endian(tc: TestCase) {
        let value = tc.draw(integers::<u64>());
        let bytes = value.to_le_bytes();
        assert_eq!(stable_hash(value), fnv1a64(bytes));
        assert_eq!(stable_hash(value as u32), fnv1a64(bytes[..4].to_vec()));
        assert_eq!(stable_hash(value as u16), fnv1a64(bytes[..2].to_vec()));
        assert_eq!(
            stable_hash(value as usize),
            stable_hash(value as usize as u64)
        );
        assert_eq!(
            stable_hash(value as isize),
            stable_hash(value as isize as i64)
        );
    }
}
