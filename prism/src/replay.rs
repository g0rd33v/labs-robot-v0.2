//! Crash replay (arch sec 3): on boot, every intent without a terminal
//! close resumes from its last completed journal step -- never repeating a
//! tool call, an outbound message, or a mutation. An intent without a
//! terminal receipt is a visible bug, not a silent drop.

use crate::lifecycle::{
    finish_planned_intent, interrupted_receipt, plan_from_decision, Decision, TurnDeps,
};
use crate::types::Plan;
use crate::verdict::VerdictProvider;
use crate::{journal, lifecycle::CapabilityRouter, outbox, receipts, PrismError};
use rusqlite::Connection;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReplaySummary {
    /// resumed to a terminal state by re-running remaining steps
    pub resumed: usize,
    /// closed as failed (interrupted before any decision was journaled)
    pub closed_failed: usize,
}

/// A verdict provider that must never be consulted during replay: decisions
/// are journaled; replay replays them, it does not re-decide.
struct NoReDecide;
impl VerdictProvider for NoReDecide {
    fn verdict(&self, _text: &str) -> crate::types::Verdict {
        unreachable!("replay must not re-run verdicts; decisions are journaled")
    }
}

/// Resume every open intent. Idempotent: a second call finds nothing to do.
pub fn resume_incomplete(
    cell: &Connection,
    router: &dyn CapabilityRouter,
) -> Result<ReplaySummary, PrismError> {
    let mut summary = ReplaySummary::default();
    let deps = TurnDeps {
        router,
        verdicts: &NoReDecide,
        crash: None,
    };

    for intent_id in journal::open_intents(cell)? {
        // receipt already exists -> just close honestly on it
        if let Some(r) = receipts::get(cell, &intent_id)? {
            fail_undelivered_replies(cell, &intent_id)?;
            journal::intent_close(cell, &intent_id, r.status.as_str())?;
            summary.resumed += 1;
            continue;
        }

        // a plan (journaled or rebuildable from the journaled decision)?
        let plan: Option<Plan> = match journal::payload_of(cell, &intent_id, "plan")? {
            Some(p) => Some(serde_json::from_str(&p)?),
            None => match journal::payload_of(cell, &intent_id, "decision")? {
                Some(d) => {
                    let decision: Decision = serde_json::from_str(&d)?;
                    // the original content travels in the intent_open payload
                    let content = journal::payload_of(cell, &intent_id, "intent_open")?
                        .and_then(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
                        .and_then(|v| v["content"].as_str().map(String::from))
                        .unwrap_or_default();
                    let plan = plan_from_decision(&intent_id, &decision, &content);
                    journal::step(
                        cell,
                        &intent_id,
                        "plan",
                        &serde_json::to_string(&plan)?,
                        None,
                    )?;
                    Some(plan)
                }
                None => None,
            },
        };

        match plan {
            Some(plan) => {
                // finish exactly as a live turn would; journaled outcomes are
                // reused, remaining steps execute idempotently
                finish_planned_intent(cell, &intent_id, &plan, &deps, false)?;
                summary.resumed += 1;
            }
            None => {
                // died before any decision: no effect ran; close honestly
                let receipt = receipts::store(cell, &interrupted_receipt(&intent_id))?;
                journal::step(
                    cell,
                    &intent_id,
                    "receipt",
                    &serde_json::json!({
                        "receipt_id": receipt.receipt_id,
                        "status": receipt.status.as_str()
                    })
                    .to_string(),
                    None,
                )?;
                journal::intent_close(cell, &intent_id, receipt.status.as_str())?;
                summary.closed_failed += 1;
            }
        }
    }
    Ok(summary)
}

/// Replies that never left before the crash: the asking session is gone, so
/// the honest terminal state is `failed`, with the reason recorded.
fn fail_undelivered_replies(cell: &Connection, intent_id: &str) -> Result<(), PrismError> {
    for (effect_id, state) in outbox::for_intent(cell, intent_id)? {
        if state == "pending" || state == "sent" {
            outbox::mark(
                cell,
                &effect_id,
                "failed",
                Some("crash before delivery; no live session"),
            )?;
        }
    }
    Ok(())
}
