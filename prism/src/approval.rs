//! Durable interrupts (arch §3b.2).
//!
//! *"A plan step with `approval: required` parks the intent in the journal
//! as `awaiting_approval`; the approval resumes execution from that
//! checkpoint — minutes or days later, across restarts, from any surface.
//! Human-in-the-loop that survives reboots; no in-memory waiting, ever."*
//!
//! The mechanism is the journal, not a queue and not a timer. A parked
//! intent is simply an open intent whose last state is `awaiting_approval`,
//! which means it survives a crash for free — the same property that makes
//! replay work makes waiting work.
//!
//! Two consequences that shape the code:
//!
//! * **Replay must not resume a parked intent.** It is not stalled; it is
//!   waiting for a person. Resuming it on boot would execute the very thing
//!   the approval exists to gate.
//! * **The authority is minted at approval, not at parking.** A grant
//!   issued before the wait would either have to outlive the wait — which
//!   defeats time-boxing — or expire during it. The approval *is* the
//!   fresh authority, so the grant is issued when the answer arrives.

use crate::types::*;
use crate::{journal, Cell, PrismError};
use serde::{Deserialize, Serialize};

pub const AWAITING: &str = "awaiting_approval";
pub const RESOLVED: &str = "approval_resolved";

/// A step waiting for a person.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parked {
    pub intent_id: String,
    pub step_id: String,
    pub capability: String,
    pub effect: Effect,
    pub args: serde_json::Value,
    pub asked_at: i64,
}

/// Park a step and record why, so the Dashboard and the chat can both show
/// what is waiting without re-deriving it.
pub fn park(cell: &Cell, intent_id: &str, step: &PlanStep) -> Result<Parked, PrismError> {
    let p = Parked {
        intent_id: intent_id.into(),
        step_id: step.step_id.clone(),
        capability: step.capability.clone(),
        effect: step.effect,
        args: step.args.clone(),
        asked_at: trust::ids::ts_ms(),
    };
    let payload = serde_json::to_string(&p)?;
    cell.with(|c| journal::step(c, intent_id, AWAITING, &payload, None))?;
    Ok(p)
}

/// Everything currently waiting on this cell, oldest first.
///
/// Derived from the journal rather than a separate table: a second store
/// could disagree with the journal, and then the question "what is waiting"
/// would have two answers.
pub fn waiting(cell: &Cell) -> Result<Vec<Parked>, PrismError> {
    let mut out = vec![];
    for intent_id in cell.with(journal::open_intents)? {
        if let Some(p) = waiting_for(cell, &intent_id)? {
            out.push(p);
        }
    }
    out.sort_by_key(|p| p.asked_at);
    Ok(out)
}

/// Is this specific intent parked, and on what?
pub fn waiting_for(cell: &Cell, intent_id: &str) -> Result<Option<Parked>, PrismError> {
    let kinds = cell.with(|c| journal::kinds_for_intent(c, intent_id))?;
    // resolved after parked means it is no longer waiting
    let last_park = kinds.iter().rposition(|k| k == AWAITING);
    let last_resolve = kinds.iter().rposition(|k| k == RESOLVED);
    match (last_park, last_resolve) {
        (Some(p), r) if r.map(|r| r < p).unwrap_or(true) => {
            let payload = cell.with(|c| journal::payload_of(c, intent_id, AWAITING))?;
            Ok(payload.and_then(|p| serde_json::from_str(&p).ok()))
        }
        _ => Ok(None),
    }
}

/// Record the person's answer. Conditional on the intent still waiting, so
/// two taps on the same approval cannot execute it twice.
pub fn resolve(cell: &Cell, intent_id: &str, approved: bool) -> Result<bool, PrismError> {
    if waiting_for(cell, intent_id)?.is_none() {
        return Ok(false);
    }
    let payload = serde_json::json!({
        "approved": approved,
        "at": trust::ids::ts_ms(),
    })
    .to_string();
    cell.with(|c| journal::step(c, intent_id, RESOLVED, &payload, None))?;
    Ok(true)
}

/// Answer a parked intent and, if approved, run the rest of its plan.
///
/// The authority is minted HERE rather than at parking: a grant issued
/// before the wait would have to outlive it -- which defeats time-boxing --
/// or expire during it. The approval is the fresh authority.
pub fn respond(
    cell: &Cell,
    intent_id: &str,
    approved: bool,
    deps: &crate::TurnDeps,
) -> Result<Option<crate::TurnOutput>, PrismError> {
    let Some(parked) = waiting_for(cell, intent_id)? else {
        return Ok(None);
    };
    if !resolve(cell, intent_id, approved)? {
        return Ok(None);
    }
    let plan: Plan = match cell.with(|c| journal::payload_of(c, intent_id, "plan"))? {
        Some(p) => serde_json::from_str(&p)?,
        None => return Ok(None),
    };
    if !approved {
        // declined: close the intent honestly. Nothing ran, and the receipt
        // says so rather than staying silent.
        let out = crate::lifecycle::close_declined(cell, intent_id, &plan, deps, &parked)?;
        return Ok(Some(out));
    }
    let out = crate::lifecycle::finish_planned_intent_with(
        cell,
        intent_id,
        &plan,
        deps,
        true,
        std::slice::from_ref(&parked.step_id),
    )?;
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn cell() -> Cell {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_cell_schema(&conn).unwrap();
        Cell::new(conn)
    }

    fn step() -> PlanStep {
        PlanStep {
            step_id: "s1".into(),
            capability: "member.invite".into(),
            args: serde_json::json!({}),
            effect: Effect::ReversibleWrite,
            approval: Approval::Required,
            deps: vec![],
        }
    }

    #[test]
    fn a_parked_intent_is_visible_until_it_is_answered() {
        let c = cell();
        c.with(|conn| journal::intent_open(conn, "int_1", "{}")).unwrap();
        park(&c, "int_1", &step()).unwrap();

        let w = waiting(&c).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].capability, "member.invite");

        assert!(resolve(&c, "int_1", true).unwrap());
        assert!(waiting(&c).unwrap().is_empty());
    }

    /// Two taps on one approval must not execute it twice. The second
    /// answer finds nothing waiting and says so.
    #[test]
    fn an_approval_can_only_be_spent_once() {
        let c = cell();
        c.with(|conn| journal::intent_open(conn, "int_1", "{}")).unwrap();
        park(&c, "int_1", &step()).unwrap();

        assert!(resolve(&c, "int_1", true).unwrap());
        assert!(
            !resolve(&c, "int_1", true).unwrap(),
            "a second approval has nothing to spend"
        );
    }

    /// Declining closes the wait as surely as approving does -- a "no" that
    /// left the intent parked would ask again forever.
    #[test]
    fn declining_also_ends_the_wait() {
        let c = cell();
        c.with(|conn| journal::intent_open(conn, "int_1", "{}")).unwrap();
        park(&c, "int_1", &step()).unwrap();
        assert!(resolve(&c, "int_1", false).unwrap());
        assert!(waiting(&c).unwrap().is_empty());
    }
}
