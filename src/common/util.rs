/// Round to 6 decimal places.
pub fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

/// FNV-1a 64-bit hash. Implemented inline (13 lines) rather than pulled from a
/// crate so the algorithm and constants are frozen next to the test vectors
/// that pin them. Used where hash values must stay stable across builds and
/// Rust releases: download cache filenames and place_id suffixes.
pub(crate) const fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_64_known_vectors() {
        // Standard FNV-1a test vectors (see http://isthe.com/chongo/tech/comp/fnv/).
        // Pinning these ensures cache filenames and place_id hash suffixes stay
        // stable across versions of this tool and any future edits to the hash.
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn test_round6() {
        assert_eq!(round6(59.912345678), 59.912346);
        assert_eq!(round6(10.0), 10.0);
        assert_eq!(round6(0.1234565), 0.123457); // rounds up
        assert_eq!(round6(0.1234564), 0.123456); // rounds down
    }

    #[test]
    fn test_round6_negative() {
        assert_eq!(round6(-10.123456789), -10.123457);
    }
}
