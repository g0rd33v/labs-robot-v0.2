//! Core-file schema (decisions Q5): principals, cell keys, boundary log,
//! core journal, instance meta. Member content never lives here.

use crate::{ids, TrustError};
use rusqlite::{params, Connection, OptionalExtension};

const CORE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS principals (
    id           INTEGER PRIMARY KEY,
    kind         TEXT NOT NULL CHECK (kind IN ('owner','member','guest')),
    display_name TEXT NOT NULL,
    cell_id      TEXT NOT NULL UNIQUE,
    status       TEXT NOT NULL DEFAULT 'active',
    created_at   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS cell_keys (
    cell_id     TEXT PRIMARY KEY,
    wrapped_dek BLOB NOT NULL,
    nonce       BLOB NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS boundary_log (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts           INTEGER NOT NULL,
    direction    TEXT NOT NULL CHECK (direction IN ('in','out')),
    channel      TEXT NOT NULL,
    counterparty TEXT NOT NULL,
    purpose      TEXT NOT NULL,
    categories   TEXT NOT NULL DEFAULT '',
    payload_hash TEXT NOT NULL,
    size         INTEGER NOT NULL,
    trust_tag    TEXT NOT NULL,
    prev_hash    TEXT NOT NULL,
    entry_hash   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS core_journal (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts           INTEGER NOT NULL,
    kind         TEXT NOT NULL,
    payload_json TEXT NOT NULL
);
-- The meter (sec 2b): one row per model call, so cost is a measurement
-- rather than an estimate. Token counts and the provider's own cost
-- figure; no content -- the boundary log carries the hashes, this carries
-- the arithmetic. Core-file because cost is an instance concern: the
-- operator pays one bill, not one per cell.
CREATE TABLE IF NOT EXISTS model_calls (
    seq               INTEGER PRIMARY KEY AUTOINCREMENT,
    ts                INTEGER NOT NULL,
    role              TEXT NOT NULL,
    model             TEXT NOT NULL,
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    cached_tokens     INTEGER NOT NULL DEFAULT 0,
    cost_usd          REAL,
    latency_ms        INTEGER NOT NULL,
    first_token_ms    INTEGER
);
CREATE TABLE IF NOT EXISTS invites (
    token_hash TEXT PRIMARY KEY,
    role       TEXT NOT NULL DEFAULT 'member',
    created_at INTEGER NOT NULL,
    used_by    INTEGER
);
-- The boundary log is append-only by construction, not by convention: the
-- hash chain makes edits detectable, and these make them fail outright.
-- An attacker with the database must now also alter the schema, which is
-- itself a visible change.
CREATE TABLE IF NOT EXISTS chain_anchors (
    seq          INTEGER PRIMARY KEY,
    hash         TEXT NOT NULL,
    ts           INTEGER NOT NULL,
    published_to TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS boundary_log_no_update
BEFORE UPDATE ON boundary_log BEGIN
    SELECT RAISE(ABORT, 'boundary_log is append-only');
END;
CREATE TRIGGER IF NOT EXISTS boundary_log_no_delete
BEFORE DELETE ON boundary_log BEGIN
    SELECT RAISE(ABORT, 'boundary_log is append-only');
END;
";

pub fn init_core(conn: &Connection) -> Result<(), TrustError> {
    conn.execute_batch(CORE_SCHEMA)?;
    // CREATE TABLE IF NOT EXISTS never adds a column to an existing table
    // (the lesson the cells learned as "no such column: class", now applied
    // here BEFORE it bites): every column the core schema gains after first
    // ship must also be added additively.
    let additions: &[(&str, &str, &str)] = &[("model_calls", "first_token_ms", "INTEGER")];
    for (table, column, ddl) in additions {
        let present = conn
            .prepare(&format!("PRAGMA table_info({table})"))?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|c| c == column);
        if !present {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {ddl};"))?;
        }
    }
    Ok(())
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<(), TrustError> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>, TrustError> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .optional()?)
}

/// System/shared actions journal (per Q10: core journal beside per-cell journals).
pub fn core_journal(conn: &Connection, kind: &str, payload_json: &str) -> Result<(), TrustError> {
    conn.execute(
        "INSERT INTO core_journal(ts, kind, payload_json) VALUES (?1, ?2, ?3)",
        params![ids::ts_ms(), kind, payload_json],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        init_core(&conn).unwrap();
        assert_eq!(meta_get(&conn, "robot_id").unwrap(), None);
        meta_set(&conn, "robot_id", "robot_abc").unwrap();
        meta_set(&conn, "robot_id", "robot_def").unwrap();
        assert_eq!(meta_get(&conn, "robot_id").unwrap().unwrap(), "robot_def");
    }
}
