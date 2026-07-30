//! The media vault (arch sec 4a): content-addressed originals beside the
//! cells (`media/ab/cdef...`), encrypted at rest like everything else;
//! references and metadata live in the cell.

use crate::MindError;
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::path::{Path, PathBuf};

pub struct MediaVault {
    root: PathBuf,
    key: [u8; 32],
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaRef {
    pub hash: String,
    pub mime: Option<String>,
    pub size: i64,
    pub created_at: i64,
    pub pinned: bool,
}

impl MediaVault {
    /// `key` should be derived from the cell's DEK (e.g.
    /// `trust::keys::derive_key(&dek, b"media")`), so vault bytes shred with
    /// the cell.
    pub fn new(root: impl Into<PathBuf>, key: [u8; 32]) -> Self {
        Self {
            root: root.into(),
            key,
        }
    }

    fn path_for(&self, hash: &str) -> PathBuf {
        self.root.join(&hash[..2]).join(&hash[2..])
    }

    /// Store bytes content-addressed by the sha256 of the *plaintext*;
    /// identical content stores once. The on-disk file is sealed.
    pub fn store(
        &self,
        conn: &Connection,
        bytes: &[u8],
        mime: Option<&str>,
        source: Option<&str>,
    ) -> Result<MediaRef, MindError> {
        let hash = trust::ids::sha256_hex(bytes);
        let path = self.path_for(&hash);
        if !path.exists() {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(|e| MindError::Vault(e.to_string()))?;
            }
            let sealed = trust::keys::seal_bytes(&self.key, bytes)
                .map_err(|e| MindError::Vault(e.to_string()))?;
            fs::write(&path, sealed).map_err(|e| MindError::Vault(e.to_string()))?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO media(hash, mime, size, created_at, source) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![hash, mime, bytes.len() as i64, trust::ids::ts_ms(), source],
        )?;
        self.get_ref(conn, &hash)?
            .ok_or_else(|| MindError::Vault("media row missing after insert".into()))
    }

    /// Read back and unseal by content hash.
    pub fn get(&self, conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>, MindError> {
        if self.get_ref(conn, hash)?.is_none() {
            return Ok(None);
        }
        let path = self.path_for(hash);
        if !path.exists() {
            return Ok(None);
        }
        let sealed = fs::read(&path).map_err(|e| MindError::Vault(e.to_string()))?;
        let plain = trust::keys::open_bytes(&self.key, &sealed)
            .map_err(|e| MindError::Vault(e.to_string()))?;
        // content-address integrity: the bytes must hash back to their name
        if trust::ids::sha256_hex(&plain) != hash {
            return Err(MindError::Vault("vault integrity: hash mismatch".into()));
        }
        Ok(Some(plain))
    }

    pub fn get_ref(&self, conn: &Connection, hash: &str) -> Result<Option<MediaRef>, MindError> {
        Ok(conn
            .query_row(
                "SELECT hash, mime, size, created_at, pinned FROM media WHERE hash = ?1",
                params![hash],
                |r| {
                    Ok(MediaRef {
                        hash: r.get(0)?,
                        mime: r.get(1)?,
                        size: r.get(2)?,
                        created_at: r.get(3)?,
                        pinned: r.get::<_, i64>(4)? != 0,
                    })
                },
            )
            .optional()?)
    }

    pub fn list(&self, conn: &Connection) -> Result<Vec<MediaRef>, MindError> {
        let mut stmt = conn.prepare(
            "SELECT hash, mime, size, created_at, pinned FROM media ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(MediaRef {
                    hash: r.get(0)?,
                    mime: r.get(1)?,
                    size: r.get(2)?,
                    created_at: r.get(3)?,
                    pinned: r.get::<_, i64>(4)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Connection, MediaVault, PathBuf) {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_cell_schema(&conn).unwrap();
        let root = std::env::temp_dir().join(format!("vault-{}", trust::ids::random_hex(6)));
        let vault = MediaVault::new(&root, trust::keys::KeyChain::new_dek());
        (conn, vault, root)
    }

    #[test]
    fn store_get_roundtrip_encrypted_and_deduped() {
        let (conn, vault, root) = setup();
        let payload = b"a voice note, allegedly".to_vec();
        let a = vault.store(&conn, &payload, Some("audio/ogg"), Some("chat")).unwrap();
        let b = vault.store(&conn, &payload, Some("audio/ogg"), Some("chat")).unwrap();
        assert_eq!(a.hash, b.hash); // content-addressed dedup
        assert_eq!(vault.list(&conn).unwrap().len(), 1);

        // on-disk bytes are sealed, not the plaintext
        let on_disk = std::fs::read(root.join(&a.hash[..2]).join(&a.hash[2..])).unwrap();
        assert!(!on_disk
            .windows(payload.len())
            .any(|w| w == payload.as_slice()));

        // reads back exactly
        assert_eq!(vault.get(&conn, &a.hash).unwrap().unwrap(), payload);
        // unknown hash is None
        assert!(vault.get(&conn, &"0".repeat(64)).unwrap().is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
