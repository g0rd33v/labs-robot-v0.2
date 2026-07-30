//! The governed lifecycle (arch sec 3):
//! Intent -> decision (floor | verdict) -> Plan -> Grant -> Execution ->
//! Verification -> Receipt -> Response. Every stage journaled; a sentence
//! claiming an effect may only be produced from a verified state transition.

use crate::floor::{self, FloorMatch};
use crate::types::*;
use crate::verdict::VerdictProvider;
use crate::{journal, outbox, receipts, Cell, Envelope, PrismError};
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use trust::ids;

/// Executes one capability step inside the member's cell. Implementations
/// MUST be idempotent per intent: re-execution after a crash must not
/// duplicate the effect.
///
/// The cell arrives as a `Cell`, not a locked connection: an implementation
/// that makes a network call must do so BETWEEN `cell.with(...)` bursts, so
/// the person's cell stays usable while their turn is thinking.
pub trait CapabilityRouter: Send + Sync {
    fn execute(
        &self,
        cell: &Cell,
        capability: &str,
        args: &serde_json::Value,
        intent_id: &str,
    ) -> Result<Outcome, PrismError>;
}

pub struct TurnDeps<'a> {
    pub router: &'a dyn CapabilityRouter,
    pub verdicts: &'a dyn VerdictProvider,
    /// Test hook: called at every journal boundary with the point name;
    /// returning true simulates the process dying right there.
    pub crash: Option<&'a dyn Fn(&str) -> bool>,
}

/// Every boundary the kill-test must murder us at.
pub const CRASH_POINTS: [&str; 6] = [
    "after_open",
    "after_decision",
    "after_plan",
    "after_grant",
    "after_execute",
    "after_receipt",
];

#[derive(Debug)]
pub struct TurnOutput {
    pub intent_id: String,
    pub reply: String,
    pub receipt: Receipt,
    pub reply_effect_id: String,
}

/// The journaled decision: which path won, floor or verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    Floor { m: FloorMatch },
    Verdict { v: Verdict },
}

fn crash_check(deps: &TurnDeps, point: &str) -> Result<(), PrismError> {
    if let Some(hook) = deps.crash {
        if hook(point) {
            return Err(PrismError::SimulatedCrash(point.to_string()));
        }
    }
    Ok(())
}

/// Run one full governed turn.
pub fn run_turn(
    cell: &Cell,
    env: &Envelope,
    deps: &TurnDeps,
) -> Result<TurnOutput, PrismError> {
    let intent_id = ids::new_id("int");
    let opened = serde_json::json!({
        "surface": env.surface,
        "modality": env.modality,
        "principal_id": env.principal_id,
        "content": env.content,
        "content_hash": ids::sha256_hex(env.content.as_bytes()),
        "device_trust": env.device_trust,
        "source_msg_id": env.source_msg_id,
    });
    cell.with(|c| journal::intent_open(c, &intent_id, &opened.to_string()))?;
    crash_check(deps, "after_open")?;

    // decision: the deterministic floor runs first and wins unconditionally (Q17)
    let decision = match floor::scan(&env.content, Local::now()) {
        Some(m) => Decision::Floor { m },
        None => Decision::Verdict {
            v: deps.verdicts.verdict(&env.content),
        },
    };
    let decision_json = serde_json::to_string(&decision)?;
    cell.with(|c| journal::step(c, &intent_id, "decision", &decision_json, None))?;
    crash_check(deps, "after_decision")?;

    let plan = plan_from_decision(&intent_id, &decision, &env.content);
    let plan_json = serde_json::to_string(&plan)?;
    cell.with(|c| journal::step(c, &intent_id, "plan", &plan_json, None))?;
    crash_check(deps, "after_plan")?;

    finish_planned_intent(cell, &intent_id, &plan, deps, true)
}

