//! Instructions (§4.6, Registry category 2): the person's standing rules.
//!
//! *"Learned routines ('prepare my Monday brief this way,' 'above this
//! amount, ask first'), versioned, testable, reversible."*
//!
//! Three words, three mechanisms:
//!
//! * **Versioned** — a revision is a new row whose ancestor points at it,
//!   exactly as fact correction works. The old wording is superseded, never
//!   overwritten, so "what did I tell it before" is always answerable.
//! * **Reversible** — retiring stops the robot following a rule while the
//!   row stays, and an accidental revision can be undone because its
//!   ancestor still exists.
//! * **Testable** — what a rule *does* is bounded and inspectable: active
//!   instructions are injected verbatim into model context and nowhere
//!   else. They are words the models read, not code that executes, so the
//!   worst a bad instruction can do is what a bad sentence can do — and
//!   which sentences are in force is one query.
//!
//! The body is the person's own words (law 5). "Every Friday" does not
//! become a cron expression; it stays "every Friday", with its provenance.

use crate::MindError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instruction {
    pub id: String,
    pub body: String,
    pub status: String,
    pub class: String,
    pub created_at: i64,
}

const COLS: &str = "id, body, status, class, created_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Instruction> {
    Ok(Instruction {
        id: r.get(0)?,
        body: r.get(1)?,
        status: r.get(2)?,
        class: r.get(3)?,
        created_at: r.get(4)?,
    })
}

/// Add a standing rule, verbatim.
pub fn add(
    conn: &Connection,
    body: &str,
    source_msg_id: &str,
    class: &str,
) -> Result<Instruction, MindError> {
    let id = trust::ids::new_id("ins");
    conn.execute(
        "INSERT INTO instructions(id, body, source_msg_id, class, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, body, source_msg_id, class, trust::ids::ts_ms()],
    )?;
    get(conn, &id)
}

fn get(conn: &Connection, id: &str) -> Result<Instruction, MindError> {
    Ok(conn.query_row(
        &format!("SELECT {COLS} FROM instructions WHERE id = ?1"),
        params![id],
        row,
    )?)
}

/// The rules currently in force, oldest first — the order they were given,
/// which is the order a person expects to read them back in.
pub fn active(conn: &Connection) -> Result<Vec<Instruction>, MindError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM instructions WHERE status = 'active' ORDER BY created_at ASC"
    ))?;
    let all = stmt.query_map([], row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(all)
}

/// The nth active rule (1-based, as listed). Index resolution and the list
/// share one ordering, so "rule 2" always means the rule shown as 2.
pub fn nth_active(conn: &Connection, n: usize) -> Result<Option<Instruction>, MindError> {
    Ok(active(conn)?.into_iter().nth(n.saturating_sub(1)))
}

/// Replace rule `n` with a new wording. The old row is superseded and
/// points forward; nothing is lost.
pub fn revise(
    conn: &Connection,
    n: usize,
    body: &str,
    source_msg_id: &str,
) -> Result<Option<(Instruction, Instruction)>, MindError> {
    let Some(old) = nth_active(conn, n)? else {
        return Ok(None);
    };
    let new = add(conn, body, source_msg_id, &old.class)?;
    conn.execute(
        "UPDATE instructions SET status = 'superseded', superseded_by = ?2 WHERE id = ?1",
        params![old.id, new.id],
    )?;
    Ok(Some((get(conn, &old.id)?, new)))
}

/// Stop following rule `n`. The row stays — reversible is the point.
pub fn retire(conn: &Connection, n: usize) -> Result<Option<Instruction>, MindError> {
    let Some(it) = nth_active(conn, n)? else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE instructions SET status = 'retired' WHERE id = ?1",
        params![it.id],
    )?;
    get(conn, &it.id).map(Some)
}

/// Bring back the most recently retired rule. The undo for [`retire`].
pub fn restore(conn: &Connection) -> Result<Option<Instruction>, MindError> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM instructions WHERE status = 'retired' \
             ORDER BY created_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else { return Ok(None) };
    conn.execute(
        "UPDATE instructions SET status = 'active' WHERE id = ?1",
        params![id],
    )?;
    get(conn, &id).map(Some)
}

