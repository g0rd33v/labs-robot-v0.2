//! Key hierarchy per decisions Q4: one master KEK wraps per-cell DEKs.
//!
//! MVP custody: the KEK lives in an auto-unlock keyfile (0600) beside the
//! data -- the Q4-sanctioned owner choice for an unattended local robot; the
//! trade (disk theft exposure) is stated in BUILD-LOG. Passphrase sealing /
//! OS keyring land with M6 hardening.

use crate::TrustError;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    Key, XChaCha20Poly1305, XNonce,
};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;

pub struct KeyChain {
    kek: [u8; KEY_LEN],
}

impl KeyChain {
    /// Load the KEK from `path`, or create it on first boot.
    pub fn load_or_create(path: &Path) -> Result<Self, TrustError> {
        if path.exists() {
            let text = fs::read_to_string(path)?;
            let bytes = hex::decode(text.trim())
                .map_err(|e| TrustError::KeyMaterial(format!("kek file: {e}")))?;
            let kek: [u8; KEY_LEN] = bytes
                .try_into()
                .map_err(|_| TrustError::KeyMaterial("kek must be 32 bytes".into()))?;
            Ok(Self { kek })
        } else {
            let mut kek = [0u8; KEY_LEN];
            rand::thread_rng().fill_bytes(&mut kek);
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(path, hex::encode(kek))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
            Ok(Self { kek })
        }
    }

    /// Domain-separated key derivation: sha256(KEK || label).
    fn derive(&self, label: &[u8]) -> [u8; KEY_LEN] {
        let mut h = Sha256::new();
        h.update(self.kek);
        h.update(label);
        h.finalize().into()
    }

    /// The key for core.db. Core's own key cannot be stored *in* core, so it
    /// is derived from the KEK; per-cell DEKs are random and wrapped (Q4/Q5).
    pub fn core_db_key(&self) -> [u8; KEY_LEN] {
        self.derive(b"core-db")
    }

    /// A fresh random data-encryption key for one cell.
    pub fn new_dek() -> [u8; KEY_LEN] {
        let mut dek = [0u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut dek);
        dek
    }

    /// AEAD-wrap a cell DEK under the KEK. Returns (nonce, ciphertext).
    pub fn wrap_dek(&self, dek: &[u8; KEY_LEN]) -> Result<(Vec<u8>, Vec<u8>), TrustError> {
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.derive(b"dek-wrap")));
        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = cipher
            .encrypt(XNonce::from_slice(&nonce), dek.as_slice())
            .map_err(|e| TrustError::Crypto(format!("wrap: {e}")))?;
        Ok((nonce.to_vec(), ct))
    }

    pub fn unwrap_dek(&self, nonce: &[u8], ct: &[u8]) -> Result<[u8; KEY_LEN], TrustError> {
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.derive(b"dek-wrap")));
        let pt = cipher
            .decrypt(
                XNonce::from_slice(nonce),
                ct,
            )
            .map_err(|e| TrustError::Crypto(format!("unwrap: {e}")))?;
        pt.try_into()
            .map_err(|_| TrustError::KeyMaterial("unwrapped dek must be 32 bytes".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("trust-keys-{}-{name}", crate::ids::random_hex(6)))
    }

    #[test]
    fn kek_roundtrips_across_loads() {
        let p = temp_path("kek.key");
        let a = KeyChain::load_or_create(&p).unwrap();
        let b = KeyChain::load_or_create(&p).unwrap();
        assert_eq!(a.core_db_key(), b.core_db_key());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn dek_wrap_unwrap_roundtrips() {
        let p = temp_path("kek2.key");
        let kc = KeyChain::load_or_create(&p).unwrap();
        let dek = KeyChain::new_dek();
        let (nonce, ct) = kc.wrap_dek(&dek).unwrap();
        assert_eq!(kc.unwrap_dek(&nonce, &ct).unwrap(), dek);
        // tampered ciphertext must not unwrap
        let mut bad = ct.clone();
        bad[0] ^= 0xff;
        assert!(kc.unwrap_dek(&nonce, &bad).is_err());
        let _ = std::fs::remove_file(p);
    }
}
