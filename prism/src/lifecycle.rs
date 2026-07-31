//! The governed lifecycle (arch sec 3):
//! Intent -> decision (floor | verdict) -> Plan -> Grant -> Execution ->
//! Verification -> Receipt -> Response. Every stage journaled; a sentence
//! claiming an effect may only be produced from a verified state transition.

use crate::floor::{self, FloorMatch};
use crate::types::*;
use crate::verdict::VerdictProvider;
use crate::{approval, journal, outbox, pending, receipts, Cell, Envelope, PrismError};
use chrono::Local;
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
        lang: &str,
    ) -> Result<Outcome, PrismError>;

    /// The tool catalog for THIS turn. Takes the cell because part of the
    /// catalog is situational: the tool that answers a question exists only
    /// while a question is open.
    fn describe(&self, cell: &Cell) -> Vec<ToolDef>;

    /// Does this capability need a person's yes? Consulted at plan time so
    /// the requirement lives with the capability rather than being
    /// rediscovered at execution.
    fn approval_for(&self, capability: &str) -> Approval {
        let _ = capability;
        Approval::Auto
    }

    /// Does this proposed call name a real tool with arguments that
    /// typecheck? Returns the registry's own effect class, so a caller
    /// cannot assert a gentler one than the code actually performs.
    ///
    /// This is the whole safety story for model-proposed work: the model is
    /// an input device, never an authority.
    fn validate(&self, tool: &str, args: &serde_json::Value) -> Result<Effect, String>;
}

/// The tool a model calls to answer a question we asked. Offered only
/// while something is actually parked, so it cannot be used to conjure a
/// confirmation out of nothing.
pub const CONFIRM_TOOL: &str = "confirmation.respond";
/// Kernel-internal steps: asking, and being told no.
pub const CONFIRM_REQUEST: &str = "confirmation.request";
pub const CONFIRM_DECLINED: &str = "confirmation.declined";
/// A yes that arrived too late, or twice, or for a call the registry no
/// longer accepts. Nothing ran, and saying so is the only honest option --
/// a "yes" that quietly turns into small talk leaves the person believing
/// something happened.
pub const CONFIRM_STALE: &str = "confirmation.stale";

/// Close a parked intent the person declined. Nothing ran; the receipt
/// says exactly that, because a decline that closed silently would look
/// identical to a drop.
pub(crate) fn close_declined(
    cell: &Cell,
    intent_id: &str,
    plan: &Plan,
    deps: &TurnDeps,
    parked: &approval::Parked,
) -> Result<TurnOutput, PrismError> {
    let receipt = failed_receipt(
        intent_id,
        &format!("{} was declined by the owner; nothing ran", parked.capability),
        "approval",
    );
    let receipt = cell.with(|c| receipts::store(c, &receipt))?;
    let parts = vec![ReplyPart::Say(Rendering::new(
        "approval_declined",
        serde_json::json!({ "capability": parked.capability }),
    ))];
    let reply = deps.renderer.render(&plan.lang, &parts, &[]).text;
    let (reply_effect_id, _) =
        cell.with(|c| outbox::enqueue(c, intent_id, "surface:chat", &reply))?;
    cell.with(|c| outbox::mark(c, &reply_effect_id, "sent", None))?;
    cell.with(|c| journal::intent_close(c, intent_id, receipt.status.as_str()))?;
    Ok(TurnOutput {
        intent_id: intent_id.to_string(),
        reply,
        lang: plan.lang.clone(),
        receipt,
        reply_effect_id,
    })
}

/// A receipt for a turn that is waiting rather than finished.
///
/// `Proposed` and deliberately NOT terminal: the intent is still open, and
/// a terminal receipt would assert an outcome that has not happened. The
/// watchdog's rule -- an intent without a terminal receipt is an alarm --
/// is relaxed for exactly this state, which is why it has its own name in
/// the journal rather than looking like a stall.
fn awaiting_receipt(intent_id: &str, capability: &str) -> Receipt {
    Receipt {
        receipt_id: ids::new_id("rcpt"),
        intent_id: intent_id.into(),
        status: ReceiptStatus::Proposed,
        claims: vec![Claim {
            claim: format!("{capability} is waiting for the owner's approval; nothing has run"),
            evidence: vec![Evidence {
                kind: "deterministic".into(),
                provider: "approval".into(),
                external_id: capability.into(),
                hash: String::new(),
                ts: ids::ts_ms(),
            }],
        }],
        models_used: vec![],
        data_disclosures: vec![],
    }
}

