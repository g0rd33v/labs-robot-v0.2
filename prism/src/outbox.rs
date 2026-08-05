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

/// The retry ladder (spec §5.2): `failed(n<3 retries: 1s/5s/25s)`.
///
/// Exponential by fives, three attempts, then the effect stays `failed`
/// and stops costing anything. The delays are deliberately short: these
/// are deliveries to a surface the person is looking at, not background
/// reconciliation — a reminder that arrives 31 seconds late is late, and
/// one that arrives an hour late is a different message.
pub const RETRY_DELAYS_MS: [i64; 3] = [1_000, 5_000, 25_000];

/// When this effect may next be tried, or `None` if its attempts are spent.
///
/// `attempts` counts deliveries MADE, and `mark` increments it -- so a row
/// that just failed its original send carries `attempts == 1`, and its
/// first retry is one second later. The ladder is therefore indexed at
/// `attempts - 1`; getting that off by one is the difference between
/// "retry in 1s" and "retry in 5s", which nothing would ever report.
///
/// A function rather than a column, because the schedule belongs to the
/// ladder and not to the row: changing the ladder must not require
/// rewriting every pending effect's stored deadline.
pub fn next_attempt_at(attempts: i64, updated_at: i64) -> Option<i64> {
    let rung = usize::try_from(attempts.max(1) - 1).unwrap_or(usize::MAX);
    RETRY_DELAYS_MS.get(rung).map(|d| updated_at + d)
}

/// Effects owed another attempt right now: `failed`, with retries left and
/// their backoff elapsed. Oldest first, so a burst of failures drains in
/// the order it was created rather than the order SQLite happens to scan.
pub fn due_for_retry(
    conn: &Connection,
    now: i64,
) -> Result<Vec<(String, String, String)>, PrismError> {
    let mut stmt = conn.prepare(
        "SELECT effect_id, target, payload_ref, attempts, updated_at \
         FROM outbox WHERE state = 'failed' ORDER BY created_at ASC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, _, _, attempts, updated_at)| {
            next_attempt_at(*attempts, *updated_at).is_some_and(|at| at <= now)
        })
        .map(|(id, target, payload, _, _)| (id, target, payload))
        .collect())
}

/// Has this effect climbed the whole ladder?
///
/// Deliberately NOT a new state value. The spec's state set is
/// pending/sent/confirmed/failed, and a `CHECK` constraint cannot be
/// altered in place -- adding 'abandoned' would be invisible to every cell
/// written before today and would fail their CHECK on write, which is the
/// `class` column's lesson in a new costume. "Given up" is therefore
/// `failed` with the ladder spent, which is a fact about the row rather
/// than a fifth name for it.
pub fn is_abandoned(attempts: i64) -> bool {
    // one original send + the whole ladder
    attempts > RETRY_DELAYS_MS.len() as i64
}

/// Effects that will never be delivered: failed, ladder spent. What the
/// owner should be told about rather than left to discover.
pub fn abandoned(conn: &Connection) -> Result<Vec<(String, String, String)>, PrismError> {
    let mut stmt = conn.prepare(
        "SELECT effect_id, target, coalesce(last_error, '') FROM outbox \
         WHERE state = 'failed' AND attempts > ?1 ORDER BY updated_at DESC",
    )?;
    let rows = stmt
        .query_map(params![RETRY_DELAYS_MS.len() as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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

    /// The spec's ladder, exactly: three retries at 1s/5s/25s, then spent.
    #[test]
    fn the_retry_ladder_is_one_five_twentyfive_then_done() {
        // attempts counts sends MADE: 1 = the original just failed
        assert_eq!(next_attempt_at(1, 1_000), Some(2_000), "first retry: +1s");
        assert_eq!(next_attempt_at(2, 1_000), Some(6_000), "second: +5s");
        assert_eq!(next_attempt_at(3, 1_000), Some(26_000), "third: +25s");
        assert_eq!(next_attempt_at(4, 1_000), None, "three retries, then stop");
        assert!(!is_abandoned(3));
        assert!(is_abandoned(4));
    }

    /// A failed effect comes back for another attempt only once its
    /// backoff has elapsed -- and never after the ladder is spent.
    #[test]
    fn a_failure_waits_its_backoff_and_then_gives_up_visibly() {
        let conn = cell();
        let (id, _) = enqueue(&conn, "int_r", "surface:chat", "deliver me").unwrap();
        mark(&conn, &id, "failed", Some("provider down")).unwrap();
        let failed_at: i64 = conn
            .query_row(
                "SELECT updated_at FROM outbox WHERE effect_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();

        // attempt 1 was the original send; the first RETRY waits 1s
        assert!(due_for_retry(&conn, failed_at + 500).unwrap().is_empty(), "too soon");
        let due = due_for_retry(&conn, failed_at + 1_500).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].2, "deliver me", "the payload comes back with it");

        // burn the remaining rungs
        mark(&conn, &id, "failed", Some("still down")).unwrap();
        mark(&conn, &id, "failed", Some("still down")).unwrap();
        mark(&conn, &id, "failed", Some("still down")).unwrap();
        assert!(
            due_for_retry(&conn, failed_at + 10_000_000).unwrap().is_empty(),
            "the ladder is spent; no amount of waiting brings it back"
        );

        // and it is visible as given-up rather than silently stuck
        let gone = abandoned(&conn).unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].1, "surface:chat");
        assert_eq!(gone[0].2, "still down", "the reason survives");
    }

    /// A delivery that succeeds on a retry must not also be retried again.
    #[test]
    fn a_recovered_effect_leaves_the_retry_queue() {
        let conn = cell();
        let (id, _) = enqueue(&conn, "int_s", "surface:chat", "x").unwrap();
        mark(&conn, &id, "failed", Some("blip")).unwrap();
        let t: i64 = conn
            .query_row("SELECT updated_at FROM outbox WHERE effect_id = ?1", params![id], |r| r.get(0))
            .unwrap();
        assert_eq!(due_for_retry(&conn, t + 2_000).unwrap().len(), 1);
        mark(&conn, &id, "sent", None).unwrap();
        assert!(due_for_retry(&conn, t + 2_000).unwrap().is_empty());
        assert!(abandoned(&conn).unwrap().is_empty());
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
