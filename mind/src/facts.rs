//! Facts with provenance (arch sec 4, law #5): every fact carries a source
//! pointer -- a FOREIGN KEY to the message it was learned from. "No
//! knowledge without a source" is a constraint, not a guideline.
//!
//! Retrieval is Q20's hybrid: RRF (k=60) over the FTS door (top-20), the
//! vector door (top-20, cosine cutoff 0.20), the graph door (same-entity,
//! top-10), and recency (top-10).

use crate::MindError;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    /// arch sec 7 data class; decides whether this may enter an external call.
    pub class: String,
    pub id: String,
    pub entity: Option<String>,
    pub content: String,
    pub source_msg_id: String,
    pub intent_id: Option<String>,
    pub status: String,
    pub confidence: f64,
    pub created_at: i64,
    pub superseded_by: Option<String>,
}

const COLS: &str =
    "id, entity, content, source_msg_id, intent_id, status, confidence, created_at, superseded_by, class";

fn row_to_fact(r: &rusqlite::Row<'_>) -> Result<Fact, rusqlite::Error> {
    Ok(Fact {
        id: r.get(0)?,
        entity: r.get(1)?,
        content: r.get(2)?,
        source_msg_id: r.get(3)?,
        intent_id: r.get(4)?,
        status: r.get(5)?,
        confidence: r.get(6)?,
        created_at: r.get(7)?,
        superseded_by: r.get(8)?,
        class: r.get(9).unwrap_or_else(|_| "owner_private".into()),
    })
}

/// Store an owner-stated fact. Idempotent per intent (UNIQUE(intent_id)):
/// crash replay cannot double-remember. The FK to `messages` enforces
/// provenance at the schema level.
pub fn remember(
    conn: &Connection,
    content: &str,
    source_msg_id: &str,
    intent_id: &str,
    embedding: Option<&[f32]>,
) -> Result<Fact, MindError> {
    // Enters TENTATIVE, not stable (Q21). A fact said once in passing is
    // not a fact confirmed -- landing everything at `stable` made "you told
    // me X" a claim about firmness the store could not support. The ladder
    // below decides where it actually belongs.
    conn.execute(
        "INSERT OR IGNORE INTO facts(id, content, source_msg_id, intent_id, status, confidence, created_at) \
         VALUES (?1, ?2, ?3, ?4, 'tentative', 0.5, ?5)",
        params![
            trust::ids::new_id("fact"),
            content,
            source_msg_id,
            intent_id,
            trust::ids::ts_ms()
        ],
    )?;
    let fact: Fact = conn.query_row(
        &format!("SELECT {COLS} FROM facts WHERE intent_id = ?1"),
        params![intent_id],
        row_to_fact,
    )?;
    if let Some(emb) = embedding {
        upsert_embedding(conn, &fact.id, emb)?;
    }
    // `memory.remember` is reached when the person SAYS something about
    // themselves, so this is an explicit owner statement -- Q21's
    // tentative→contextual trigger. Extraction from documents will call
    // this with `false` when it exists.
    crate::promotion::place(conn, &fact.id, content, true)?;
    // re-read: the row moved
    let fact: Fact = conn.query_row(
        &format!("SELECT {COLS} FROM facts WHERE id = ?1"),
        params![fact.id],
        row_to_fact,
    )?;
    Ok(fact)
}

/// Attach/replace the vector for a fact (no-op if the vec table is absent).
pub fn upsert_embedding(conn: &Connection, fact_id: &str, emb: &[f32]) -> Result<(), MindError> {
    if !vec_available(conn) {
        return Ok(());
    }
    let rowid: i64 = conn.query_row(
        "SELECT rowid FROM facts WHERE id = ?1",
        params![fact_id],
        |r| r.get(0),
    )?;
    conn.execute("DELETE FROM facts_vec WHERE rowid = ?1", params![rowid])?;
    conn.execute(
        "INSERT INTO facts_vec(rowid, embedding) VALUES (?1, ?2)",
        params![rowid, f32s_to_bytes(emb)],
    )?;
    Ok(())
}