/// Everything after the plan is journaled. Shared verbatim with replay so a
/// resumed intent takes exactly the code path a live one does.
pub(crate) fn finish_planned_intent(
    cell: &Cell,
    intent_id: &str,
    plan: &Plan,
    deps: &TurnDeps,
    live: bool,
) -> Result<TurnOutput, PrismError> {
    // execute steps, reusing journaled outcomes on replay (never re-run a
    // completed effect)
    let prior: Vec<Outcome> = cell
        .with(|c| journal::payloads_of(c, intent_id, "outcome"))?
        .iter()
        .filter_map(|p| serde_json::from_str(p).ok())
        .collect();
    let mut outcomes: Vec<Outcome> = Vec::new();
    for step in &plan.steps {
        if let Some(done) = prior.iter().find(|o| o.step_id == step.step_id) {
            outcomes.push(done.clone());
            continue;
        }
        // grants: scoped, time-boxed authority for anything that writes
        if step.effect != Effect::Read {
            let grant = Grant {
                grant_id: ids::new_id("grant"),
                capability: step.capability.clone(),
                scope: step.args.clone(),
                principal: 0,
                expires_at: ids::ts_ms() + 5 * 60_000,
                issued_by: "policy:auto".into(),
            };
            let grant_json = serde_json::to_string(&grant)?;
            cell.with(|c| journal::step(c, intent_id, "grant", &grant_json, None))?;
            crash_check(deps, "after_grant")?;
        }
        // the cell is NOT locked here: a capability may spend seconds in a
        // model call, and the person's other requests must not queue on it
        let outcome = execute_step(cell, deps.router, step, intent_id)?;
        let outcome_json = serde_json::to_string(&outcome)?;
        let outcome_hash = ids::sha256_hex(outcome.detail.as_bytes());
        cell.with(|c| {
            journal::step(c, intent_id, "outcome", &outcome_json, Some(&outcome_hash))
        })?;
        crash_check(deps, "after_execute")?;
        outcomes.push(outcome);
    }

    // verification is deterministic here: outcomes carry row-level evidence
    let all_ok = outcomes.iter().all(|o| o.ok);
    let verify_json = serde_json::json!({ "ok": all_ok }).to_string();
    cell.with(|c| journal::step(c, intent_id, "verify", &verify_json, None))?;

    // receipt: compiled from evidence, never narrated by a model
    let mut receipt = build_receipt(intent_id, &outcomes);

    // the deterministic expression check (sec 5 / Q26) runs before the
    // receipt is stored, so an unsupported effect claim is recorded as
    // uncertain rather than verified
    let draft_reply = compose_reply(&outcomes, &receipt);
    let unsupported = unsupported_effect_claim(&draft_reply, &outcomes);
    if unsupported {
        receipt.status = ReceiptStatus::Uncertain;
        receipt.claims.push(Claim {
            claim: "an utterance in this turn asserted an effect that no step \
                    performed; the assertion is unsupported and was flagged to \
                    the person"
                .into(),
            evidence: vec![Evidence {
                kind: "deterministic".into(),
                provider: "expression-check".into(),
                external_id: "unsupported-effect-claim".into(),
                hash: ids::sha256_hex(draft_reply.as_bytes()),
                ts: ids::ts_ms(),
            }],
        });
        let flag_json =
            serde_json::json!({ "reason": "unsupported effect claim" }).to_string();
        cell.with(|c| journal::step(c, intent_id, "expression.flagged", &flag_json, None))?;
    }
    let receipt = cell.with(|c| receipts::store(c, &receipt))?;
    let receipt_json = serde_json::json!({
        "receipt_id": receipt.receipt_id,
        "status": receipt.status.as_str()
    })
    .to_string();
    cell.with(|c| journal::step(c, intent_id, "receipt", &receipt_json, None))?;
    crash_check(deps, "after_receipt")?;

    // reply through the transactional outbox (Q11): enqueued before it can
    // possibly leave, deduped structurally
    let mut reply = compose_reply(&outcomes, &receipt);
    if unsupported {
        reply.push_str(UNSUPPORTED_NOTE);
    }
    let (reply_effect_id, _fresh) =
        cell.with(|c| outbox::enqueue(c, intent_id, "surface:chat", &reply))?;
    if !live {
        // resumed after a crash: the session that asked is gone; the honest
        // state is failed-delivery, the material effect stands
        cell.with(|c| {
            outbox::mark(
                c,
                &reply_effect_id,
                "failed",
                Some("crash before delivery; no live session"),
            )
        })?;
    }
    let enq_json = serde_json::json!({ "effect_id": reply_effect_id }).to_string();
    cell.with(|c| journal::step(c, intent_id, "reply.enqueue", &enq_json, None))?;
    cell.with(|c| journal::intent_close(c, intent_id, receipt.status.as_str()))?;

    Ok(TurnOutput {
        intent_id: intent_id.to_string(),
        reply,
        receipt,
        reply_effect_id,
    })
}