/// How long a step's authority lives. Long enough that an ordinary crash
/// and restart resumes inside it; short enough that a turn resumed days
/// later has to be re-authorised rather than executing on stale consent.
pub const GRANT_TTL_MS: i64 = 15 * 60_000;

/// Who this turn is acting for. Read from the journaled intent rather than
/// assumed -- a grant that records principal 0 for everyone records
/// nothing.
fn acting_principal(cell: &Cell, intent_id: &str) -> i64 {
    cell.with(|c| journal::payload_of(c, intent_id, "intent_open"))
        .ok()
        .flatten()
        .and_then(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
        .and_then(|v| v["principal_id"].as_i64())
        .unwrap_or(-1)
}

/// Milliseconds since the epoch as RFC 3339 local time -- the one wire
/// representation for a moment, whether the floor computed it or a model
/// proposed it.
pub fn rfc3339(ms: i64) -> String {
    match chrono::TimeZone::timestamp_millis_opt(&Local, ms).earliest() {
        Some(dt) => dt.to_rfc3339(),
        None => String::new(),
    }
}

/// Turns structure into sentences. Lives at the surface, never here: the
/// kernel emits `ReplyPart`s and has no opinion about words.
pub trait Renderer: Send + Sync {
    fn render(&self, lang: &str, parts: &[ReplyPart], actions: &[ActionRecord]) -> Rendered;
}

/// A rendered reply, and an honest account of what it cost in privacy.
///
/// English costs nothing: the templates are local, and no part of the
/// person's cell leaves the machine to produce a sentence. Every other
/// language is rendered by a model, which means the SLOTS go out -- and
/// slots are their facts, their reminders, the sources of their registry.
/// That is a disclosure, and a system that logs every byte crossing the
/// boundary should not be quiet about the one it causes itself.
#[derive(Debug, Default)]
pub struct Rendered {
    pub text: String,
    /// Rendering ids whose data was sent to a model to be put into words.
    /// Empty for English and for anything rendered locally.
    pub disclosed: Vec<String>,
}

pub struct TurnDeps<'a> {
    pub router: &'a dyn CapabilityRouter,
    pub verdicts: &'a dyn VerdictProvider,
    pub renderer: &'a dyn Renderer,
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
    /// The language this turn was answered in. The caller persists it per
    /// cell so background lanes -- a reminder firing at 03:00, a backup
    /// failure -- address the person in their own language instead of
    /// defaulting to English.
    pub lang: String,
    pub receipt: Receipt,
    pub reply_effect_id: String,
}

/// The journaled decision: which path won, floor or verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    Floor {
        m: FloorMatch,
        #[serde(default = "crate::types::default_lang")]
        lang: String,
    },
    Verdict {
        v: Verdict,
        /// Present when a model proposed a tool AND the registry accepted
        /// it. A rejected proposal never reaches here -- it is journaled as
        /// `call.rejected` and the turn continues without it.
        #[serde(default)]
        call: Option<ValidatedCall>,
    },
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
        // the floor is English, so a floor match is an English turn
        Some(m) => Decision::Floor {
            m,
            lang: "en".into(),
        },
        None => {
            // one call: it classifies the turn AND, if a tool fits, proposes
            // it. The catalog comes from the registry, so it cannot name a
            // tool that does not exist.
            //
            let tools = deps.router.describe(cell);
            let now = Local::now().to_rfc3339();
            let routed = deps.verdicts.route(&env.content, &tools, &now);
            let call = validate_proposal(cell, &intent_id, deps, routed.call)?;
            Decision::Verdict {
                v: routed.verdict,
                call,
            }
        }
    };
    let decision_json = serde_json::to_string(&decision)?;
    cell.with(|c| journal::step(c, &intent_id, "decision", &decision_json, None))?;
    crash_check(deps, "after_decision")?;

    let mut plan = plan_from_decision(&intent_id, &decision, &env.content);
    apply_approval_policy(&mut plan, deps.router);
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
    finish_planned_intent_with(cell, intent_id, plan, deps, live, &[])
}