pub fn vec_available(conn: &Connection) -> bool {
    matches!(
        conn.query_row(
            "SELECT value FROM cell_meta WHERE key = 'vec_ready'",
            [],
            |r| r.get::<_, String>(0),
        ),
        Ok(v) if v == "1"
    )
}

fn f32s_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

// ------------------------------------------------------------------ recall

/// Q20 hybrid recall. `query_embedding` is optional (vector door skipped
/// without it). An empty query returns recent facts.
pub fn recall(
    conn: &Connection,
    query: &str,
    query_embedding: Option<&[f32]>,
    limit: usize,
) -> Result<Vec<Fact>, MindError> {
    let mut doors: Vec<Vec<i64>> = Vec::new();
    if !query.trim().is_empty() {
        doors.push(fts_door(conn, query)?);
        if let Some(emb) = query_embedding {
            doors.push(vec_door(conn, emb)?);
        }
        let seeds: Vec<i64> = doors.iter().flatten().take(5).copied().collect();
        doors.push(graph_door(conn, &seeds)?);
    }
    doors.push(recency_door(conn)?);

    let fused = rrf(&doors, 60.0);
    let mut facts = Vec::new();
    for rowid in fused.into_iter().take(limit) {
        let fact: Option<Fact> = conn
            .query_row(
                &format!("SELECT {COLS} FROM facts WHERE rowid = ?1 AND status != 'superseded'"),
                params![rowid],
                row_to_fact,
            )
            .optional()?;
        if let Some(f) = fact {
            facts.push(f);
        }
    }
    Ok(facts)
}

/// Reciprocal-rank fusion: score = sum over doors of 1/(k + rank).
fn rrf(doors: &[Vec<i64>], k: f64) -> Vec<i64> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for door in doors {
        for (rank, rowid) in door.iter().enumerate() {
            *scores.entry(*rowid).or_insert(0.0) += 1.0 / (k + rank as f64 + 1.0);
        }
    }
    let mut ranked: Vec<(i64, f64)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.into_iter().map(|(rowid, _)| rowid).collect()
}

