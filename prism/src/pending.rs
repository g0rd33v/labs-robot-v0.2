//! Calls that are waiting for a yes.
//!
//! An explicit instruction may delete; an inference may not. The English
//! floor is an instruction -- "forget fact 2" is unambiguous, and it runs.
//! A model reading a sentence in any language and concluding that deletion
//! was meant is an inference, however confident, and inference is not
//! consent.
//!
//! So an irreversible call proposed by a model is parked here, the person is
//! asked, and only their answer releases it. Parking is durable (arch: a
//! plan step awaiting approval survives a restart), which is why this is a
//! row rather than something held in memory.

use crate::{Cell, PrismError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use trust::ids;

/// How long a question stays answerable. Long enough to reply to, short
/// enough that a stale "yes" cannot detonate something the person has
/// forgotten agreeing to.
pub const TTL_MS: i64 = 10 * 60_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    pub intent_id: String,
    pub tool: String,
    pub args: serde_json::Value,
    pub created_at: i64,
}

pub fn init_schema(conn: &Connection) -> Result<(), PrismError> {
    conn.execute_batch(
        "
CREATE TABLE IF NOT EXISTS pending_calls (
    id         TEXT PRIMARY KEY,
    intent_id  TEXT NOT NULL,
    tool       TEXT NOT NULL,
    args_json  TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    state      TEXT NOT NULL CHECK (state IN ('open','confirmed','declined','expired'))
);
",
    )?;
    Ok(())
}

/// Park a call. Any older question is superseded: only the most recent
/// thing asked can be answered, so a bare "yes" is never ambiguous.
pub fn park(
    conn: &Connection,
    intent_id: &str,
    tool: &str,
    args: &serde_json::Value,
) -> Result<Pending, PrismError> {
    conn.execute(
        "UPDATE pending_calls SET state = 'expired' WHERE state = 'open'",
        [],
    )?;
    let p = Pending {
        id: ids::new_id("pend"),
        intent_id: intent_id.into(),
        tool: tool.into(),
        args: args.clone(),
        created_at: ids::ts_ms(),
    };
    conn.execute(
        "INSERT INTO pending_calls(id, intent_id, tool, args_json, created_at, state) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'open')",
        params![p.id, p.intent_id, p.tool, p.args.to_string(), p.created_at],
    )?;
    Ok(p)
}

/// The open question, if there is a live one. Expired rows are swept as
/// they are found rather than lingering as a trap.
pub fn open(conn: &Connection) -> Result<Option<Pending>, PrismError> {
    let row: Option<(String, String, String, String, i64)> = conn
        .query_row(
            "SELECT id, intent_id, tool, args_json, created_at FROM pending_calls \
             WHERE state = 'open' ORDER BY created_at DESC, rowid DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((id, intent_id, tool, args_json, created_at)) = row else {
        return Ok(None);
    };
    if ids::ts_ms() - created_at > TTL_MS {
        conn.execute(
            "UPDATE pending_calls SET state = 'expired' WHERE id = ?1",
            params![id],
        )?;
        return Ok(None);
    }
    Ok(Some(Pending {
        id,
        intent_id,
        tool,
        args: serde_json::from_str(&args_json).unwrap_or(serde_json::Value::Null),
        created_at,
    }))
}

/// Close a question. Conditional on it still being open, so the same yes
/// cannot fire twice -- a replayed turn must not delete a second time.
pub fn resolve(conn: &Connection, id: &str, state: &str) -> Result<bool, PrismError> {
    let n = conn.execute(
        "UPDATE pending_calls SET state = ?2 WHERE id = ?1 AND state = 'open'",
        params![id, state],
    )?;
    Ok(n == 1)
}

/// Is anything waiting on this cell? Used to decide whether the answering
/// tool is even offered.
pub fn is_waiting(cell: &Cell) -> bool {
    cell.with(open).ok().flatten().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Cell {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        Cell::new(conn)
    }

    #[test]
    fn only_the_latest_question_can_be_answered() {
        let cell = cell();
        cell.with(|c| {
            park(c, "int_1", "memory.forget", &serde_json::json!({"index": 1}))?;
            let second = park(c, "int_2", "memory.forget", &serde_json::json!({"index": 2}))?;
            let live = open(c)?.expect("one open question");
            assert_eq!(live.id, second.id);
            assert_eq!(live.args["index"], 2);
            Ok(())
        })
        .unwrap();
    }

    /// The same yes must not fire twice -- that is what a replayed turn
    /// looks like, and this one deletes.
    #[test]
    fn a_confirmation_is_spent_when_it_is_used() {
        let cell = cell();
        cell.with(|c| {
            let p = park(c, "int_1", "memory.forget", &serde_json::json!({"index": 1}))?;
            assert!(resolve(c, &p.id, "confirmed")?);
            assert!(!resolve(c, &p.id, "confirmed")?, "spent twice");
            assert!(open(c)?.is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_stale_question_cannot_be_answered() {
        let cell = cell();
        cell.with(|c| {
            let p = park(c, "int_1", "memory.forget", &serde_json::json!({"index": 1}))?;
            c.execute(
                "UPDATE pending_calls SET created_at = ?2 WHERE id = ?1",
                params![p.id, ids::ts_ms() - TTL_MS - 1],
            )?;
            assert!(open(c)?.is_none(), "a stale yes must not detonate");
            Ok(())
        })
        .unwrap();
    }
}