pub(crate) fn plan_from_decision(intent_id: &str, decision: &Decision, content: &str) -> Plan {
    let step = |capability: &str, args: serde_json::Value, effect: Effect| PlanStep {
        step_id: ids::new_id("pstep"),
        capability: capability.into(),
        args,
        effect,
        approval: Approval::Auto,
        deps: vec![],
    };
    let steps = match decision {
        Decision::Floor { m } => match m {
            FloorMatch::TimeNow => vec![step("answer.time", serde_json::json!({}), Effect::Read)],
            FloorMatch::SelfMeta => vec![step("answer.self", serde_json::json!({}), Effect::Read)],
            FloorMatch::Help => vec![step("answer.help", serde_json::json!({}), Effect::Read)],
            FloorMatch::Remind { fire_at_ms, about } => vec![step(
                "reminder.create",
                serde_json::json!({ "fire_at": fire_at_ms, "about": about }),
                Effect::ReversibleWrite,
            )],
            FloorMatch::ListReminders => {
                vec![step("reminder.list", serde_json::json!({}), Effect::Read)]
            }
            FloorMatch::CancelReminder => vec![step(
                "reminder.cancel_last",
                serde_json::json!({}),
                Effect::ReversibleWrite,
            )],
            FloorMatch::Remember { content } => vec![step(
                "memory.remember",
                serde_json::json!({ "content": content }),
                Effect::ReversibleWrite,
            )],
            FloorMatch::Recall { query } => vec![step(
                "memory.recall",
                serde_json::json!({ "query": query }),
                Effect::Read,
            )],
            FloorMatch::RegistryList => {
                vec![step("registry.list", serde_json::json!({}), Effect::Read)]
            }
            // deletion is real (owner's erase right) -- honestly irreversible
            FloorMatch::ForgetFact { index } => vec![step(
                "memory.forget",
                serde_json::json!({ "index": index }),
                Effect::Irreversible,
            )],
            FloorMatch::CorrectFact { index, content } => vec![step(
                "memory.correct",
                serde_json::json!({ "index": index, "content": content }),
                Effect::ReversibleWrite,
            )],
            FloorMatch::Invite => vec![step(
                "member.invite",
                serde_json::json!({}),
                Effect::ReversibleWrite,
            )],
            FloorMatch::TelegramCode => vec![step(
                "telegram.bind_code",
                serde_json::json!({}),
                Effect::ReversibleWrite,
            )],
            FloorMatch::WebSearch { query } => vec![step(
                "web.research",
                serde_json::json!({ "query": query }),
                Effect::Read,
            )],
        },
        Decision::Verdict { v } => {
            // the verdict routes; capabilities execute. chitchat with a
            // ready one-liner answers directly (Q16's reply field); search
            // or a web door goes through research; everything else is a
            // model answer with memory context.
            if v.action == VerdictAction::Chitchat && v.reply.is_some() {
                vec![step(
                    "answer.direct",
                    serde_json::json!({ "reply": v.reply.clone().unwrap_or_default() }),
                    Effect::Read,
                )]
            } else if v.action == VerdictAction::Search || v.door == Door::Web {
                vec![step(
                    "web.research",
                    serde_json::json!({ "query": content }),
                    Effect::Read,
                )]
            } else {
                vec![step(
                    "answer.model",
                    serde_json::json!({ "query": content, "tier": v.tier }),
                    Effect::Read,
                )]
            }
        }
    };
    Plan {
        plan_id: ids::new_id("plan"),
        intent_id: intent_id.into(),
        steps,
    }
}