fn fts_door(conn: &Connection, query: &str) -> Result<Vec<i64>, MindError> {
    // quote each term so user text can never be FTS syntax
    let match_expr = query
        .split_whitespace()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    if match_expr.is_empty() {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare(
        "SELECT f.rowid FROM facts_fts JOIN facts f ON f.rowid = facts_fts.rowid \
         WHERE facts_fts MATCH ?1 AND f.status != 'superseded' \
         ORDER BY facts_fts.rank LIMIT 20",
    )?;
    let rows = stmt
        .query_map(params![match_expr], |r| r.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn vec_door(conn: &Connection, emb: &[f32]) -> Result<Vec<i64>, MindError> {
    if !vec_available(conn) {
        return Ok(vec![]);
    }
    // cosine distance <= 0.8 == similarity >= 0.20 (the Akita cutoff, Q20)
    let mut stmt = conn.prepare(
        "SELECT v.rowid FROM facts_vec v JOIN facts f ON f.rowid = v.rowid \
         WHERE v.embedding MATCH ?1 AND k = 20 AND v.distance <= 0.8 \
           AND f.status != 'superseded' \
         ORDER BY v.distance",
    )?;
    let rows = stmt
        .query_map(params![f32s_to_bytes(emb)], |r| r.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 1-hop over the entity graph: facts sharing an entity with the seeds.
/// (Entities are model-extracted from M4 on; until then this door is mostly
/// quiet -- but the path is Q20-complete.)
fn graph_door(conn: &Connection, seed_rowids: &[i64]) -> Result<Vec<i64>, MindError> {
    let mut out = Vec::new();
    for rowid in seed_rowids {
        let entity: Option<Option<String>> = conn
            .query_row(
                "SELECT entity FROM facts WHERE rowid = ?1",
                params![rowid],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(Some(entity)) = entity {
            let mut stmt = conn.prepare(
                "SELECT rowid FROM facts WHERE entity = ?1 AND rowid != ?2 \
                 AND status != 'superseded' LIMIT 10",
            )?;
            let rows = stmt
                .query_map(params![entity, rowid], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            out.extend(rows);
            if out.len() >= 10 {
                out.truncate(10);
                break;
            }
        }
    }
    Ok(out)
}

fn recency_door(conn: &Connection) -> Result<Vec<i64>, MindError> {
    let mut stmt = conn.prepare(
        "SELECT rowid FROM facts WHERE status != 'superseded' \
         ORDER BY created_at DESC, rowid DESC LIMIT 10",
    )?;
    let rows = stmt
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------- registry

/// Registry-lite (arch sec 4b): every active fact with its source message.
/// Ordered newest first; the 1-based position is the address for
/// forget/correct commands.
pub fn registry_list(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<(Fact, String, i64)>, MindError> {
    let mut stmt = conn.prepare(
        "SELECT f.id, f.entity, f.content, f.source_msg_id, f.intent_id, f.status, \
                f.confidence, f.created_at, f.superseded_by, f.class, m.content, m.ts \
         FROM facts f JOIN messages m ON m.id = f.source_msg_id \
         WHERE f.status != 'superseded' \
         ORDER BY f.created_at DESC, f.rowid DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit as i64], |r| {
            Ok((row_to_fact(r)?, r.get::<_, String>(10)?, r.get::<_, i64>(11)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Idempotency marker for destructive registry ops: index addressing is not
/// stable across re-execution, so the first execution records what it did
/// under its intent; crash replay returns the recorded result instead of
/// resolving the index again (and possibly hitting a different fact).
///
/// Public because the same hazard applies to any capability whose target is
/// selected at execution time rather than named in the plan (e.g.
/// `reminder.cancel_last`, which cancels "the latest" -- a different row
/// after a crash).
pub fn op_marker(conn: &Connection, intent_id: &str) -> Result<Option<serde_json::Value>, MindError> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM cell_meta WHERE key = ?1",
            params![format!("op:{intent_id}")],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.and_then(|s| serde_json::from_str(&s).ok()))
}

pub fn set_op_marker(
    conn: &Connection,
    intent_id: &str,
    value: &serde_json::Value,
) -> Result<(), MindError> {
    conn.execute(
        "INSERT INTO cell_meta(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![format!("op:{intent_id}"), value.to_string()],
    )?;
    Ok(())
}

/// Owner's erase right: the row is deleted for real (FTS + vector cleaned).
/// Addressed by registry position (1-based, newest first). Idempotent per
/// intent: re-execution after a crash returns the recorded result and never
/// deletes a second fact.
pub fn forget_by_index(
    conn: &Connection,
    index: usize,
    intent_id: &str,
    origin: &str,
) -> Result<Option<String>, MindError> {
    if let Some(marker) = op_marker(conn, intent_id)? {
        return Ok(marker["content"].as_str().map(String::from));
    }
    let listed = registry_list(conn, 100)?;
    let Some((fact, _, _)) = listed.into_iter().nth(index.saturating_sub(1)) else {
        set_op_marker(conn, intent_id, &serde_json::json!({ "op": "forget" }))?;
        return Ok(None);
    };
    let rowid: i64 = conn.query_row(
        "SELECT rowid FROM facts WHERE id = ?1",
        params![fact.id],
        |r| r.get(0),
    )?;
    let tx = conn.unchecked_transaction()?;
    // Erasing a fact erases its whole supersession CHAIN. The predecessors
    // are earlier versions of the same knowledge -- "my password is hunter2"
    // corrected to "...is xyz" -- and they are unreachable from the registry
    // (every read path filters `status != 'superseded'`). Detaching them
    // would leave the original text on disk, unaddressable and undeletable,
    // while the reply claims "deleted, not hidden". The erase right means
    // the content goes, so we walk the chain backwards and delete all of it.
    let mut doomed = vec![(fact.id.clone(), rowid)];
    let mut frontier = vec![fact.id.clone()];
    while let Some(current) = frontier.pop() {
        let mut stmt =
            tx.prepare("SELECT id, rowid FROM facts WHERE superseded_by = ?1")?;
        let parents = stmt
            .query_map(params![current], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (id, rid) in parents {
            frontier.push(id.clone());
            doomed.push((id, rid));
        }
    }
    let vec_on = vec_available(&tx);
    for (id, rid) in &doomed {
        if vec_on {
            tx.execute("DELETE FROM facts_vec WHERE rowid = ?1", params![rid])?;
        }
        // break inbound links before the row goes, so the FK never trips
        tx.execute(
            "UPDATE facts SET superseded_by = NULL WHERE superseded_by = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM facts WHERE id = ?1", params![id])?;
        // A deletion the other instance must learn about, or sync would
        // quietly resurrect it. The marker carries the id and the moment --
        // never the content -- and is collected once every peer has applied
        // it, so "deleted for real" survives replication.
        tx.execute(
            "INSERT OR REPLACE INTO tombstones(id, kind, deleted_at, origin) \
             VALUES (?1, 'fact', ?2, ?3)",
            params![id, trust::ids::ts_ms(), origin],
        )?;
    }
    tx.execute(
        "INSERT INTO cell_meta(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![
            format!("op:{intent_id}"),
            serde_json::json!({ "op": "forget", "id": fact.id, "content": fact.content })
                .to_string()
        ],
    )?;
    tx.commit()?;
    Ok(Some(fact.content))
}

/// Correction is supersession, never overwrite (arch sec 4): a new fact is
/// stored with the same provenance chain; the old one is marked superseded
/// and drops out of recall, but its history remains inspectable. Idempotent
/// per intent via the op marker.
pub fn correct_by_index(
    conn: &Connection,
    index: usize,
    new_content: &str,
    source_msg_id: &str,
    intent_id: &str,
    embedding: Option<&[f32]>,
) -> Result<Option<(String, Fact)>, MindError> {
    if let Some(marker) = op_marker(conn, intent_id)? {
        let old_content = marker["old_content"].as_str().map(String::from);
        let new: Option<Fact> = conn
            .query_row(
                &format!("SELECT {COLS} FROM facts WHERE intent_id = ?1"),
                params![intent_id],
                row_to_fact,
            )
            .optional()?;
        return Ok(old_content.zip(new));
    }
    let listed = registry_list(conn, 100)?;
    let Some((old, _, _)) = listed.into_iter().nth(index.saturating_sub(1)) else {
        set_op_marker(conn, intent_id, &serde_json::json!({ "op": "correct" }))?;
        return Ok(None);
    };
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO facts(id, content, source_msg_id, intent_id, status, confidence, created_at) \
         VALUES (?1, ?2, ?3, ?4, 'stable', 1.0, ?5)",
        params![
            trust::ids::new_id("fact"),
            new_content,
            source_msg_id,
            intent_id,
            trust::ids::ts_ms()
        ],
    )?;
    let new: Fact = tx.query_row(
        &format!("SELECT {COLS} FROM facts WHERE intent_id = ?1"),
        params![intent_id],
        row_to_fact,
    )?;
    tx.execute(
        "UPDATE facts SET status = 'superseded', superseded_by = ?2 WHERE id = ?1",
        params![old.id, new.id],
    )?;
    tx.execute(
        "INSERT INTO cell_meta(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![
            format!("op:{intent_id}"),
            serde_json::json!({ "op": "correct", "old_id": old.id, "old_content": old.content })
                .to_string()
        ],
    )?;
    tx.commit()?;
    if let Some(emb) = embedding {
        upsert_embedding(conn, &new.id, emb)?;
    }
    Ok(Some((old.content, new)))
}

pub fn count_active(conn: &Connection) -> Result<i64, MindError> {
    Ok(conn.query_row(
        "SELECT count(*) FROM facts WHERE status != 'superseded'",
        [],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::init_cell_schema(&conn).unwrap();
        conn
    }

    fn msg(conn: &Connection, text: &str) -> String {
        crate::record_message(conn, "in", "chat", text).unwrap()
    }

    #[test]
    fn provenance_is_a_constraint_not_a_convention() {
        let conn = cell();
        // a fact with a bogus source pointer must be REJECTED by the schema
        let err = conn.execute(
            "INSERT INTO facts(id, content, source_msg_id, status, confidence, created_at) \
             VALUES ('fact_x', 'orphan', 'msg_does_not_exist', 'stable', 1.0, 0)",
            [],
        );
        assert!(err.is_err(), "law #5: no fact without a source");
    }

    #[test]
    fn remember_is_idempotent_and_recall_finds_it() {
        let conn = cell();
        let m = msg(&conn, "remember that i drink green tea");
        let a = remember(&conn, "i drink green tea", &m, "int_1", None).unwrap();
        let b = remember(&conn, "i drink green tea", &m, "int_1", None).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(count_active(&conn).unwrap(), 1);

        let found = recall(&conn, "tea", None, 5).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].content, "i drink green tea");
        // empty query returns recent facts
        assert_eq!(recall(&conn, "", None, 5).unwrap().len(), 1);
        // unrelated queries still surface recency-door results (RRF), but
        // matching ones rank the match first
        let m2 = msg(&conn, "remember that mark's birthday is in june");
        remember(&conn, "mark's birthday is in june", &m2, "int_2", None).unwrap();
        let found = recall(&conn, "birthday", None, 5).unwrap();
        assert_eq!(found[0].content, "mark's birthday is in june");
    }

    #[test]
    fn forget_works_for_real_and_replay_is_safe() {
        let conn = cell();
        let m = msg(&conn, "remember secret plans");
        remember(&conn, "secret plans", &m, "int_1", None).unwrap();
        let m2 = msg(&conn, "remember second fact");
        remember(&conn, "second fact", &m2, "int_2", None).unwrap();

        let gone = forget_by_index(&conn, 2, "int_f", "inst_test").unwrap().unwrap();
        assert_eq!(gone, "secret plans"); // position 2 = older fact
        assert_eq!(count_active(&conn).unwrap(), 1);
        // gone from the fts door too
        assert!(recall(&conn, "secret", None, 5)
            .unwrap()
            .iter()
            .all(|f| f.content != "secret plans"));
        // crash replay: same intent re-executes -> recorded result, and the
        // OTHER fact is untouched (index re-resolution would have hit it)
        let again = forget_by_index(&conn, 2, "int_f", "inst_test").unwrap().unwrap();
        assert_eq!(again, "secret plans");
        assert_eq!(count_active(&conn).unwrap(), 1);
        // forgetting nothing is honest (fresh intent, empty position)
        assert!(forget_by_index(&conn, 9, "int_g", "inst_test").unwrap().is_none());
    }

    #[test]
    fn correct_supersedes_never_overwrites() {
        let conn = cell();
        let m = msg(&conn, "remember i live in moscow");
        remember(&conn, "i live in moscow", &m, "int_1", None).unwrap();
        let m2 = msg(&conn, "correct fact 1: i live in lisbon");
        let (old_content, new) =
            correct_by_index(&conn, 1, "i live in lisbon", &m2, "int_2", None)
                .unwrap()
                .unwrap();
        assert_eq!(old_content, "i live in moscow");
        assert_eq!(new.content, "i live in lisbon");
        let old_id: String = conn
            .query_row(
                "SELECT id FROM facts WHERE content = 'i live in moscow'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // replay-safe: the same intent returns the recorded result
        let (rc, rn) = correct_by_index(&conn, 1, "i live in lisbon", &m2, "int_2", None)
            .unwrap()
            .unwrap();
        assert_eq!(rc, "i live in moscow");
        assert_eq!(rn.id, new.id);
        assert_eq!(count_active(&conn).unwrap(), 1);
        // recall prefers the correction; the old fact is out of every door
        let found = recall(&conn, "live", None, 5).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].content, "i live in lisbon");
        // but the history is preserved: superseded, linked, inspectable
        let (status, by): (String, Option<String>) = conn
            .query_row(
                "SELECT status, superseded_by FROM facts WHERE id = ?1",
                params![old_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "superseded");
        assert_eq!(by.unwrap(), new.id);
    }

    /// Regression: forgetting a CORRECTED fact used to detach the original
    /// and leave it on disk forever -- unreachable from the registry (it is
    /// `superseded`), so undeletable through any command, while the reply
    /// claimed "the row is deleted, not hidden". The erase right has to
    /// reach the whole chain.
    #[test]
    fn forget_erases_the_whole_supersession_chain() {
        let conn = cell();
        let m1 = msg(&conn, "remember that my password is hunter2");
        remember(&conn, "my password is hunter2", &m1, "int_1", None).unwrap();
        let m2 = msg(&conn, "correct fact 1: my password is xyz");
        correct_by_index(&conn, 1, "my password is xyz", &m2, "int_2", None)
            .unwrap()
            .unwrap();
        // a second correction, so the chain is three deep
        let m3 = msg(&conn, "correct fact 1: my password is abc");
        correct_by_index(&conn, 1, "my password is abc", &m3, "int_3", None)
            .unwrap()
            .unwrap();
        let total: i64 = conn
            .query_row("SELECT count(*) FROM facts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 3, "two superseded + one live");

        forget_by_index(&conn, 1, "int_forget", "inst_test").unwrap().unwrap();

        // nothing survives -- not the live row, not the superseded originals
        let remaining: i64 = conn
            .query_row("SELECT count(*) FROM facts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0, "superseded originals must not be stranded");
        let leaked: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts WHERE content LIKE '%hunter2%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leaked, 0, "the original secret is still on disk");
        assert_eq!(count_active(&conn).unwrap(), 0);
    }

    #[test]
    fn registry_lists_facts_with_sources() {
        let conn = cell();
        let m = msg(&conn, "remember that the demo is on friday");
        remember(&conn, "the demo is on friday", &m, "int_1", None).unwrap();
        let listed = registry_list(&conn, 10).unwrap();
        assert_eq!(listed.len(), 1);
        let (fact, source, _ts) = &listed[0];
        assert_eq!(fact.content, "the demo is on friday");
        assert!(source.contains("demo is on friday")); // the actual words
    }
}

/// Set an object's data class (arch §7).
///
/// One-based index into the registry, like every other owner operation on
/// facts -- the person is looking at a numbered list, not at row ids.
/// Owner-confirmed (sec 4's mutation protocol, final rung): the person
/// looked at the fact and said "yes, that is true". Recorded as a moment
/// rather than a flag, so the Registry can say WHEN it was confirmed --
/// a confirmation ages like any other assertion.
pub fn confirm_by_index(conn: &Connection, index: usize) -> Result<Option<String>, MindError> {
    let listed = registry_list(conn, 100)?;
    let Some((fact, _, _)) = listed.into_iter().nth(index.saturating_sub(1)) else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE facts SET confirmed_at = ?2, confidence = 1.0, status = 'stable' \
         WHERE id = ?1",
        params![fact.id, trust::ids::ts_ms()],
    )?;
    Ok(Some(fact.content))
}

pub fn classify_by_index(
    conn: &Connection,
    index: usize,
    class: &str,
) -> Result<Option<String>, MindError> {
    let listed = registry_list(conn, 100)?;
    let Some((fact, _, _)) = listed.into_iter().nth(index.saturating_sub(1)) else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE facts SET class = ?2 WHERE id = ?1",
        params![fact.id, class],
    )?;
    Ok(Some(fact.content))
}

#[cfg(test)]
mod class_tests {
    use super::*;

    /// Item 3's gate, at the layer that owns the data: a restricted fact is
    /// still recallable locally -- the robot has not forgotten it -- but it
    /// carries the class that keeps it out of an external call.
    #[test]
    fn a_classified_fact_keeps_its_class_through_recall_and_registry() {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_cell_schema(&conn).unwrap();
        let m = crate::record_message(&conn, "in", "chat", "my passport number is X").unwrap();
        remember(&conn, "my passport number is X", &m, "int_1", None).unwrap();

        // defaults to the protective end
        let listed = registry_list(&conn, 10).unwrap();
        assert_eq!(listed[0].0.class, "owner_private");

        assert!(classify_by_index(&conn, 1, "restricted")
            .unwrap()
            .is_some());

        // the registry shows it
        let listed = registry_list(&conn, 10).unwrap();
        assert_eq!(listed[0].0.class, "restricted");
        assert!(listed[0].1.contains("passport"), "the source still reads back");

        // and recall still FINDS it -- the robot has not forgotten anything;
        // the class governs where it may go, not whether it is known
        let found = recall(&conn, "passport", None, 5).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].class, "restricted");
    }

    #[test]
    fn classifying_a_fact_that_is_not_there_says_so() {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_cell_schema(&conn).unwrap();
        assert!(classify_by_index(&conn, 9, "restricted").unwrap().is_none());
    }
}
