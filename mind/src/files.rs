//! Files: names over vault content.
//!
//! The vault (§4a) is content-addressed — it knows bytes, not documents.
//! A *file* is the other half: a name a person chose, pointing at content,
//! carrying its own classification and its own provenance. Keeping the two
//! apart is what makes saving the same text under two names cost one copy,
//! and what makes renaming free.
//!
//! Overwriting a name is a new version, not a new file: the name is the
//! identity, so `save` twice under one name leaves one file whose content
//! moved. The superseded bytes stay in the vault, unreferenced and inert —
//! content-addressed storage has no way to know whether some other name
//! still wants them.

use crate::MindError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRef {
    pub id: String,
    pub name: String,
    pub hash: String,
    pub size: i64,
    pub class: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Point `name` at `hash`. Creates or re-versions; returns the file either way.
pub fn put(
    conn: &Connection,
    name: &str,
    hash: &str,
    size: i64,
    class: &str,
    source_msg_id: &str,
) -> Result<FileRef, MindError> {
    let now = trust::ids::ts_ms();
    match get(conn, name)? {
        Some(existing) => {
            conn.execute(
                "UPDATE files SET hash = ?1, size = ?2, class = ?3, updated_at = ?4 \
                 WHERE id = ?5",
                params![hash, size, class, now, existing.id],
            )?;
        }
        None => {
            conn.execute(
                "INSERT INTO files(id, name, hash, size, class, source_msg_id, \
                                   created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    trust::ids::new_id("fil"),
                    name,
                    hash,
                    size,
                    class,
                    source_msg_id,
                    now
                ],
            )?;
        }
    }
    get(conn, name)?.ok_or_else(|| MindError::Vault("file row missing after write".into()))
}

pub fn get(conn: &Connection, name: &str) -> Result<Option<FileRef>, MindError> {
    Ok(conn
        .query_row(
            "SELECT id, name, hash, size, class, created_at, updated_at \
             FROM files WHERE name = ?1",
            params![name],
            row,
        )
        .optional()?)
}

/// Newest change first — the order a person thinks in.
pub fn list(conn: &Connection) -> Result<Vec<FileRef>, MindError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, hash, size, class, created_at, updated_at \
         FROM files ORDER BY updated_at DESC",
    )?;
    let all = stmt.query_map([], row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(all)
}

/// Erase the name, and tombstone it so the erasure travels (a delete that
/// does not sync is a delete that comes back).
pub fn delete(conn: &Connection, name: &str, origin: &str) -> Result<bool, MindError> {
    let Some(f) = get(conn, name)? else {
        return Ok(false);
    };
    conn.execute("DELETE FROM files WHERE id = ?1", params![f.id])?;
    conn.execute(
        "INSERT OR IGNORE INTO tombstones(id, kind, deleted_at, origin) \
         VALUES (?1, 'file', ?2, ?3)",
        params![f.id, trust::ids::ts_ms(), origin],
    )?;
    Ok(true)
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FileRef> {
    Ok(FileRef {
        id: r.get(0)?,
        name: r.get(1)?,
        hash: r.get(2)?,
        size: r.get(3)?,
        class: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_cell_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO messages(id, ts, direction, surface, content) \
             VALUES ('m1', 1, 'in', 'web', 'save this')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media(hash, size, created_at) VALUES ('h1', 3, 1), ('h2', 5, 2)",
            [],
        )
        .unwrap();
        conn
    }

    /// The name is the identity. Saving twice under one name is a new
    /// version of one file, not two files.
    #[test]
    fn a_second_save_re_versions_rather_than_duplicating() {
        let c = cell();
        let a = put(&c, "notes.md", "h1", 3, "owner_private", "m1").unwrap();
        let b = put(&c, "notes.md", "h2", 5, "sensitive", "m1").unwrap();
        assert_eq!(a.id, b.id, "same file");
        assert_eq!(b.hash, "h2");
        assert_eq!(b.size, 5);
        assert_eq!(b.class, "sensitive");
        assert_eq!(list(&c).unwrap().len(), 1);
        assert_eq!(a.created_at, b.created_at, "created stays put");
    }

    /// Two names over identical bytes are two files and one copy -- the
    /// whole reason names and content are separate tables.
    #[test]
    fn two_names_may_share_one_content() {
        let c = cell();
        put(&c, "a.md", "h1", 3, "owner_private", "m1").unwrap();
        put(&c, "b.md", "h1", 3, "owner_private", "m1").unwrap();
        let all = list(&c).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].hash, all[1].hash);
    }

    /// A delete that leaves no tombstone is a delete the next sync undoes.
    #[test]
    fn deleting_leaves_a_tombstone_so_the_erasure_travels() {
        let c = cell();
        put(&c, "notes.md", "h1", 3, "owner_private", "m1").unwrap();
        assert!(delete(&c, "notes.md", "main").unwrap());
        assert!(list(&c).unwrap().is_empty());

        let n: i64 = c
            .query_row(
                "SELECT count(*) FROM tombstones WHERE kind = 'file'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        assert!(!delete(&c, "notes.md", "main").unwrap(), "already gone");
    }
}
