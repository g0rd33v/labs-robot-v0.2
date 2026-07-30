//! mind: the epistemic memory substrate (arch sec 4).
//!
//! M2 ships the message store and the reminders ledger (the first
//! commitment-ledger entries). Facts with source FKs, FTS5 + sqlite-vec
//! retrieval, and the Registry arrive with M3.

pub mod reminders;

use rusqlite::{params, Connection};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MindError {
    #[error("sql: {0}")]
    Sql(#[from] rusqlite::Error),
}

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
CREATE TABLE IF NOT EXISTS cell_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
",
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
}
