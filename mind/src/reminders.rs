//! Reminders: the first entries of the commitment ledger (arch sec 4, store
//! #5). Creation is idempotent per intent -- re-execution after a crash
//! cannot duplicate a reminder (UNIQUE(intent_id) is the constraint, not a
//! convention).

use crate::MindError;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq)]
pub struct Reminder {
    pub id: String,
    pub intent_id: String,
    pub created_at: i64,
    pub fire_at: i64,
    pub about: String,
    pub status: String,
}

fn row_to_reminder(r: &rusqlite::Row<'_>) -> Result<Reminder, rusqlite::Error> {
    Ok(Reminder {
        id: r.get(0)?,
        intent_id: r.get(1)?,
        created_at: r.get(2)?,
        fire_at: r.get(3)?,
        about: r.get(4)?,
        status: r.get(5)?,
    })
}

const COLS: &str = "id, intent_id, created_at, fire_at, about, status";

/// Idempotent create: the same intent always yields the same reminder.
pub fn create(
    conn: &Connection,
    intent_id: &str,
    fire_at: i64,
    about: &str,
) -> Result<Reminder, MindError> {
    conn.execute(
        "INSERT OR IGNORE INTO reminders(id, intent_id, created_at, fire_at, about) \
         VALUES (?1,?2,?3,?4,?5)",
        params![
            trust::ids::new_id("rem"),
            intent_id,
            trust::ids::ts_ms(),
            fire_at,
            about
        ],
    )?;
    let reminder = conn.query_row(
        &format!("SELECT {COLS} FROM reminders WHERE intent_id = ?1"),
        params![intent_id],
        row_to_reminder,
    )?;
    // the ask enters the ledger (sec 4.5) in the same call that accepts it,
    // so no caller can create the promise and forget the accounting
    crate::commitments::open(
        conn,
        &reminder.id,
        &reminder.about,
        "reminder",
        "open",
        None,
        Some(intent_id),
        Some(reminder.fire_at),
    )?;
    Ok(reminder)
}

/// Active reminders, soonest first.
pub fn list_active(conn: &Connection) -> Result<Vec<Reminder>, MindError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM reminders WHERE status = 'active' ORDER BY fire_at ASC"
    ))?;
    let rows = stmt
        .query_map([], row_to_reminder)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Cancel the most recently created active reminder; returns it, if any.
///
/// Idempotent per intent (the CapabilityRouter contract): "the latest" is
/// resolved at execution time, so a crash between the UPDATE and the
/// journal write would otherwise make replay cancel a SECOND reminder while
/// the receipt claimed one. The op marker records what this intent actually
/// cancelled and replays that answer instead of re-resolving.
pub fn cancel_latest(conn: &Connection, intent_id: &str) -> Result<Option<Reminder>, MindError> {
    if let Some(marker) = crate::facts::op_marker(conn, intent_id)? {
        let id = marker["id"].as_str().unwrap_or_default();
        return Ok(conn
            .query_row(
                &format!("SELECT {COLS} FROM reminders WHERE id = ?1"),
                params![id],
                row_to_reminder,
            )
            .optional()?);
    }
    let latest: Option<Reminder> = conn
        .query_row(
            &format!(
                "SELECT {COLS} FROM reminders WHERE status = 'active' \
                 ORDER BY created_at DESC, rowid DESC LIMIT 1"
            ),
            [],
            row_to_reminder,
        )
        .optional()?;
    let tx = conn.unchecked_transaction()?;
    match &latest {
        Some(rem) => {
            tx.execute(
                "UPDATE reminders SET status = 'cancelled' WHERE id = ?1",
                params![rem.id],
            )?;
            crate::facts::set_op_marker(
                &tx,
                intent_id,
                &serde_json::json!({ "op": "cancel_latest", "id": rem.id }),
            )?;
            crate::commitments::close(&tx, &rem.id, "cancelled", "cancelled by you")?;
        }
        None => {
            crate::facts::set_op_marker(
                &tx,
                intent_id,
                &serde_json::json!({ "op": "cancel_latest", "id": null }),
            )?;
        }
    }
    tx.commit()?;
    Ok(latest.map(|mut r| {
        r.status = "cancelled".into();
        r
    }))
}

/// Reminders whose time has come: active with fire_at <= now.
pub fn due_active(conn: &Connection, now_ms: i64) -> Result<Vec<Reminder>, MindError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM reminders WHERE status = 'active' AND fire_at <= ?1 \
         ORDER BY fire_at ASC"
    ))?;
    let rows = stmt
        .query_map(params![now_ms], row_to_reminder)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The commitment closed: it fired.
pub fn mark_fired(conn: &Connection, id: &str) -> Result<(), MindError> {
    conn.execute(
        "UPDATE reminders SET status = 'fired' WHERE id = ?1",
        params![id],
    )?;
    crate::commitments::close(conn, id, "done", "fired on time")?;
    Ok(())
}

pub fn count_active(conn: &Connection) -> Result<i64, MindError> {
    Ok(conn.query_row(
        "SELECT count(*) FROM reminders WHERE status = 'active'",
        [],
        |r| r.get(0),
    )?)
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
    fn create_is_idempotent_per_intent() {
        let conn = cell();
        let a = create(&conn, "int_1", 1000, "call mark").unwrap();
        let b = create(&conn, "int_1", 1000, "call mark").unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(count_active(&conn).unwrap(), 1);
        // a different intent is a different reminder
        create(&conn, "int_2", 2000, "stretch").unwrap();
        assert_eq!(count_active(&conn).unwrap(), 2);
    }

    #[test]
    fn list_and_cancel() {
        let conn = cell();
        create(&conn, "int_1", 2000, "later").unwrap();
        create(&conn, "int_2", 1000, "sooner").unwrap();
        let listed = list_active(&conn).unwrap();
        assert_eq!(listed[0].about, "sooner"); // soonest first
        let cancelled = cancel_latest(&conn, "int_c1").unwrap().unwrap();
        assert_eq!(cancelled.about, "sooner"); // most recently created
        assert_eq!(count_active(&conn).unwrap(), 1);
        assert!(cancel_latest(&conn, "int_c2").unwrap().is_some());
        assert!(cancel_latest(&conn, "int_c3").unwrap().is_none()); // nothing left
    }

    /// Regression: "the latest" is resolved at execution time, so replaying
    /// the same intent must NOT cancel a second reminder.
    #[test]
    fn cancel_is_idempotent_per_intent() {
        let conn = cell();
        create(&conn, "int_1", 2000, "first").unwrap();
        create(&conn, "int_2", 1000, "second").unwrap();
        assert_eq!(count_active(&conn).unwrap(), 2);

        let a = cancel_latest(&conn, "int_cancel").unwrap().unwrap();
        assert_eq!(count_active(&conn).unwrap(), 1);
        // crash replay: same intent runs again
        let b = cancel_latest(&conn, "int_cancel").unwrap().unwrap();
        assert_eq!(a.id, b.id, "replay must return the same reminder");
        assert_eq!(
            count_active(&conn).unwrap(),
            1,
            "replay must not cancel a second reminder"
        );

        // a nothing-to-cancel result is also recorded, so replay stays quiet
        let conn2 = cell();
        assert!(cancel_latest(&conn2, "int_empty").unwrap().is_none());
        create(&conn2, "int_new", 5000, "arrived later").unwrap();
        assert!(
            cancel_latest(&conn2, "int_empty").unwrap().is_none(),
            "replay of an empty cancel must not eat a newly created reminder"
        );
        assert_eq!(count_active(&conn2).unwrap(), 1);
    }
}
