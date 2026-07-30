//! The durable journal (arch sec 3): append-only step log, per cell.
//! Journal only at decision and effect boundaries (arch sec 3b). After a
//! crash the Robot resumes from the last completed step, never repeating
//! an effect -- see `replay`.

use crate::PrismError;
use rusqlite::{params, Connection, OptionalExtension};
use trust::ids;

/// Open an intent: seq 0, kind `intent_open`.
pub fn intent_open(conn: &Connection, intent_id: &str, payload_json: &str) -> Result<(), PrismError> {
    write_step(conn, intent_id, 0, "intent_open", payload_json, None)
}

/// A completed decision/effect boundary inside an intent.
pub fn step(
    conn: &Connection,
    intent_id: &str,
    kind: &str,
    payload_json: &str,
    outcome_hash: Option<&str>,
) -> Result<(), PrismError> {
    let seq = next_seq(conn, intent_id)?;
    write_step(conn, intent_id, seq, kind, payload_json, outcome_hash)
}

/// Close an intent with a terminal status.
pub fn intent_close(conn: &Connection, intent_id: &str, status: &str) -> Result<(), PrismError> {
    let payload = serde_json::json!({ "status": status }).to_string();
    step(conn, intent_id, "intent_close", &payload, None)
}

fn next_seq(conn: &Connection, intent_id: &str) -> Result<i64, PrismError> {
    Ok(conn.query_row(
        "SELECT coalesce(max(seq), -1) + 1 FROM journal WHERE intent_id = ?1",
        params![intent_id],
        |r| r.get(0),
    )?)
}

fn write_step(
    conn: &Connection,
    intent_id: &str,
    seq: i64,
    kind: &str,
    payload_json: &str,
    outcome_hash: Option<&str>,
) -> Result<(), PrismError> {
    let now = ids::ts_ms();
    conn.execute(
        "INSERT INTO journal(step_id, intent_id, seq, kind, payload_json, started_at, completed_at, outcome_hash) \
         VALUES (?1,?2,?3,?4,?5,?6,?6,?7)",
        params![ids::new_id("step"), intent_id, seq, kind, payload_json, now, outcome_hash],
    )?;
    Ok(())
}

/// Intents with no `intent_close` row -- the replay work-list.
pub fn open_intents(conn: &Connection) -> Result<Vec<String>, PrismError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT intent_id FROM journal j \
         WHERE NOT EXISTS (SELECT 1 FROM journal c \
                           WHERE c.intent_id = j.intent_id AND c.kind = 'intent_close') \
         ORDER BY (SELECT min(started_at) FROM journal m WHERE m.intent_id = j.intent_id)",
    )?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Latest payload of `kind` within an intent, if journaled.
pub fn payload_of(
    conn: &Connection,
    intent_id: &str,
    kind: &str,
) -> Result<Option<String>, PrismError> {
    Ok(conn
        .query_row(
            "SELECT payload_json FROM journal WHERE intent_id = ?1 AND kind = ?2 \
             ORDER BY seq DESC LIMIT 1",
            params![intent_id, kind],
            |r| r.get(0),
        )
        .optional()?)
}

/// All payloads of `kind` within an intent, seq order.
pub fn payloads_of(
    conn: &Connection,
    intent_id: &str,
    kind: &str,
) -> Result<Vec<String>, PrismError> {
    let mut stmt = conn.prepare(
        "SELECT payload_json FROM journal WHERE intent_id = ?1 AND kind = ?2 ORDER BY seq ASC",
    )?;
    let rows = stmt
        .query_map(params![intent_id, kind], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Step kinds of one intent, in seq order.
pub fn kinds_for_intent(conn: &Connection, intent_id: &str) -> Result<Vec<String>, PrismError> {
    let mut stmt =
        conn.prepare("SELECT kind FROM journal WHERE intent_id = ?1 ORDER BY seq ASC")?;
    let kinds = stmt
        .query_map(params![intent_id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(kinds)
}

/// Step kinds of the most recently opened intent. Test helper.
pub fn kinds_for_latest_intent(conn: &Connection) -> Result<Vec<String>, PrismError> {
    let intent_id: String = conn.query_row(
        "SELECT intent_id FROM journal WHERE kind = 'intent_open' \
         ORDER BY started_at DESC, seq DESC LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    kinds_for_intent(conn, &intent_id)
}

/// Total journal rows in this cell.
pub fn count(conn: &Connection) -> Result<i64, PrismError> {
    Ok(conn.query_row("SELECT count(*) FROM journal", [], |r| r.get(0))?)
}
