//! mind: the epistemic memory substrate (arch sec 4).
//!
//! M3 ships: facts with source-FK provenance (law #5 as schema), FTS5 +
//! sqlite-vec hybrid recall fused with RRF (Q20), the content-addressed
//! encrypted media vault (sec 4a), and Registry-lite (sec 4b): list with
//! sources, correct (supersession), forget (deletes for real).

pub mod facts;
pub mod reminders;
pub mod vault;

use rusqlite::{params, Connection};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MindError {
    #[error("sql: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("vault: {0}")]
    Vault(String),
}

/// Register sqlite-vec for every future connection in this process. Safe to
/// call more than once. Must run before cells are opened if the vector door
/// is wanted on them.
pub fn install_vec() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        type InitFn = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        let f: InitFn = std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
        rusqlite::ffi::sqlite3_auto_extension(Some(f));
    });
}

/// Embedding dimension for the vector table (bge-m3-class seat: e5-small,
/// 384-d). A model change means re-index with a new table (Q24).
pub const EMBED_DIM: usize = 384;

/// Per-cell memory tables. Idempotent.
pub fn init_cell_schema(conn: &Connection) -> Result<(), MindError> {
    conn.execute_batch(
        "
CREATE TABLE IF NOT EXISTS messages (
    id        TEXT PRIMARY KEY,
    ts        INTEGER NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('in','out')),
    surface   TEXT NOT NULL,
    lang      TEXT,
    content   TEXT NOT NULL,
    media_ref TEXT
);
CREATE TABLE IF NOT EXISTS reminders (
    id         TEXT PRIMARY KEY,
    intent_id  TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    fire_at    INTEGER NOT NULL,
    about      TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'active'
               CHECK (status IN ('active','cancelled','fired'))
);
CREATE TABLE IF NOT EXISTS facts (
    id            TEXT PRIMARY KEY,
    entity        TEXT,
    content       TEXT NOT NULL,
    source_msg_id TEXT NOT NULL REFERENCES messages(id),
    intent_id     TEXT UNIQUE,
    status        TEXT NOT NULL DEFAULT 'stable'
                  CHECK (status IN ('tentative','contextual','stable','contested','superseded')),
    confidence    REAL NOT NULL DEFAULT 1.0,
    created_at    INTEGER NOT NULL,
    superseded_by TEXT REFERENCES facts(id)
);
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
    content, content='facts', content_rowid='rowid'
);
CREATE TRIGGER IF NOT EXISTS facts_ai AFTER INSERT ON facts BEGIN
    INSERT INTO facts_fts(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER IF NOT EXISTS facts_ad AFTER DELETE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
END;
CREATE TRIGGER IF NOT EXISTS facts_au AFTER UPDATE OF content ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
    INSERT INTO facts_fts(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TABLE IF NOT EXISTS media (
    hash       TEXT PRIMARY KEY,
    mime       TEXT,
    size       INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    pinned     INTEGER NOT NULL DEFAULT 0,
    source     TEXT
);
CREATE TABLE IF NOT EXISTS cell_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
",
    )?;

    // the vector door is optional equipment: present when sqlite-vec is
    // installed in this process, absent (and recall degrades to FTS+recency)
    // when it is not
    let vec_ready = conn
        .execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec USING vec0(embedding float[{EMBED_DIM}] distance_metric=cosine);"
        ))
        .is_ok();
    conn.execute(
        "INSERT INTO cell_meta(key, value) VALUES ('vec_ready', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![if vec_ready { "1" } else { "0" }],
    )?;
    Ok(())
}

/// Record one message verbatim in its source language (arch sec 2d: memory
/// keeps the person's actual words). Returns the message id.
pub fn record_message(
    conn: &Connection,
    direction: &str,
    surface: &str,
    content: &str,
) -> Result<String, MindError> {
    let id = trust::ids::new_id("msg");
    conn.execute(
        "INSERT INTO messages(id, ts, direction, surface, content) VALUES (?1,?2,?3,?4,?5)",
        params![id, trust::ids::ts_ms(), direction, surface, content],
    )?;
    Ok(id)
}

/// Number of stored messages. Used by tests and gate demos.
pub fn message_count(conn: &Connection) -> Result<i64, MindError> {
    Ok(conn.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        init_cell_schema(&conn).unwrap();
        record_message(&conn, "in", "chat", "привет, робот").unwrap();
        record_message(&conn, "out", "chat", "hello back").unwrap();
        assert_eq!(message_count(&conn).unwrap(), 2);
        // stored verbatim, source language intact
        let content: String = conn
            .query_row(
                "SELECT content FROM messages WHERE direction = 'in'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(content, "привет, робот");
    }

    #[test]
    fn vec_door_activates_when_extension_installed() {
        install_vec();
        let conn = Connection::open_in_memory().unwrap();
        init_cell_schema(&conn).unwrap();
        assert!(facts::vec_available(&conn), "vec0 should be available");
        // and a vector roundtrip works end to end
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let m = record_message(&conn, "in", "chat", "remember i like sailing").unwrap();
        let emb: Vec<f32> = (0..EMBED_DIM).map(|i| (i as f32).sin()).collect();
        facts::remember(&conn, "i like sailing", &m, "int_v", Some(&emb)).unwrap();
        let found = facts::recall(&conn, "sailing", Some(&emb), 5).unwrap();
        assert_eq!(found[0].content, "i like sailing");
    }
}