/// As above, with the set of step ids a person has already approved.
pub(crate) fn finish_planned_intent_with(
    cell: &Cell,
    intent_id: &str,
    plan: &Plan,
    deps: &TurnDeps,
    live: bool,
    approved: &[String],
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
        // sec 3b.2: a step needing a person parks the intent in the journal
        // and stops here. Not a queue, not a timer -- an open intent whose
        // last state is `awaiting_approval`, which survives a crash for
        // free because replay already understands open intents.
        if step.approval == Approval::Required && !approved.contains(&step.step_id) {
            let parked = approval::park(cell, intent_id, step)?;
            let parts = vec![ReplyPart::Say(Rendering::new(
                "approval_needed",
                serde_json::json!({
                    "capability": parked.capability,
                    "args": parked.args,
                }),
            ))];
            let reply = deps.renderer.render(&plan.lang, &parts, &[]).text;
            let (reply_effect_id, _) =
                cell.with(|c| outbox::enqueue(c, intent_id, "surface:chat", &reply))?;
            // the intent stays OPEN: it is waiting, not finished, and a
            // receipt now would be a receipt for something that has not
            // happened
            return Ok(TurnOutput {
                intent_id: intent_id.to_string(),
                reply,
                lang: plan.lang.clone(),
                receipt: awaiting_receipt(intent_id, &parked.capability),
                reply_effect_id,
            });
        }

        // grants: scoped, time-boxed authority for anything that writes
        if step.effect != Effect::Read {
            let grant = Grant {
                grant_id: ids::new_id("grant"),
                capability: step.capability.clone(),
                scope: step.args.clone(),
                principal: acting_principal(cell, intent_id),
                expires_at: ids::ts_ms() + GRANT_TTL_MS,
                issued_by: "policy:auto".into(),
            };
            let grant_json = serde_json::to_string(&grant)?;
            cell.with(|c| journal::step(c, intent_id, "grant", &grant_json, None))?;
            crash_check(deps, "after_grant")?;

            // ...and then READ it. A grant that is minted, journaled and
            // never checked is an authority model in costume. The check is
            // here rather than at mint time because the interesting cases
            // are the ones where time passes in between: a crash resumed
            // hours later, a step that waited for an approval.
            if let Err(denial) = grant.authorises(&step.capability, &step.args, ids::ts_ms()) {
                let refused = serde_json::json!({
                    "grant_id": grant.grant_id,
                    "capability": step.capability,
                    "reason": denial.to_string(),
                })
                .to_string();
                cell.with(|c| journal::step(c, intent_id, "grant.refused", &refused, None))?;
                outcomes.push(Outcome::failed(
                    step.step_id.clone(),
                    vec![Evidence {
                        kind: "deterministic".into(),
                        provider: "grant-check".into(),
                        external_id: grant.grant_id.clone(),
                        hash: String::new(),
                        ts: ids::ts_ms(),
                    }],
                    format!("refused: {denial}"),
                    Rendering::new(
                        "grant_refused",
                        serde_json::json!({ "why": denial.to_string() }),
                    ),
                ));
                continue;
            }
        }
        // the cell is NOT locked here: a capability may spend seconds in a
        // model call, and the person's other requests must not queue on it
        let outcome = execute_step(cell, deps.router, step, intent_id, &plan.lang)?;
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

    // sec 5 / Q26, before the receipt is stored: a reply that asserts a
    // change no step performed is recorded as uncertain rather than
    // verified, and the person is told
    let unsupported = unsupported_effect_claim(&outcomes, plan);
    if let Some(why) = &unsupported {
        receipt.status = ReceiptStatus::Uncertain;
        receipt.claims.push(Claim {
            claim: format!(
                "this turn's reply asserted a change that no step performed: {why}"
            ),
            evidence: vec![Evidence {
                kind: "deterministic".into(),
                provider: "expression-check".into(),
                external_id: "unsupported-effect-claim".into(),
                hash: String::new(),
                ts: ids::ts_ms(),
            }],
        });
        let flag = serde_json::json!({ "reason": why }).to_string();
        cell.with(|c| journal::step(c, intent_id, "expression.flagged", &flag, None))?;
    }

    let receipt = cell.with(|c| receipts::store(c, &receipt))?;
    let receipt_json = serde_json::json!({
        "receipt_id": receipt.receipt_id,
        "status": receipt.status.as_str()
    })
    .to_string();
    cell.with(|c| journal::step(c, intent_id, "receipt", &receipt_json, None))?;
    crash_check(deps, "after_receipt")?;

    // the ONE place structure becomes words, at the very edge, outside the
    // kernel: everything above this line is data
    let mut parts = reply_parts(&outcomes, &receipt);
    if unsupported.is_some() {
        parts.push(ReplyPart::Say(Rendering::bare("unsupported_note")));
    }
    let actions = action_records(&receipt, &outcomes, plan);
    let rendered = deps.renderer.render(&plan.lang, &parts, &actions);
    let reply = rendered.text;

    // saying it in their language cost a trip to a model, carrying their
    // own data as the material. Journal it: the boundary log records the
    // bytes, this records WHY they went.
    if !rendered.disclosed.is_empty() {
        let disclosed = serde_json::json!({
            "to": "model:render",
            "lang": plan.lang,
            "renderings": rendered.disclosed,
        })
        .to_string();
        cell.with(|c| journal::step(c, intent_id, "render.disclosed", &disclosed, None))?;
    }

    // reply through the transactional outbox (Q11): enqueued before it can
    // possibly leave, deduped structurally
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
        lang: plan.lang.clone(),
        receipt,
        reply_effect_id,
    })
}

