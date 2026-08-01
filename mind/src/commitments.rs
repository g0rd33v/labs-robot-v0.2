//! The commitment ledger (§4.5).
//!
//! *"What the owner asked, what the Robot promised, deadlines, waiting
//! conditions, delegations, why each commitment closed. The Second Law —
//! never silently drop a request — cannot be implemented with chat
//! history; it is implemented here."*
//!
//! The design is one invariant carried three ways:
//!
//! * **Closing requires a reason** — a schema CHECK, like the source FK on
//!   facts. There is no way to write a closed commitment with no `why`, so
//!   "why did this end" is always answerable and a silent drop is not a
//!   discipline, it is a constraint violation.
//! * **Openings are hooked, not remembered.** A reminder's creation opens a
//!   commitment inside the same call; parking an approval opens one the
//!   same way. Nothing depends on a caller remembering to log the ask.
//! * **Ids are derived, not random.** The commitment for reminder X is
//!   `cmt_<X>` on every instance, so two robots that both hold the reminder
//!   hold ONE commitment after a sync instead of breeding duplicates.
//!
//! What belongs here is DEFERRED work — anything whose fulfilment is not
//! the reply itself. A question answered in the same turn is not a
//! commitment; a reminder, a parked approval, or a promise to do something
//! later is.

use crate::MindError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Commitment {
    pub id: String,
    pub what: String,
    pub kind: String,
    pub status: String,
    pub due_at: Option<i64>,
    pub created_at: i64,
    pub closed_at: Option<i64>,
    pub closed_why: Option<String>,
}

const COLS: &str = "id, what, kind, status, due_at, created_at, closed_at, closed_why";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Commitment> {
    Ok(Commitment {
        id: r.get(0)?,
        what: r.get(1)?,
        kind: r.get(2)?,
        status: r.get(3)?,
        due_at: r.get(4)?,
        created_at: r.get(5)?,
        closed_at: r.get(6)?,
        closed_why: r.get(7)?,
    })
}

/// The derived id: one backing thing, one commitment, on every instance.
pub fn id_for(backing_id: &str) -> String {
    format!("cmt_{backing_id}")
}

/// Open a commitment. Idempotent by derived id, so the hook that opens it
/// can run on replay without breeding a second ledger entry.
#[allow(clippy::too_many_arguments)]
pub fn open(
    conn: &Connection,
    backing_id: &str,
    what: &str,
    kind: &str,
    status: &str,
    source_msg_id: Option<&str>,
    intent_id: Option<&str>,
    due_at: Option<i64>,
) -> Result<(), MindError> {
    conn.execute(
        "INSERT OR IGNORE INTO commitments(id, what, kind, status, source_msg_id, \
         intent_id, due_at, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            id_for(backing_id),
            what,
            kind,
            status,
            source_msg_id,
            intent_id,
            due_at,
            trust::ids::ts_ms()
        ],
    )?;
    Ok(())
}

/// Close a commitment, with the reason — there is no version of this call
/// without one. Closing an already-closed commitment changes nothing: the
/// first reason stands, because it is the one that was true at the time.
pub fn close(
    conn: &Connection,
    backing_id: &str,
    status: &str,
    why: &str,
) -> Result<bool, MindError> {
    let n = conn.execute(
        "UPDATE commitments SET status = ?2, closed_at = ?3, closed_why = ?4 \
         WHERE id = ?1 AND closed_at IS NULL",
        params![id_for(backing_id), status, trust::ids::ts_ms(), why],
    )?;
    Ok(n > 0)
}

/// Everything still owed: open and waiting, oldest ask first.
pub fn outstanding(conn: &Connection) -> Result<Vec<Commitment>, MindError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM commitments WHERE closed_at IS NULL ORDER BY created_at ASC"
    ))?;
    let all = stmt.query_map([], row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(all)
}

/// The most recently closed, newest first — the "and why" half of the
/// screen.
pub fn recently_closed(conn: &Connection, limit: usize) -> Result<Vec<Commitment>, MindError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM commitments WHERE closed_at IS NOT NULL \
         ORDER BY closed_at DESC LIMIT ?1"
    ))?;
    let all = stmt
        .query_map(params![limit as i64], row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(all)
}

/// Is this backing thing's commitment still open?
pub fn is_open(conn: &Connection, backing_id: &str) -> Result<bool, MindError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM commitments WHERE id = ?1 AND closed_at IS NULL",
            params![id_for(backing_id)],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
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

    /// The Second Law as a constraint: the schema has no way to hold a
    /// closed commitment with no reason.
    #[test]
    fn a_commitment_cannot_close_without_a_why() {
        let c = cell();
        open(&c, "rem_1", "call mark", "reminder", "open", None, None, Some(99)).unwrap();

        let e = c.execute(
            "UPDATE commitments SET status = 'done', closed_at = 5 WHERE id = 'cmt_rem_1'",
            [],
        );
        assert!(e.is_err(), "closed with no why must violate the CHECK");

        // and an open one cannot pretend to be closed
        let e = c.execute(
            "UPDATE commitments SET status = 'done' WHERE id = 'cmt_rem_1'",
            [],
        );
        assert!(e.is_err(), "a terminal status with no closed_at is a lie");

        assert!(close(&c, "rem_1", "done", "fired on time").unwrap());
        let closed = recently_closed(&c, 10).unwrap();
        assert_eq!(closed[0].closed_why.as_deref(), Some("fired on time"));
        assert!(outstanding(&c).unwrap().is_empty());
    }

    /// The first reason stands. A close that arrives second — a sync from
    /// the other instance, a replay — must not rewrite history.
    #[test]
    fn the_first_close_wins_and_open_is_idempotent() {
        let c = cell();
        open(&c, "rem_1", "call mark", "reminder", "open", None, None, None).unwrap();
        open(&c, "rem_1", "call mark", "reminder", "open", None, None, None).unwrap();
        assert_eq!(outstanding(&c).unwrap().len(), 1, "one ask, one entry");

        assert!(close(&c, "rem_1", "cancelled", "cancelled by you").unwrap());
        assert!(!close(&c, "rem_1", "done", "fired on time").unwrap());
        let closed = recently_closed(&c, 1).unwrap();
        assert_eq!(closed[0].status, "cancelled");
        assert_eq!(closed[0].closed_why.as_deref(), Some("cancelled by you"));
    }

    #[test]
    fn the_screen_splits_owed_from_settled() {
        let c = cell();
        open(&c, "a", "first ask", "reminder", "open", None, None, None).unwrap();
        open(&c, "b", "second ask", "approval", "waiting", None, Some("int_9"), None).unwrap();
        open(&c, "d", "third ask", "promise", "open", None, None, None).unwrap();
        close(&c, "d", "failed", "the model call failed; nothing ran").unwrap();

        let owed = outstanding(&c).unwrap();
        assert_eq!(owed.len(), 2);
        assert_eq!(owed[0].what, "first ask", "oldest ask first");
        assert!(is_open(&c, "b").unwrap());
        assert!(!is_open(&c, "d").unwrap());
        let settled = recently_closed(&c, 5).unwrap();
        assert_eq!(settled.len(), 1);
        assert!(settled[0].closed_why.as_deref().unwrap().contains("nothing ran"));
    }
}
