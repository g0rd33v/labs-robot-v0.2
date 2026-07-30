//! Encrypted SQLite cells (arch sec 2/2a): one SQLCipher file per principal.

use crate::TrustError;
use rusqlite::Connection;
use std::io::Read;
use std::path::Path;

/// Open (or create) an encrypted database at `path` with a raw 32-byte key.
/// Fails if the key is wrong for an existing file.
pub fn open_encrypted(path: &Path, key: &[u8; 32]) -> Result<Connection, TrustError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    let hexkey = hex::encode(key);
    // PRAGMA key must be the first statement on the connection.
    conn.execute_batch(&format!("PRAGMA key = \"x'{hexkey}'\";"))?;
    // First real read fails fast on a wrong key (NotADatabase).
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))?;
    let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(conn)
}

/// True when the file does not start with the plaintext SQLite magic --
/// i.e. it is opaque without its key. The M1 encryption check.
pub fn file_looks_encrypted(path: &Path) -> Result<bool, TrustError> {
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 16];
    let n = f.read(&mut head)?;
    if n < 16 {
        return Ok(true); // empty/truncated file is not plaintext SQLite
    }
    Ok(head != *b"SQLite format 3\0")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::KeyChain;

    fn temp_db(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("trust-cell-{}-{name}.db", crate::ids::random_hex(6)))
    }

    #[test]
    fn cell_encrypts_persists_and_rejects_wrong_key() {
        let path = temp_db("a");
        let key = KeyChain::new_dek();

        let conn = open_encrypted(&path, &key).unwrap();
        conn.execute_batch("CREATE TABLE t(x TEXT); INSERT INTO t VALUES('secret');")
            .unwrap();
        drop(conn);

        // on-disk bytes are opaque
        assert!(file_looks_encrypted(&path).unwrap());

        // right key reads back
        let conn = open_encrypted(&path, &key).unwrap();
        let x: String = conn
            .query_row("SELECT x FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(x, "secret");
        drop(conn);

        // wrong key must fail
        let wrong = KeyChain::new_dek();
        assert!(open_encrypted(&path, &wrong).is_err());

        let _ = std::fs::remove_file(&path);
    }
}
