//! The transactional outbox (decisions Q10/Q11): effects are written here
//! before anything leaves the process; `dedupe_key` is UNIQUE, so a double
//! send is structurally impossible, not statistically unlikely.

use crate::PrismError;
use rusqlite::{params, Connection, OptionalExtension};
use trust::ids;

/// Enqueue an effect. Returns (effect_id, newly_enqueued). Re-enqueueing the
/// same (intent, target, payload) returns the existing effect untouched.
pub fn enqueue(
    conn: &Connection,
    intent_id: &str,
    target: &str,
    payload: &str,
) -> Result<(String, bool), PrismError> {
    let dedupe_key =
        ids::sha256_hex(format!("{intent_id}|{target}|{payload}").as_bytes());
    let existing: Option<String> = conn
        .query_row(
            "SELECT effect_id FROM outbox WHERE dedupe_key = ?1",
            params![dedupe_key],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok((id, false));
    }
    let effect_id = ids::new_id("eff");
    let now = ids::ts_ms();
    conn.execute(
        "INSERT OR IGNORE INTO outbox \
         (effect_id, intent_id, dedupe_key, target, payload_ref, state, attempts, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,'pending',0,?6,?6)",
        params![effect_id, intent_id, dedupe_key, target, payload, now],
    )?;
    // if a concurrent insert won the race, return the winner
    let id: String = conn.query_row(
        "SELECT effect_id FROM outbox WHERE dedupe_key = ?1",
        params![dedupe_key],
        |r| r.get(0),
    )?;
    let fresh = id == effect_id;
    Ok((id, fresh))
}

pub fn mark(
    conn: &Connection,
    effect_id: &str,
    state: &str,
    last_error: Option<&str>,
) -> Result<(), PrismError> {
    conn.execute(
        "UPDATE outbox SET state = ?2, last_error = ?3, attempts = attempts + 1, updated_at = ?4 \
         WHERE effect_id = ?1",
        params![effect_id, state, last_error, ids::ts_ms()],
    )?;
    Ok(())
}

pub fn state_of(conn: &Connection, effect_id: &str) -> Result<Option<String>, PrismError> {
    Ok(conn
        .query_row(
            "SELECT state FROM outbox WHERE effect_id = ?1",
            params![effect_id],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn payload_of(conn: &Connection, effect_id: &str) -> Result<Option<String>, PrismError> {
    Ok(conn
        .query_row(
            "SELECT payload_ref FROM outbox WHERE effect_id = ?1",
            params![effect_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Effects of one intent, insertion order: (effect_id, state).
pub fn for_intent(
    conn: &Connection,
    intent_id: &str,
) -> Result<Vec<(String, String)>, PrismError> {
    let mut stmt = conn.prepare(
        "SELECT effect_id, state FROM outbox WHERE intent_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt
        .query_map(params![intent_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_cell_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn double_enqueue_is_structurally_impossible() {
        let conn = cell();
        let (id1, fresh1) = enqueue(&conn, "int_a", "surface:chat", "hello").unwrap();
        let (id2, fresh2) = enqueue(&conn, "int_a", "surface:chat", "hello").unwrap();
        assert!(fresh1);
        assert!(!fresh2);
        assert_eq!(id1, id2);
        // a different payload for the same intent is a different effect
        let (_id3, fresh3) = enqueue(&conn, "int_a", "surface:chat", "different").unwrap();
        assert!(fresh3);
    }

    #[test]
    fn state_transitions_are_recorded() {
        let conn = cell();
        let (id, _) = enqueue(&conn, "int_b", "surface:chat", "hi").unwrap();
        assert_eq!(state_of(&conn, &id).unwrap().unwrap(), "pending");
        mark(&conn, &id, "sent", None).unwrap();
        mark(&conn, &id, "confirmed", None).unwrap();
        assert_eq!(state_of(&conn, &id).unwrap().unwrap(), "confirmed");
    }
}
