//! The durable journal (arch sec 3): append-only step log, per cell.
//! Journal only at decision and effect boundaries (arch sec 3b).

use crate::PrismError;
use rusqlite::{params, Connection};
use trust::ids;

/// Open an intent: seq 0, kind `intent_open`.
pub fn intent_open(conn: &Connection, intent_id: &str, payload_json: &str) -> Result<(), PrismError> {
    write_step(conn, intent_id, 0, "intent_open", payload_json, None)
}

/// A completed step inside an intent. M1 steps complete immediately.
pub fn step(
    conn: &Connection,
    intent_id: &str,
    seq: i64,
    kind: &str,
    payload_json: &str,
    outcome_hash: Option<&str>,
) -> Result<(), PrismError> {
    write_step(conn, intent_id, seq, kind, payload_json, outcome_hash)
}

/// Close an intent with a terminal status; seq is max+1 for the intent.
pub fn intent_close(conn: &Connection, intent_id: &str, status: &str) -> Result<(), PrismError> {
    let next: i64 = conn.query_row(
        "SELECT coalesce(max(seq), 0) + 1 FROM journal WHERE intent_id = ?1",
        params![intent_id],
        |r| r.get(0),
    )?;
    let payload = format!("{{\"status\":\"{status}\"}}");
    write_step(conn, intent_id, next, "intent_close", &payload, None)
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

/// Step kinds of the most recently opened intent, in seq order. Test helper.
pub fn kinds_for_latest_intent(conn: &Connection) -> Result<Vec<String>, PrismError> {
    let intent_id: String = conn.query_row(
        "SELECT intent_id FROM journal ORDER BY started_at DESC, seq DESC LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    let mut stmt =
        conn.prepare("SELECT kind FROM journal WHERE intent_id = ?1 ORDER BY seq ASC")?;
    let kinds = stmt
        .query_map(params![intent_id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(kinds)
}

/// Total journal rows in this cell. Test helper.
pub fn count(conn: &Connection) -> Result<i64, PrismError> {
    Ok(conn.query_row("SELECT count(*) FROM journal", [], |r| r.get(0))?)
}