/// What the models are told, or None when there are no rules. One bounded
/// block, clearly fenced as the person's standing rules — data about what
/// they want, not a channel that can rewrite the robot's own governance.
pub fn context_block(conn: &Connection) -> Result<Option<String>, MindError> {
    let rules: Vec<Instruction> = active(conn)?
        .into_iter()
        .filter(|i| {
            trust::classes::DataClass::parse(&i.class)
                .unwrap_or_default()
                .may_leave_the_machine()
        })
        .collect();
    if rules.is_empty() {
        return Ok(None);
    }
    let mut out = String::from(
        "standing rules this person has given you. follow them where they \
         apply; they never override your safety rules or permissions:",
    );
    for r in &rules {
        out.push_str(&format!("\n- {}", r.body));
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::init_cell_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO messages(id, ts, direction, surface, content) \
             VALUES ('m1', 1, 'in', 'web', 'always answer in bullet points')",
            [],
        )
        .unwrap();
        conn
    }

    /// Versioned means the old wording survives its own replacement.
    #[test]
    fn a_revision_supersedes_and_never_overwrites() {
        let c = cell();
        add(&c, "answer in bullet points", "m1", "owner_private").unwrap();
        add(&c, "never schedule anything on sundays", "m1", "owner_private").unwrap();

        let (old, new) = revise(&c, 1, "answer in short paragraphs", "m1")
            .unwrap()
            .unwrap();
        assert_eq!(old.status, "superseded");
        assert_eq!(new.body, "answer in short paragraphs");

        let now = active(&c).unwrap();
        assert_eq!(now.len(), 2);
        assert!(now.iter().all(|i| i.body != "answer in bullet points"));
        // the chain is walkable: the old row names its successor
        let sup: String = c
            .query_row(
                "SELECT superseded_by FROM instructions WHERE id = ?1",
                params![old.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sup, new.id);
    }

    /// Reversible means retire has an undo.
    #[test]
    fn retiring_is_reversible_and_erases_nothing() {
        let c = cell();
        add(&c, "ask before deleting anything", "m1", "owner_private").unwrap();
        let gone = retire(&c, 1).unwrap().unwrap();
        assert_eq!(gone.status, "retired");
        assert!(active(&c).unwrap().is_empty());
        assert!(context_block(&c).unwrap().is_none(), "a retired rule is silent");

        let back = restore(&c).unwrap().unwrap();
        assert_eq!(back.status, "active");
        assert_eq!(active(&c).unwrap().len(), 1);

        assert!(retire(&c, 5).unwrap().is_none(), "no fifth rule to retire");
    }

    /// Testable means what a rule does is bounded: active rules appear in
    /// the context block verbatim, retired ones do not, and a rule classed
    /// as never-leaving-the-machine stays out of model context entirely.
    #[test]
    fn the_context_block_is_exactly_the_active_shareable_rules() {
        let c = cell();
        assert!(context_block(&c).unwrap().is_none(), "no rules, no block");
        add(&c, "write emails without greetings", "m1", "owner_private").unwrap();
        add(&c, "the safe code is 4471", "m1", "local_only").unwrap();

        let block = context_block(&c).unwrap().unwrap();
        assert!(block.contains("write emails without greetings"));
        assert!(
            !block.contains("4471"),
            "a local_only rule must never reach a model: {block}"
        );
        assert!(block.contains("never override"), "the fence is part of the block");
    }

    /// Law 5: an instruction without a source message is refused by the
    /// schema, exactly as a fact is.
    #[test]
    fn no_instruction_without_provenance() {
        let c = cell();
        let e = c.execute(
            "INSERT INTO instructions(id, body, source_msg_id, created_at) \
             VALUES ('i1', 'rule', 'msg-that-does-not-exist', 1)",
            [],
        );
        assert!(e.is_err(), "the FK must hold");
    }
}