const SELF_TEXT: &str = "i'm bender -- your robot, v0.2 MVP. i run on this \
machine only; your words live in an encrypted cell here, every crossing is \
in the boundary log, and everything i claim to have done carries a receipt. \
i can tell time, keep reminders, and remember facts you tell me -- every \
fact with its source, listed in my registry, correctable and deletable for \
real. my model brain arrives with M4.";

const HELP_TEXT: &str = "here's what works today:\n\
- time / date -- \"what time is it\"\n\
- reminders -- \"remind me in 10 minutes to stretch\", \"remind me at 18:30 \
to call mark\", \"remind me tomorrow at 9 to stretch\"\n\
- \"my reminders\" / \"cancel reminder\"\n\
- memory -- \"remember that i drink green tea\", \"what do you remember \
about tea\"\n\
- registry -- \"my facts\" (every fact + its source), \"forget fact 2\" \
(deletes for real), \"correct fact 1: ...\"\n\
- \"who are you\" -- about me\n\
everything else needs my model brain, which arrives with M4.";

const FALLBACK_TEXT: &str = "i can't reason about that yet -- my model brain \
arrives with M4. today i can tell the time, and set, list, or cancel \
reminders. try \"help\".";

fn execute_step(
    cell: &Cell,
    router: &dyn CapabilityRouter,
    step: &PlanStep,
    intent_id: &str,
) -> Result<Outcome, PrismError> {
    // floor answers are system-generated constants computed from local state:
    // they attest to exactly what they say
    let deterministic = |detail: String| {
        Outcome::attested(
            step.step_id.clone(),
            vec![Evidence {
                kind: "deterministic".into(),
                provider: "floor".into(),
                external_id: step.capability.clone(),
                hash: ids::sha256_hex(detail.as_bytes()),
                ts: ids::ts_ms(),
            }],
            detail,
        )
    };
    match step.capability.as_str() {
        "answer.time" => {
            let now = Local::now();
            Ok(deterministic(format!(
                "it's {}",
                now.format("%H:%M on %A, %d %B %Y")
            )))
        }
        "answer.self" => Ok(deterministic(SELF_TEXT.into())),
        "answer.help" => Ok(deterministic(HELP_TEXT.into())),
        "answer.fallback" => Ok(deterministic(FALLBACK_TEXT.into())),
        // the verdict's own chitchat one-liner (Q16 reply field) -- model
        // text, so it speaks but attests to nothing
        "answer.direct" => Ok(Outcome::utterance(
            step.step_id.clone(),
            vec![Evidence {
                kind: "provider_response".into(),
                provider: "verdict".into(),
                external_id: "verdict.reply".into(),
                hash: ids::sha256_hex(
                    step.args["reply"].as_str().unwrap_or("").as_bytes(),
                ),
                ts: ids::ts_ms(),
            }],
            step.args["reply"].as_str().unwrap_or("hi.").to_string(),
        )),
        _ => {
            let mut outcome = router.execute(cell, &step.capability, &step.args, intent_id)?;
            outcome.step_id = step.step_id.clone();
            Ok(outcome)
        }
    }
}

/// Compile a receipt from outcomes -- also used by system intents (e.g. the
/// reminder scheduler) so their fires carry receipts like any other action.
pub fn build_receipt(intent_id: &str, outcomes: &[Outcome]) -> Receipt {
    let all_ok = !outcomes.is_empty() && outcomes.iter().all(|o| o.ok);
    let any_ok = outcomes.iter().any(|o| o.ok);
    let status = if all_ok {
        ReceiptStatus::Verified
    } else if any_ok {
        ReceiptStatus::Partial
    } else {
        ReceiptStatus::Failed
    };
    // receipts name the models that acted (arch sec 0a): collected from
    // provider_response evidence, never from narration
    let mut models_used: Vec<String> = outcomes
        .iter()
        .flat_map(|o| o.evidence.iter())
        .filter(|e| e.kind == "provider_response")
        .map(|e| e.external_id.clone())
        .collect();
    models_used.sort();
    models_used.dedup();
    Receipt {
        receipt_id: ids::new_id("rcpt"),
        intent_id: intent_id.into(),
        status,
        claims: outcomes
            .iter()
            .map(|o| Claim {
                // model prose is never promoted to a claim: an utterance
                // asserts only that a model spoke, and the evidence says
                // which one
                claim: match &o.claim {
                    Some(c) => c.clone(),
                    None => format!(
                        "produced an utterance of {} characters; asserts no external effect",
                        o.detail.chars().count()
                    ),
                },
                evidence: o.evidence.clone(),
            })
            .collect(),
        models_used,
        data_disclosures: vec![],
    }
}

