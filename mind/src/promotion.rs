//! The mutation protocol's ladder (§4: *"models propose memory; Mind
//! decides"*), with Q21's thresholds.
//!
//! *"tentative → contextual: 2 independent sources or 1 explicit owner
//! statement. contextual → stable: 3 occurrences across ≥7 days or owner
//! confirmation in the Registry. Contradiction unresolved after 2 gentle
//! prompts: keep both, mark `contested`, prefer the newest in answers with
//! a hedge, surface in Registry."*
//!
//! Two things this module exists to prevent, both of which the code did
//! before it:
//!
//! * **Everything landing `stable` on first mention.** A fact the person
//!   said once, in passing, was being held with the same confidence as one
//!   they confirmed — so "you told me X" was sometimes a lie about how
//!   firmly they told it.
//! * **A contradiction silently overwriting.** Two incompatible things are
//!   evidence about the world, not a race to be last. Both survive, the
//!   pair is marked, and answers hedge instead of picking confidently.
//!
//! What "independent source" means here: a different source MESSAGE. Two
//! sentences in one message are one telling; the same claim next week is a
//! second. That is the strictest reading available without a model in the
//! loop, and it errs toward under-promotion, which is the safe direction.

use crate::MindError;
use rusqlite::{params, Connection};

/// Q21's second threshold: three occurrences must span at least this long.
pub const STABLE_SPAN_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Where a fact sits on the ladder. `contested` is a FLAG, not a rung
/// (spec §5.2) — a contested fact still has a status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    Tentative,
    Contextual,
    Stable,
}

impl Rung {
    pub fn as_str(&self) -> &'static str {
        match self {
            Rung::Tentative => "tentative",
            Rung::Contextual => "contextual",
            Rung::Stable => "stable",
        }
    }
}

/// The evidence behind one claim, gathered from the store.
#[derive(Debug, Clone, Copy, Default)]
pub struct Evidence {
    /// distinct source messages carrying this claim
    pub sources: usize,
    /// stated by the owner in their own words (not inferred from a document)
    pub owner_stated: bool,
    /// confirmed in the Registry
    pub owner_confirmed: bool,
    /// span between the first and last occurrence
    pub span_ms: i64,
}

/// Q21, applied. Deliberately a pure function over evidence: the thresholds
/// are a decision from the decisions document, and reading them should not
/// require reading a query.
pub fn rung_for(e: Evidence) -> Rung {
    // contextual → stable
    if e.owner_confirmed || (e.sources >= 3 && e.span_ms >= STABLE_SPAN_MS) {
        return Rung::Stable;
    }
    // tentative → contextual
    if e.owner_stated || e.sources >= 2 {
        return Rung::Contextual;
    }
    Rung::Tentative
}

