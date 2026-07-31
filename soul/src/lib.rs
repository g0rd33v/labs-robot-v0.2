//! Soul: the relationship model (arch sec 5, design in `docs/SOUL.md`).
//!
//! **Soul is the renderer's policy.** The kernel emits `Rendering { id,
//! slots }` compiled from evidence; the surface turns that into words; this
//! decides how. That ordering is what makes sec 5's boundary -- *shapes
//! expression, never overrides facts* -- true by construction rather than
//! by discipline: nothing here ever sees a decision, only a decided result,
//! and the receipt is compiled and stored before the renderer is called.
//!
//! S1 is state and control: the dial, its bounds, and the owner's switches.
//! There is deliberately no adaptation yet. A robot should be inspectable
//! and controllable before it is adaptive -- if the dial cannot be read and
//! pinned by hand, nobody should be letting it move on its own.

pub mod dial;
pub mod express;
pub mod stance;

use rusqlite::Connection;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SoulError {
    #[error("sql: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("{0}")]
    Refused(String),
}

/// The four tables of Q25, per member cell.
///
/// `soul_lessons.evidence_msg_id` is a real foreign key to `messages`, in
/// the same way `facts.source_msg_id` is: law 5 applies to a lesson about
/// how someone likes to be spoken to exactly as it applies to a fact about
/// them. Nothing writes to it before S4; the constraint exists from the
/// start so it cannot be forgotten later.
pub fn init_cell_schema(conn: &Connection) -> Result<(), SoulError> {
    conn.execute_batch(
        "
CREATE TABLE IF NOT EXISTS soul_persona (
    dimension  TEXT PRIMARY KEY,
    value      INTEGER NOT NULL,
    floor      INTEGER NOT NULL,
    ceiling    INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (floor >= 0 AND ceiling <= 100 AND floor <= ceiling),
    CHECK (value >= floor AND value <= ceiling)
);
CREATE TABLE IF NOT EXISTS soul_lessons (
    id               TEXT PRIMARY KEY,
    statement        TEXT NOT NULL,
    dimension        TEXT,
    direction        INTEGER NOT NULL DEFAULT 0,
    evidence_msg_id  TEXT NOT NULL REFERENCES messages(id),
    status           TEXT NOT NULL DEFAULT 'proposed'
                     CHECK (status IN ('proposed','active','retired')),
    confidence       REAL NOT NULL DEFAULT 0.5,
    created_at       INTEGER NOT NULL,
    reinforced_count INTEGER NOT NULL DEFAULT 0,
    retired_at       INTEGER
);
CREATE TABLE IF NOT EXISTS soul_revisions (
    id                TEXT PRIMARY KEY,
    created_at        INTEGER NOT NULL,
    reason            TEXT NOT NULL,
    diff_json         TEXT NOT NULL,
    rolls_back_to     TEXT,
    applied           INTEGER NOT NULL DEFAULT 0,
    evaluator_verdict TEXT
);
CREATE TABLE IF NOT EXISTS soul_journal (
    id          TEXT PRIMARY KEY,
    created_at  INTEGER NOT NULL,
    revision_id TEXT,
    entry       TEXT NOT NULL
);
",
    )?;
    Ok(())
}