/// Phrases in which the Robot asserts it changed something in the world.
/// Deliberately narrow: only first-person completions, not descriptions.
const EFFECT_CLAIM_PATTERNS: [&str; 26] = [
    "i've set",
    "i have set",
    "i've created",
    "i have created",
    "i've added",
    "i have added",
    "i've saved",
    "i have saved",
    "i've stored",
    "i have stored",
    "i've deleted",
    "i have deleted",
    "i've removed",
    "i have removed",
    "i've cancelled",
    "i've canceled",
    "i've scheduled",
    "i have scheduled",
    "i've remembered",
    "i have remembered",
    "i've sent",
    "i have sent",
    "reminder is set",
    "я поставил",
    "я запомнил",
    "я удалил",
];

/// The deterministic claim-vs-receipt check (arch sec 5 / Q26: "string/set
/// logic, ~0 ms", run on every turn, never on the model that generated).
///
/// If an utterance asserts the Robot performed an effect, but the turn
/// executed no effect-producing step, the assertion is unsupported. Rather
/// than let it stand, we say so -- and the receipt goes `uncertain`, because
/// what was said cannot be backed by evidence.
pub fn unsupported_effect_claim(reply: &str, outcomes: &[Outcome]) -> bool {
    if outcomes.iter().any(|o| o.is_effect()) {
        return false; // something really happened; the claim may be true
    }
    let lower = reply.to_lowercase();
    EFFECT_CLAIM_PATTERNS.iter().any(|p| lower.contains(p))
}

const UNSUPPORTED_NOTE: &str = "\n\n[note from my own checks: the sentence above \
sounds like i changed something, but this turn executed no action -- nothing was \
created, saved, or deleted. the receipt for this turn records no effect.]";

/// The reply is rendered from the receipt's claims -- system evidence, not
/// model narration. English for now; Soul's user-language rendering is a
/// later milestone.
pub(crate) fn compose_reply(outcomes: &[Outcome], receipt: &Receipt) -> String {
    match receipt.status {
        ReceiptStatus::Verified | ReceiptStatus::Partial => {
            let mut parts: Vec<&str> = outcomes
                .iter()
                .filter(|o| o.ok && !o.detail.is_empty())
                .map(|o| o.detail.as_str())
                .collect();
            if parts.is_empty() {
                parts.push("done.");
            }
            let mut reply = parts.join("\n");
            if receipt.status == ReceiptStatus::Partial {
                reply.push_str("\n(some of that failed -- the receipt has the honest detail.)");
            }
            reply
        }
        _ => {
            // honest failure: say what actually went wrong, not a generic line
            let details: Vec<&str> = outcomes
                .iter()
                .filter(|o| !o.ok && !o.detail.is_empty())
                .map(|o| o.detail.as_str())
                .collect();
            if details.is_empty() {
                "that didn't work -- nothing was changed. the receipt records the \
                 failure honestly."
                    .to_string()
            } else {
                format!(
                    "{}\n(nothing was changed; the receipt records this as {}.)",
                    details.join("\n"),
                    receipt.status.as_str()
                )
            }
        }
    }
}

/// An honest failed receipt with a single deterministic claim. Used by
/// crash recovery and the zombie sweeper (Q12) -- a closed intent always
/// says truthfully why it closed.
pub fn failed_receipt(intent_id: &str, claim: &str, provider: &str) -> Receipt {
    Receipt {
        receipt_id: ids::new_id("rcpt"),
        intent_id: intent_id.into(),
        status: ReceiptStatus::Failed,
        claims: vec![Claim {
            claim: claim.into(),
            evidence: vec![Evidence {
                kind: "deterministic".into(),
                provider: provider.into(),
                external_id: "honest-failure".into(),
                hash: String::new(),
                ts: ids::ts_ms(),
            }],
        }],
        models_used: vec![],
        data_disclosures: vec![],
    }
}

