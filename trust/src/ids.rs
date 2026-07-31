//! Identifiers, timestamps, and hashing. One dialect everywhere (appendix A).

use rand::RngCore;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch. All internal timestamps use this form.
pub fn ts_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `n_bytes` of cryptographic randomness, hex-encoded.
pub fn random_hex(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// A prefixed opaque id, e.g. `int_9f2a...`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", random_hex(8))
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// The raw digest. Hex is for logs; protocols that hash into base64url
/// (PKCE, for one) need the bytes.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Fill a buffer with cryptographic randomness.
pub fn fill_random(buf: &mut [u8]) {
    rand::thread_rng().fill_bytes(buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_prefixed() {
        let a = new_id("int");
        let b = new_id("int");
        assert!(a.starts_with("int_") && b.starts_with("int_"));
        assert_ne!(a, b);
    }

    #[test]
    fn sha256_is_stable() {
        assert_eq!(
            sha256_hex(b"bender"),
            sha256_hex(b"bender"),
        );
        assert_ne!(sha256_hex(b"bender"), sha256_hex(b"akita"));
    }
}
