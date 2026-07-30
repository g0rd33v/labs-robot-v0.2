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
pub fn cancel_latest(conn: &Connection) -> Result<Option<Reminder>, MindError> {
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
    if let Some(ref rem) = latest {
        conn.execute(
            "UPDATE reminders SET status = 'cancelled' WHERE id = ?1",
            params![rem.id],
        )?;
    }
    Ok(latest.map(|mut r| {
        r.status = "cancelled".into();
        r
    }))
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
        let cancelled = cancel_latest(&conn).unwrap().unwrap();
        assert_eq!(cancelled.about, "sooner"); // most recently created
        assert_eq!(count_active(&conn).unwrap(), 1);
        assert!(cancel_latest(&conn).unwrap().is_some());
        assert!(cancel_latest(&conn).unwrap().is_none()); // nothing left
    }
}