/// Put a model's proposal through the registry before it can become a plan.
///
/// Every outcome is journaled, including refusal: an audit of this turn
/// shows what was proposed and why it was or was not allowed. The model is
/// an input device, never an authority.
fn validate_proposal(
    cell: &Cell,
    intent_id: &str,
    deps: &TurnDeps,
    proposed: Option<ToolCall>,
) -> Result<Option<ValidatedCall>, PrismError> {
    let Some(c) = proposed else { return Ok(None) };

    // an answer to a question we asked: release the parked call, or drop it
    if c.tool == CONFIRM_TOOL {
        let said_yes = c.args["confirmed"].as_bool().unwrap_or(false);
        let Some(p) = cell.with(pending::open)? else {
            // Nothing is open. Either a question was just answered and this
            // is a second, late "yes" -- which must be acknowledged -- or
            // the model mis-fired and this is ordinary talk.
            let late = cell.with(|conn| pending::recently_resolved(conn, pending::TTL_MS))?;
            return Ok(late.then(|| ValidatedCall {
                tool: CONFIRM_STALE.into(),
                args: serde_json::json!({ "tool": "" }),
                effect: Effect::Read,
            }));
        };
        if !said_yes {
            cell.with(|conn| pending::resolve(conn, &p.id, "declined"))?;
            return Ok(Some(ValidatedCall {
                tool: CONFIRM_DECLINED.into(),
                args: serde_json::json!({ "tool": p.tool }),
                effect: Effect::Read,
            }));
        }
        // spend the confirmation BEFORE planning: a replayed turn must not
        // find it still open and delete a second time
        if !cell.with(|conn| pending::resolve(conn, &p.id, "confirmed"))? {
            // someone else already spent it -- a double submit, or a
            // resumed turn. Say so; a "yes" that quietly becomes small talk
            // leaves the person believing something happened.
            return Ok(Some(ValidatedCall {
                tool: CONFIRM_STALE.into(),
                args: serde_json::json!({ "tool": p.tool }),
                effect: Effect::Read,
            }));
        }
        // re-validate: the registry is the authority, not the parked row
        let effect = match deps.router.validate(&p.tool, &p.args) {
            Ok(e) => e,
            Err(why) => {
                let rejected =
                    serde_json::json!({ "tool": p.tool, "reason": why }).to_string();
                cell.with(|conn| journal::step(conn, intent_id, "call.rejected", &rejected, None))?;
                // the confirmation is spent and the call cannot run. Telling
                // them is the only honest option.
                return Ok(Some(ValidatedCall {
                    tool: CONFIRM_STALE.into(),
                    args: serde_json::json!({ "tool": p.tool }),
                    effect: Effect::Read,
                }));
            }
        };
        let ok = serde_json::json!({ "tool": p.tool, "confirmed_by": "person" }).to_string();
        cell.with(|conn| journal::step(conn, intent_id, "call.confirmed", &ok, None))?;
        return Ok(Some(ValidatedCall {
            tool: p.tool,
            args: p.args,
            effect,
        }));
    }
    match deps.router.validate(&c.tool, &c.args) {
        Ok(effect) => {
            let ok = serde_json::json!({ "tool": c.tool, "effect": effect }).to_string();
            cell.with(|conn| journal::step(conn, intent_id, "call.accepted", &ok, None))?;
            // sec 6b: an inference does not get to destroy anything. Park
            // it, ask, and let the answer release it.
            if effect == Effect::Irreversible {
                let parked =
                    cell.with(|conn| pending::park(conn, intent_id, &c.tool, &c.args))?;
                let note =
                    serde_json::json!({ "tool": c.tool, "pending_id": parked.id }).to_string();
                cell.with(|conn| journal::step(conn, intent_id, "call.parked", &note, None))?;
                return Ok(Some(ValidatedCall {
                    tool: CONFIRM_REQUEST.into(),
                    args: serde_json::json!({ "tool": c.tool, "args": c.args }),
                    effect: Effect::Read,
                }));
            }
            Ok(Some(ValidatedCall {
                tool: c.tool,
                args: c.args,
                effect,
            }))
        }
        Err(why) => {
            let rejected = serde_json::json!({ "tool": c.tool, "reason": why }).to_string();
            cell.with(|conn| journal::step(conn, intent_id, "call.rejected", &rejected, None))?;
            Ok(None)
        }
    }
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
    // approval is a property of the capability, applied to every step
    // uniformly -- a requirement that depended on which branch built the
    // plan would be a requirement with holes in it.
    let steps = match decision {
        Decision::Floor { m, .. } => match m {
            FloorMatch::TimeNow => vec![step("time.now", serde_json::json!({}), Effect::Read)],
            FloorMatch::SelfMeta => vec![step("robot.about", serde_json::json!({}), Effect::Read)],
            FloorMatch::Help => vec![step("robot.help", serde_json::json!({}), Effect::Read)],
            FloorMatch::Remind { fire_at_ms, about } => vec![step(
                "reminder.create",
                serde_json::json!({
                    "fire_at": rfc3339(*fire_at_ms),
                    "about": about,
                }),
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
            FloorMatch::SoulShow => {
                vec![step("soul.show", serde_json::json!({}), Effect::Read)]
            }
            FloorMatch::WebSearch { query } => vec![step(
                "web.research",
                serde_json::json!({ "query": query }),
                Effect::Read,
            )],
        },
        Decision::Verdict {
            call: Some(c), ..
        } => vec![step(&c.tool, c.args.clone(), c.effect)],
        Decision::Verdict { v, call: None } => {
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
    // one language for the whole turn, decided once: the floor reports the
    // pack that matched, the verdict reports what it detected. A language
    // with no pack resolves to English for the deterministic strings, while
    // the model still answers in the person's own language.
    let lang = match decision {
        Decision::Floor { lang, .. } => lang.clone(),
        Decision::Verdict { v, .. } => v.lang.clone(),
    };
    let lang = if lang.trim().is_empty() {
        "en".to_string()
    } else {
        lang
    };
    Plan {
        plan_id: ids::new_id("plan"),
        intent_id: intent_id.into(),
        lang,
        steps,
    }
}

/// Stamp each step with what its capability requires. Applied after the
/// plan is built so no branch can forget it.
pub(crate) fn apply_approval_policy(plan: &mut Plan, router: &dyn CapabilityRouter) {
    for step in &mut plan.steps {
        step.approval = router.approval_for(&step.capability);
    }
}

fn execute_step(
    cell: &Cell,
    router: &dyn CapabilityRouter,
    step: &PlanStep,
    intent_id: &str,
    lang: &str,
) -> Result<Outcome, PrismError> {
    // floor answers are system-generated constants computed from local state:
    // they attest to exactly what they say
    let internal = |claim: &str, say: Rendering| {
        Outcome::attested(
            step.step_id.clone(),
            vec![Evidence {
                kind: "deterministic".into(),
                provider: "kernel".into(),
                external_id: step.capability.clone(),
                hash: String::new(),
                ts: ids::ts_ms(),
            }],
            claim.into(),
            say,
        )
    };
    match step.capability.as_str() {
        // the question, and the answer "no" -- both are reads, so neither
        // produces an action record: nothing happened, and nothing is shown
        CONFIRM_REQUEST => Ok(internal(
            "asked the person to confirm an irreversible action",
            Rendering::new(
                "confirm_irreversible",
                serde_json::json!({ "tool": step.args["tool"] }),
            ),
        )),
        CONFIRM_DECLINED => Ok(internal(
            "the person declined; nothing was done",
            Rendering::new(
                "confirmation_declined",
                serde_json::json!({ "tool": step.args["tool"] }),
            ),
        )),
        CONFIRM_STALE => Ok(internal(
            "a confirmation arrived that could no longer be used; nothing was done",
            Rendering::new(
                "confirmation_stale",
                serde_json::json!({ "tool": step.args["tool"] }),
            ),
        )),
        "answer.fallback" => Ok(Outcome::attested(
            step.step_id.clone(),
            vec![Evidence {
                kind: "deterministic".into(),
                provider: "floor".into(),
                external_id: step.capability.clone(),
                hash: String::new(),
                ts: ids::ts_ms(),
            }],
            "declined: nothing in this robot does what was asked".into(),
            Rendering::bare("fallback"),
        )),
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
            let mut outcome =
                router.execute(cell, &step.capability, &step.args, intent_id, lang)?;
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

/// The reply as STRUCTURE: what each step contributed, in order. No words
/// are chosen here -- that is the renderer's job, at the surface.
pub(crate) fn reply_parts(outcomes: &[Outcome], receipt: &Receipt) -> Vec<ReplyPart> {
    let mut parts: Vec<ReplyPart> = outcomes
        .iter()
        .filter(|o| o.ok || o.rendering.is_some())
        .filter(|o| !o.detail.is_empty() || o.rendering.is_some())
        .map(|o| o.reply_part())
        .collect();
    if parts.is_empty() {
        parts.push(ReplyPart::Say(Rendering::bare("done")));
    }
    match receipt.status {
        ReceiptStatus::Partial => parts.push(ReplyPart::Say(Rendering::bare("partial_note"))),
        ReceiptStatus::Failed | ReceiptStatus::Uncertain => {
            parts.push(ReplyPart::Say(Rendering::new(
                "failed_note",
                serde_json::json!({ "status": receipt.status.as_str() }),
            )))
        }
        _ => {}
    }
    parts
}

/// Rendering ids that assert a change to the world.
///
/// These are kernel vocabulary, not language: `forgotten` is an id, and the
/// sentence a person reads for it is the surface's business. Listing them
/// is not the phrase-list problem returning -- a phrase list had to be
/// written once per language and could be evaded by writing in a language
/// nobody listed. This list is written once, full stop.
const EFFECT_CLAIMS: [&str; 8] = [
    "reminder_created",
    "reminder_cancelled",
    "remembered",
    "forgotten",
    "corrected",
    "invite_created",
    "telegram_bind_code",
    "media_stored",
];

/// The deterministic claim-vs-receipt check (arch sec 5 / Q26: "string/set
/// logic, ~0 ms", every turn, never on the model that generated).
///
/// The invariant: **a reply may not assert a change that no step performed.**
/// A rendering from `EFFECT_CLAIMS` must have come from a step the plan
/// classified as more than a read, and that step must have succeeded with
/// evidence behind it.
///
/// This replaced a scan for phrases like "i saved it", which had to be
/// written per language and so was blind in every language nobody had
/// listed. Checking the STRUCTURE instead is language-free by construction:
/// there is nothing to translate and nothing to evade.
///
/// What it catches is the failure that gets likelier as capabilities are
/// added -- a read-only step emitting a rendering that announces a change.
/// What it does not catch is a model writing an effect claim in free prose;
/// that is what the action record is for, and the two are complementary
/// rather than alternatives.
pub fn unsupported_effect_claim(
    outcomes: &[Outcome],
    plan: &Plan,
) -> Option<String> {
    for o in outcomes {
        let Some(r) = &o.rendering else { continue };
        if !EFFECT_CLAIMS.contains(&r.id.as_str()) {
            continue;
        }
        let step = plan.steps.iter().find(|s| s.step_id == o.step_id);
        match step {
            Some(s) if s.effect == Effect::Read => {
                return Some(format!(
                    "step {} announced '{}' from a read-only capability",
                    s.capability, r.id
                ))
            }
            Some(_) if !o.ok || o.evidence.is_empty() => {
                return Some(format!(
                    "'{}' was announced with no evidence behind it",
                    r.id
                ))
            }
            None => {
                return Some(format!(
                    "'{}' was announced by a step that is not in the plan",
                    r.id
                ))
            }
            _ => {}
        }
    }
    None
}

/// The action record: what actually happened, compiled from the receipt's
/// evidence rather than from anyone's sentence (arch sec 5 / Q26).
///
/// This is what makes the receipts law hold when a MODEL writes the reply.
/// The old defence was a list of phrases like "i saved it", per language --
/// which meant an unlisted language was an unchecked language. Now the
/// truth is simply displayed beside the claim, in every language at once,
/// because it is not language at all.
pub fn action_records(receipt: &Receipt, outcomes: &[Outcome], plan: &Plan) -> Vec<ActionRecord> {
    outcomes
        .iter()
        .filter_map(|o| {
            // a decline is evidence that nothing happened, so it earns no
            // line in a record of what did
            if o.evidence.iter().all(|e| e.kind == "declined") && !o.evidence.is_empty() {
                return None;
            }
            let st = plan.steps.iter().find(|s| s.step_id == o.step_id)?;
            // reads changed nothing, so there is nothing to vouch for; a
            // failure is worth showing whatever it was going to do
            (st.effect != Effect::Read || !o.ok).then(|| ActionRecord {
                tool: st.capability.clone(),
                status: if o.ok {
                    receipt.status.as_str().into()
                } else {
                    "failed".into()
                },
                detail: o.claim.clone().unwrap_or_default(),
            })
        })
        .collect()
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

    /// A capability that really acted DOES attest, and the receipt asserts
    /// its ENGLISH audit sentence -- not the words the person will read,
    /// which are rendered separately and may be in any language.
    #[test]
    fn attested_effects_are_claimed_verbatim() {
        let outcome = Outcome::attested(
            "s1".into(),
            ev("row"),
            "scheduled a reminder for 1234 ms".to_string(),
            Rendering::new(
                "reminder_created",
                serde_json::json!({ "when_ms": 1234, "about": "позвонить марку" }),
            ),
        );
        let receipt = build_receipt("int_2", &[outcome]);
        assert_eq!(receipt.status, ReceiptStatus::Verified);
        assert!(receipt.claims[0].claim.contains("scheduled a reminder"));
        // the audit trail is english whatever the person speaks
        assert!(receipt.claims[0].claim.is_ascii());
    }

    /// A failed provider call must not produce a Verified receipt, and the
    /// reply says what went wrong -- as structure, for the surface to say.
    #[test]
    fn failures_are_not_verified() {
        let outcome = Outcome::failed(
            "s1".into(),
            ev("deterministic"),
            "the model call failed".to_string(),
            Rendering::bare("provider_failure"),
        );
        let receipt = build_receipt("int_3", std::slice::from_ref(&outcome));
        assert_eq!(receipt.status, ReceiptStatus::Failed);

        let parts = reply_parts(std::slice::from_ref(&outcome), &receipt);
        assert!(matches!(&parts[0], ReplyPart::Say(r) if r.id == "provider_failure"));
        assert!(matches!(&parts[1], ReplyPart::Say(r) if r.id == "failed_note"));
    }

    /// Q26, restored and made structural.
    ///
    /// The old check scanned for phrases like "i saved it" and had to be
    /// written once per language, so it was blind in every language nobody
    /// listed. This one asks whether the STRUCTURE supports the claim,
    /// which has nothing to translate and nothing to evade.
    #[test]
    fn a_read_only_step_may_not_announce_a_change() {
        let step = |effect: Effect| PlanStep {
            step_id: "s1".into(),
            capability: "memory.recall".into(),
            args: serde_json::json!({}),
            effect,
            approval: Approval::Auto,
            deps: vec![],
        };
        let plan = |effect: Effect| Plan {
            plan_id: "p".into(),
            intent_id: "i".into(),
            lang: "en".into(),
            steps: vec![step(effect)],
        };
        // a capability that reads, announcing a deletion
        let bad = Outcome::attested(
            "s1".into(),
            ev("deterministic"),
            "read some facts".into(),
            Rendering::new("forgotten", serde_json::json!({ "content": "x" })),
        );
        assert!(
            unsupported_effect_claim(std::slice::from_ref(&bad), &plan(Effect::Read))
                .is_some(),
            "a read that announces a deletion must be caught"
        );
        // the same rendering from a step that really deletes is fine
        assert!(unsupported_effect_claim(
            std::slice::from_ref(&bad),
            &plan(Effect::Irreversible)
        )
        .is_none());

        // and an ordinary read announcing an ordinary read is fine
        let fine = Outcome::attested(
            "s1".into(),
            ev("deterministic"),
            "read some facts".into(),
            Rendering::bare("recall_empty"),
        );
        assert!(
            unsupported_effect_claim(&[fine], &plan(Effect::Read)).is_none()
        );
    }

    /// The receipts law without a phrase list (sec 5 / Q26).
    ///
    /// The old check scanned the reply for wordings like "i saved it", in
    /// every language we had thought of -- so an unlisted language was an
    /// unchecked one. Now the truth is compiled from the receipt and shown
    /// beside the claim. A model can say whatever it likes; if nothing
    /// happened, no record appears, in any language at once.
    #[test]
    fn effect_claims_are_answered_by_records_not_by_phrase_matching() {
        // a model asserting an effect on a turn that executed none
        let spoke = Outcome::utterance(
            "s1".into(),
            ev("provider_response"),
            "Sure! I've saved that to your memory.".into(),
        );
        let receipt = build_receipt("int_4", std::slice::from_ref(&spoke));
        let read_only = Plan {
            plan_id: "p".into(),
            intent_id: "int_4".into(),
            lang: "en".into(),
            steps: vec![PlanStep {
                step_id: "s1".into(),
                capability: "answer.model".into(),
                args: serde_json::json!({}),
                effect: Effect::Read,
                approval: Approval::Auto,
                deps: vec![],
            }],
        };
        assert!(
            action_records(&receipt, std::slice::from_ref(&spoke), &read_only).is_empty(),
            "an utterance is not an action"
        );

        // ...and the same sentence when a capability really did the work
        let did = Outcome::attested(
            "s2".into(),
            ev("row"),
            "stored a fact with its source".into(),
            Rendering::bare("remembered"),
        );
        let receipt = build_receipt("int_5", std::slice::from_ref(&did));
        let plan = Plan {
            plan_id: "p".into(),
            intent_id: "int_5".into(),
            lang: "en".into(),
            steps: vec![PlanStep {
                step_id: "s2".into(),
                capability: "memory.remember".into(),
                args: serde_json::json!({}),
                effect: Effect::ReversibleWrite,
                approval: Approval::Auto,
                deps: vec![],
            }],
        };
        let records = action_records(&receipt, std::slice::from_ref(&did), &plan);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tool, "memory.remember");
        assert_eq!(records[0].status, "verified");
    }
}

#[cfg(test)]
mod decline_tests {
    use super::*;

    /// A refusal must not produce a tick. The action record says what
    /// happened; a write that was declined did not happen, and showing
    /// "✓ soul.set" beside "i can't" is the receipts surface contradicting
    /// itself.
    #[test]
    fn a_declined_write_earns_no_action_record() {
        let declined = Outcome::attested(
            "s1".into(),
            vec![Evidence {
                kind: "declined".into(),
                provider: "robot".into(),
                external_id: "soul.set".into(),
                hash: String::new(),
                ts: 0,
            }],
            "declined: soul.set".into(),
            Rendering::bare("soul_refused"),
        );
        let plan = Plan {
            plan_id: "p".into(),
            intent_id: "i".into(),
            lang: "en".into(),
            steps: vec![PlanStep {
                step_id: "s1".into(),
                capability: "soul.set".into(),
                args: serde_json::json!({}),
                // a WRITE that did not write
                effect: Effect::ReversibleWrite,
                approval: Approval::Auto,
                deps: vec![],
            }],
        };
        let receipt = build_receipt("i", std::slice::from_ref(&declined));
        assert!(
            action_records(&receipt, std::slice::from_ref(&declined), &plan).is_empty(),
            "a refusal must not look like a write"
        );
    }
}
