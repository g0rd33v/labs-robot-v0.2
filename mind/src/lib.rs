//! mind: the epistemic memory substrate (arch sec 4).
//!
//! M3 ships: facts with source-FK provenance (law #5 as schema), FTS5 +
//! sqlite-vec hybrid recall fused with RRF (Q20), the content-addressed
//! encrypted media vault (sec 4a), and Registry-lite (sec 4b): list with
//! sources, correct (supersession), forget (deletes for real).

pub mod commitments;
pub mod connections;
pub mod facts;
pub mod instructions;
pub mod files;
pub mod merge;
pub mod promotion;
pub mod reminders;
pub mod vault;

use rusqlite::{params, Connection, OptionalExtension};
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

/// Bring an existing table up to the current shape, one column at a time.
///
/// Idempotent and additive only. SQLite can add a column with a default in
/// place, which covers every column this schema has gained; anything
/// needing a rewrite would need a real migration and should not sneak in
/// here.
fn add_missing_columns(conn: &Connection, wanted: &[(&str, &str, &str)]) -> Result<(), MindError> {
    for (table, column, ddl) in wanted {
        let present: bool = conn
            .prepare(&format!("PRAGMA table_info({table})"))?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == column);
        if !present {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {ddl};"))?;
        }
    }
    Ok(())
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
    superseded_by TEXT REFERENCES facts(id),
    -- arch sec 7: every object carries a classification. Defaulted to the
    -- protective end, because the objects nobody classified are always the
    -- majority and a scheme that defaults to permissive protects nothing.
    class         TEXT NOT NULL DEFAULT 'owner_private'
);
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
    content, content='facts', content_rowid='rowid'
);
-- sec 4.3: the semantic index covers EVERYTHING, and conversations are
-- most of everything. Facts answer what-do-we-know; this answers
-- what-was-said -- the store LongMemEval-class questions exercise.
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    content, content='messages', content_rowid='rowid'
);
CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
END;
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
-- A file is a name over vault content: the vault knows bytes, this knows
-- documents. Separate tables so two names cost one copy, and so a file
-- carries its own class (sec 7) and its own provenance (law 5).
CREATE TABLE IF NOT EXISTS files (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    hash          TEXT NOT NULL REFERENCES media(hash),
    size          INTEGER NOT NULL,
    class         TEXT NOT NULL DEFAULT 'owner_private',
    source_msg_id TEXT NOT NULL REFERENCES messages(id),
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);
-- Instructions (sec 4.6 / Registry category 2): the person's standing rules,
-- in their own words. Versioned by supersession like facts -- a revision is
-- a new row pointing back, never an overwrite -- and reversible: retiring
-- stops the robot following a rule without destroying the history of having
-- had it.
CREATE TABLE IF NOT EXISTS instructions (
    id            TEXT PRIMARY KEY,
    body          TEXT NOT NULL,
    source_msg_id TEXT NOT NULL REFERENCES messages(id),
    status        TEXT NOT NULL DEFAULT 'active'
                  CHECK (status IN ('active','superseded','retired')),
    superseded_by TEXT REFERENCES instructions(id),
    class         TEXT NOT NULL DEFAULT 'owner_private',
    created_at    INTEGER NOT NULL
);
-- The commitment ledger (sec 4.5): what was asked, what was promised, and
-- why each closed. The Second Law -- never silently drop a request -- is
-- enforceable only if closing REQUIRES a reason, so that is a constraint,
-- not a convention, exactly as source_msg_id is for facts.
CREATE TABLE IF NOT EXISTS commitments (
    id            TEXT PRIMARY KEY,
    what          TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('reminder','approval','promise')),
    status        TEXT NOT NULL DEFAULT 'open'
                  CHECK (status IN ('open','waiting','done','declined','cancelled','failed')),
    source_msg_id TEXT REFERENCES messages(id),
    intent_id     TEXT,
    due_at        INTEGER,
    created_at    INTEGER NOT NULL,
    closed_at     INTEGER,
    closed_why    TEXT,
    CHECK (closed_at IS NULL OR closed_why IS NOT NULL),
    CHECK ((status IN ('open','waiting')) = (closed_at IS NULL))
);
-- Contradictions (Q21): a contested fact is still tentative or stable, so
-- the contradiction is a RELATIONSHIP and cannot live in a status column.
-- Both facts survive; this records that they disagree.
CREATE TABLE IF NOT EXISTS fact_contests (
    fact_a     TEXT NOT NULL REFERENCES facts(id),
    fact_b     TEXT NOT NULL REFERENCES facts(id),
    noticed_at INTEGER NOT NULL,
    PRIMARY KEY (fact_a, fact_b)
);
CREATE TABLE IF NOT EXISTS tombstones (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    deleted_at INTEGER NOT NULL,
    origin     TEXT NOT NULL
);
-- Connected accounts. The most dangerous rows in the cell: a refresh token
-- is standing, renewable access to someone's mailbox. They live here
-- because the cell is encrypted at rest; they are absent from merge::export
-- because a token on a USB stick is standing access on a USB stick.
CREATE TABLE IF NOT EXISTS connections (
    provider      TEXT PRIMARY KEY,
    account       TEXT NOT NULL,
    scopes        TEXT NOT NULL,
    access_token  TEXT NOT NULL,
    refresh_token TEXT,
    expires_at    INTEGER NOT NULL,
    connected_at  INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS cell_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
",
    )?;

    // Older cells wrote messages before messages_fts existed. An external-
    // content FTS table reads rows from its content table, so no count can
    // reveal an unbuilt index -- a one-time marker in cell_meta says
    // whether this cell has ever rebuilt. Without this, every pre-existing
    // conversation is invisible to recall on exactly the instances with
    // the most to recall.
    let built: bool = conn
        .query_row(
            "SELECT 1 FROM cell_meta WHERE key = 'messages_fts_built'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(MindError::Sql)?
        .is_some();
    if !built {
        conn.execute_batch("INSERT INTO messages_fts(messages_fts) VALUES('rebuild');")?;
        conn.execute(
            "INSERT INTO cell_meta(key, value) VALUES ('messages_fts_built', '1')",
            [],
        )?;
    }

    // `CREATE TABLE IF NOT EXISTS` creates tables; it never adds a column
    // to one that already exists. So every column added after a cell was
    // first written is invisible to that cell, and the failure is not a
    // startup error -- it is a query, months later, against the one
    // instance that happens to be older. That is exactly how `class`
    // silently broke sync with a USB stick written before it existed.
    add_missing_columns(
        conn,
        &[
            ("facts", "class", "TEXT NOT NULL DEFAULT 'owner_private'"),
            // sec 4's mutation protocol ends at "owner-confirmed fact"; the
            // Registry's confirm button needs somewhere to put that.
            ("facts", "confirmed_at", "INTEGER"),
        ],
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

#[cfg(test)]
mod schema_tests {
    use super::*;

    /// What was said is findable later, and hostile input is terms, not
    /// FTS syntax.
    #[test]
    fn conversation_is_searchable_and_a_query_is_never_syntax() {
        let conn = Connection::open_in_memory().unwrap();
        init_cell_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO messages(id, ts, direction, surface, content) VALUES
             ('m1', 100, 'in', 'web', 'my dentist appointment is on thursday'),
             ('m2', 200, 'out', 'web', 'noted -- thursday it is'),
             ('m3', 300, 'in', 'web', 'the wifi password at the office changed');",
        )
        .unwrap();

        let hits = recall_messages(&conn, "when is my dentist visit", 5).unwrap();
        assert!(
            hits.iter().any(|(_, _, c)| c.contains("dentist")),
            "{hits:?}"
        );
        // operators and quotes arrive as words, never as query syntax
        for hostile in ["dentist\" OR \"*", "NEAR(a b)", "col: *", "\"\"\""] {
            let r = recall_messages(&conn, hostile, 5);
            assert!(r.is_ok(), "hostile query errored: {hostile:?}");
        }
        assert!(recall_messages(&conn, "   ", 5).unwrap().is_empty());
    }

    /// Older cells wrote messages before the index existed; opening them
    /// backfills, so history is searchable exactly where there is most of it.
    #[test]
    fn a_pre_index_cell_backfills_its_conversation_index() {
        let conn = Connection::open_in_memory().unwrap();
        // a cell born before messages_fts: table without the index
        conn.execute_batch(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, ts INTEGER NOT NULL,
                 direction TEXT NOT NULL, surface TEXT NOT NULL, lang TEXT,
                 content TEXT NOT NULL, media_ref TEXT);
             INSERT INTO messages(id, ts, direction, surface, content)
                 VALUES ('m1', 1, 'in', 'web', 'the old conversation about the boiler');",
        )
        .unwrap();
        init_cell_schema(&conn).unwrap();
        let hits = recall_messages(&conn, "boiler", 5).unwrap();
        assert_eq!(hits.len(), 1, "pre-existing words must be indexed");
    }

    /// The failure this fixes, reproduced: a cell written before `class`
    /// existed. `CREATE TABLE IF NOT EXISTS` leaves it alone, so every
    /// query naming the column fails -- and it fails on the OTHER instance,
    /// months later, as "no such column: class" in the middle of a sync.
    #[test]
    fn an_older_cell_gains_columns_it_was_written_without() {
        let conn = Connection::open_in_memory().unwrap();
        // the shape of `facts` before sec 7's classification landed
        conn.execute_batch(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, ts INTEGER NOT NULL,
                 direction TEXT NOT NULL, surface TEXT NOT NULL, lang TEXT,
                 content TEXT NOT NULL, media_ref TEXT);
             CREATE TABLE facts (
                 id TEXT PRIMARY KEY, entity TEXT, content TEXT NOT NULL,
                 source_msg_id TEXT NOT NULL REFERENCES messages(id),
                 intent_id TEXT UNIQUE, status TEXT NOT NULL DEFAULT 'stable',
                 confidence REAL NOT NULL DEFAULT 1.0,
                 created_at INTEGER NOT NULL, superseded_by TEXT);
             INSERT INTO messages(id, ts, direction, surface, content)
                 VALUES ('m1', 1, 'in', 'chat', 'older words');
             INSERT INTO facts(id, content, source_msg_id, created_at)
                 VALUES ('f1', 'an older fact', 'm1', 1);",
        )
        .unwrap();

        assert!(
            conn.query_row("SELECT class FROM facts", [], |r| r.get::<_, String>(0))
                .is_err(),
            "precondition: the old cell has no class column"
        );

        init_cell_schema(&conn).unwrap();

        // the column is there, and the rows that predate it land on the
        // protective default rather than on nothing
        let class: String = conn
            .query_row("SELECT class FROM facts WHERE id = 'f1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(class, "owner_private");

        // and the export that broke sync now runs
        let d = crate::merge::export(&conn, 0).unwrap();
        assert_eq!(d.facts.len(), 1);

        // idempotent: running it again is a no-op, not a duplicate column
        init_cell_schema(&conn).unwrap();
        assert_eq!(crate::merge::export(&conn, 0).unwrap().facts.len(), 1);
    }
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

/// What was said, found again (§4.3).
///
/// FTS over the whole conversation history, newest-first among equals.
/// Distinct from `facts::recall`: facts are curated knowledge with
/// provenance; this is verbatim conversation, and it is what questions
/// like "when did I mention the dentist" actually need — the fact may
/// never have been extracted, but the words were said.
pub fn recall_messages(
    conn: &Connection,
    query: &str,
    k: usize,
) -> Result<Vec<(i64, String, String)>, MindError> {
    // FTS5 query syntax is an injection surface of its own (quotes,
    // operators); every term is quoted so the person's words are terms,
    // never syntax. OR-joined: recall is retrieval, not boolean algebra.
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .take(12)
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect();
    if terms.is_empty() {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare(
        "SELECT m.ts, m.direction, m.content \
         FROM messages_fts f JOIN messages m ON m.rowid = f.rowid \
         WHERE messages_fts MATCH ?1 \
         ORDER BY bm25(messages_fts), m.ts DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![terms.join(" OR "), k as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Number of stored messages. Used by tests and gate demos.
pub fn message_count(conn: &Connection) -> Result<i64, MindError> {
    Ok(conn.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?)
}

/// The last `limit` messages in chronological order: (direction, content).
/// Feeds the model's conversation context.
pub fn recent_messages(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<(String, String)>, MindError> {
    let mut stmt = conn.prepare(
        "SELECT direction, content FROM (\
             SELECT direction, content, ts, rowid FROM messages \
             ORDER BY ts DESC, rowid DESC LIMIT ?1\
         ) ORDER BY ts ASC, rowid ASC",
    )?;
    let rows = stmt
        .query_map(params![limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Messages after a timestamp, chronological: (ts, direction, content).
/// Feeds the chat history/poll endpoint.
pub fn messages_after(
    conn: &Connection,
    after_ts: i64,
    limit: usize,
) -> Result<Vec<(i64, String, String)>, MindError> {
    let mut stmt = conn.prepare(
        "SELECT ts, direction, content FROM messages WHERE ts > ?1 \
         ORDER BY ts ASC, rowid ASC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![after_ts, limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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