/// A failed receipt for an intent interrupted before any decision was
/// journaled: no effect ran, and the honest answer is "resend".
pub(crate) fn interrupted_receipt(intent_id: &str) -> Receipt {
    failed_receipt(
        intent_id,
        "interrupted before any decision; no effect was executed; message preserved",
        "replay",
    )
}

/// Format a fire-time for human display (local time).
pub fn format_fire_at(fire_at_ms: i64) -> String {
    match Local.timestamp_millis_opt(fire_at_ms).earliest() {
        Some(dt) => dt.format("%H:%M on %a, %d %b").to_string(),
        None => format!("t+{fire_at_ms}ms"),
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;

    fn ev(kind: &str) -> Vec<Evidence> {
        vec![Evidence {
            kind: kind.into(),
            provider: "test".into(),
            external_id: "x".into(),
            hash: String::new(),
            ts: 0,
        }]
    }

    /// The receipts law, structurally: model prose must never appear as a
    /// receipt claim. Before this split, a model replying "I've set that
    /// reminder" produced a Verified receipt asserting exactly that, with
    /// "a model spoke" as its only evidence.
    #[test]
    fn model_prose_never_becomes_a_receipt_claim() {
        let lie = "I've set that reminder for tomorrow at 9!".to_string();
        let outcome = Outcome::utterance("s1".into(), ev("provider_response"), lie.clone());
        let receipt = build_receipt("int_1", &[outcome]);

        assert_eq!(receipt.claims.len(), 1);
        assert!(
            !receipt.claims[0].claim.contains("I've set"),
            "model narration leaked into the receipt: {}",
            receipt.claims[0].claim
        );
        assert!(receipt.claims[0].claim.contains("asserts no external effect"));
    }

    /// A capability that really acted DOES attest, and its text is the claim.
    #[test]
    fn attested_effects_are_claimed_verbatim() {
        let outcome = Outcome::attested(
            "s1".into(),
            ev("row"),
            "done -- i'll remind you at 09:00: call mark".to_string(),
        );
        let receipt = build_receipt("int_2", &[outcome]);
        assert_eq!(receipt.status, ReceiptStatus::Verified);
        assert!(receipt.claims[0].claim.contains("call mark"));
    }

    /// A failed provider call must not produce a Verified receipt.
    #[test]
    fn failures_are_not_verified() {
        let outcome = Outcome::failed(
            "s1".into(),
            ev("deterministic"),
            "i'm having trouble thinking right now".to_string(),
        );
        let receipt = build_receipt("int_3", std::slice::from_ref(&outcome));
        assert_eq!(receipt.status, ReceiptStatus::Failed);
        // and the reply says what went wrong, not a generic line
        let reply = compose_reply(&[outcome], &receipt);
        assert!(reply.contains("trouble thinking"), "{reply}");
        assert!(reply.contains("nothing was changed"), "{reply}");
    }

    /// The deterministic claim-vs-receipt check (sec 5 / Q26): an utterance
    /// asserting an effect on a turn that executed none is flagged.
    #[test]
    fn unsupported_effect_claims_are_detected() {
        let spoke = Outcome::utterance(
            "s1".into(),
            ev("provider_response"),
            "Sure! I've saved that to your memory.".into(),
        );
        assert!(unsupported_effect_claim(
            "Sure! I've saved that to your memory.",
            std::slice::from_ref(&spoke)
        ));

        // ...but not when a capability actually did the work
        let did = Outcome::attested("s2".into(), ev("row"), "remembered: x".into());
        assert!(!unsupported_effect_claim(
            "I've saved that to your memory.",
            &[did]
        ));

        // ordinary answers are not flagged
        assert!(!unsupported_effect_claim(
            "Rust 1.97.1 is the latest stable release.",
            &[spoke]
        ));
    }
}