/// Gather the evidence for one fact's content, then place it.
///
/// `owner_stated` is passed in rather than inferred: only the caller knows
/// whether this turn was the person typing or the robot extracting from a
/// document, and guessing that from stored rows is how an extraction gets
/// promoted as if it were a statement.
pub fn evidence_for(
    conn: &Connection,
    content: &str,
    owner_stated: bool,
) -> Result<Evidence, MindError> {
    let (sources, first, last): (i64, i64, i64) = conn.query_row(
        "SELECT count(DISTINCT source_msg_id), coalesce(min(created_at), 0), \
                coalesce(max(created_at), 0) \
         FROM facts WHERE content = ?1 AND status != 'superseded'",
        params![content],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let confirmed: i64 = conn.query_row(
        "SELECT count(*) FROM facts WHERE content = ?1 AND confirmed_at IS NOT NULL",
        params![content],
        |r| r.get(0),
    )?;
    Ok(Evidence {
        sources: sources.max(0) as usize,
        owner_stated,
        owner_confirmed: confirmed > 0,
        span_ms: (last - first).max(0),
    })
}

/// Place a fact on the ladder and write it back. Returns the rung.
///
/// Never demotes: evidence accumulates, and a fact that reached `stable`
/// does not fall back because a later query counted differently.
pub fn place(
    conn: &Connection,
    fact_id: &str,
    content: &str,
    owner_stated: bool,
) -> Result<Rung, MindError> {
    let rung = rung_for(evidence_for(conn, content, owner_stated)?);
    conn.execute(
        "UPDATE facts SET status = ?2, confidence = ?3 \
         WHERE id = ?1 AND status NOT IN ('superseded', 'stable')",
        params![
            fact_id,
            rung.as_str(),
            match rung {
                Rung::Tentative => 0.5,
                Rung::Contextual => 0.8,
                Rung::Stable => 1.0,
            }
        ],
    )?;
    Ok(rung)
}

/// Mark two facts as contradicting each other (Q21's third clause).
///
/// Both survive. `contested` is a flag on a separate table rather than a
/// status, because a contested fact is still tentative or stable — the
/// contradiction is a relationship, and relationships do not fit in an
/// enum on one row.
pub fn mark_contested(conn: &Connection, a: &str, b: &str) -> Result<(), MindError> {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    conn.execute(
        "INSERT OR IGNORE INTO fact_contests(fact_a, fact_b, noticed_at) VALUES (?1, ?2, ?3)",
        params![lo, hi, trust::ids::ts_ms()],
    )?;
    Ok(())
}

/// Is this fact contested by anything still standing?
pub fn is_contested(conn: &Connection, fact_id: &str) -> Result<bool, MindError> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM fact_contests c \
         JOIN facts f ON f.id = CASE WHEN c.fact_a = ?1 THEN c.fact_b ELSE c.fact_a END \
         WHERE (c.fact_a = ?1 OR c.fact_b = ?1) AND f.status != 'superseded'",
        params![fact_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Every live contradiction, newest first: (fact_a id+content, fact_b
/// id+content). What the Registry surfaces as "conflicting — pick one".
#[allow(clippy::type_complexity)]
pub fn contests(conn: &Connection) -> Result<Vec<(String, String, String, String)>, MindError> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.content, b.id, b.content FROM fact_contests c \
         JOIN facts a ON a.id = c.fact_a JOIN facts b ON b.id = c.fact_b \
         WHERE a.status != 'superseded' AND b.status != 'superseded' \
         ORDER BY c.noticed_at DESC",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
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

    fn say(conn: &Connection, id: &str, ts: i64) {
        conn.execute(
            "INSERT INTO messages(id, ts, direction, surface, content) \
             VALUES (?1, ?2, 'in', 'web', 'said')",
            params![id, ts],
        )
        .unwrap();
    }

    fn fact(conn: &Connection, id: &str, content: &str, src: &str, ts: i64) {
        conn.execute(
            "INSERT INTO facts(id, content, source_msg_id, status, created_at) \
             VALUES (?1, ?2, ?3, 'tentative', ?4)",
            params![id, content, src, ts],
        )
        .unwrap();
    }

    /// Q21's thresholds, read straight off the decision.
    #[test]
    fn the_ladder_is_exactly_q21() {
        // one passing mention: tentative
        assert_eq!(rung_for(Evidence { sources: 1, ..Default::default() }), Rung::Tentative);
        // 2 independent sources OR 1 explicit owner statement: contextual
        assert_eq!(rung_for(Evidence { sources: 2, ..Default::default() }), Rung::Contextual);
        assert_eq!(
            rung_for(Evidence { sources: 1, owner_stated: true, ..Default::default() }),
            Rung::Contextual
        );
        // 3 occurrences across >= 7 days: stable
        assert_eq!(
            rung_for(Evidence { sources: 3, span_ms: STABLE_SPAN_MS, ..Default::default() }),
            Rung::Stable
        );
        // 3 occurrences in one afternoon is NOT stable -- the span is the point
        assert_eq!(
            rung_for(Evidence { sources: 3, span_ms: 3_600_000, ..Default::default() }),
            Rung::Contextual
        );
        // owner confirmation in the Registry: stable, on its own
        assert_eq!(
            rung_for(Evidence { sources: 1, owner_confirmed: true, ..Default::default() }),
            Rung::Stable
        );
    }

    /// Evidence is counted from the store, and repetition inside one
    /// message is one telling, not two.
    #[test]
    fn two_tellings_promote_but_one_message_twice_does_not() {
        let c = cell();
        say(&c, "m1", 1_000);
        fact(&c, "f1", "i drink green tea", "m1", 1_000);
        assert_eq!(place(&c, "f1", "i drink green tea", false).unwrap(), Rung::Tentative);

        // the same message again: still one source
        fact(&c, "f1b", "i drink green tea", "m1", 1_100);
        assert_eq!(place(&c, "f1b", "i drink green tea", false).unwrap(), Rung::Tentative);

        // a different message next week: two sources -> contextual
        say(&c, "m2", 1_000 + STABLE_SPAN_MS);
        fact(&c, "f2", "i drink green tea", "m2", 1_000 + STABLE_SPAN_MS);
        assert_eq!(place(&c, "f2", "i drink green tea", false).unwrap(), Rung::Contextual);

        // a third, still spanning the week -> stable
        say(&c, "m3", 2_000 + STABLE_SPAN_MS);
        fact(&c, "f3", "i drink green tea", "m3", 2_000 + STABLE_SPAN_MS);
        assert_eq!(place(&c, "f3", "i drink green tea", false).unwrap(), Rung::Stable);
    }

    /// Owner confirmation short-circuits the whole ladder, and a fact never
    /// falls back down it.
    #[test]
    fn confirmation_makes_it_stable_and_nothing_demotes() {
        let c = cell();
        say(&c, "m1", 1_000);
        fact(&c, "f1", "my daughter is vera", "m1", 1_000);
        place(&c, "f1", "my daughter is vera", false).unwrap();

        c.execute("UPDATE facts SET confirmed_at = 5 WHERE id = 'f1'", []).unwrap();
        assert_eq!(place(&c, "f1", "my daughter is vera", false).unwrap(), Rung::Stable);
        let status: String = c
            .query_row("SELECT status FROM facts WHERE id = 'f1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "stable");

        // a later placement with weaker evidence must not demote it
        c.execute("UPDATE facts SET confirmed_at = NULL WHERE id = 'f1'", []).unwrap();
        place(&c, "f1", "my daughter is vera", false).unwrap();
        let status: String = c
            .query_row("SELECT status FROM facts WHERE id = 'f1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "stable", "evidence accumulates; it does not evaporate");
    }

    /// A contradiction keeps BOTH facts and is visible as a pair.
    #[test]
    fn a_contradiction_keeps_both_and_surfaces() {
        let c = cell();
        say(&c, "m1", 1_000);
        say(&c, "m2", 2_000);
        fact(&c, "f1", "i am vegetarian", "m1", 1_000);
        fact(&c, "f2", "i eat fish now", "m2", 2_000);

        mark_contested(&c, "f2", "f1").unwrap();
        assert!(is_contested(&c, "f1").unwrap());
        assert!(is_contested(&c, "f2").unwrap());
        // order does not create a second contest
        mark_contested(&c, "f1", "f2").unwrap();
        assert_eq!(contests(&c).unwrap().len(), 1);

        // both rows still exist -- neither was overwritten
        let n: i64 = c
            .query_row("SELECT count(*) FROM facts WHERE status != 'superseded'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);

        // superseding one ends the contest: there is no longer a conflict
        c.execute("UPDATE facts SET status = 'superseded' WHERE id = 'f1'", []).unwrap();
        assert!(!is_contested(&c, "f2").unwrap());
        assert!(contests(&c).unwrap().is_empty());
    }
}
