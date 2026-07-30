//! Reminder capabilities: the commitment ledger's write surface.

use super::{attested, mind_err, note_evidence, row_evidence, Capability, Ctx};
use prism::types::{Effect, Outcome};
use prism::PrismError;

pub struct Create;

impl Capability for Create {
    fn name(&self) -> &'static str {
        "reminder.create"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let fire_at = args["fire_at"]
            .as_i64()
            .ok_or_else(|| PrismError::Capability("reminder.create: fire_at missing".into()))?;
        let about = args["about"]
            .as_str()
            .ok_or_else(|| PrismError::Capability("reminder.create: about missing".into()))?;
        // idempotent per intent (UNIQUE(intent_id)): replay cannot double-book
        let rem = ctx
            .cell
            .with(|c| mind::reminders::create(c, ctx.intent_id, fire_at, about).map_err(mind_err))?;
        attested(
            row_evidence(&rem.id, &trust::ids::sha256_hex(about.as_bytes())),
            ctx.say(
                "reminder_created",
                &[
                    ("when", &ctx.pack().datetime_ms("fire_at", rem.fire_at)),
                    ("about", &rem.about),
                ],
            ),
        )
    }
}

pub struct List;

impl Capability for List {
    fn name(&self) -> &'static str {
        "reminder.list"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let all = ctx
            .cell
            .with(|c| mind::reminders::list_active(c).map_err(mind_err))?;
        let detail = if all.is_empty() {
            ctx.say("reminder_list_empty", &[])
        } else {
            let lines: Vec<String> = all
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    ctx.say(
                        "reminder_line",
                        &[
                            ("n", &(i + 1).to_string()),
                            ("when", &ctx.pack().datetime_ms("fire_at", r.fire_at)),
                            ("about", &r.about),
                        ],
                    )
                })
                .collect();
            format!("{}\n{}", ctx.say("reminder_list_header", &[]), lines.join("\n"))
        };
        attested(note_evidence("reminder.list"), detail)
    }
}

pub struct CancelLast;

impl Capability for CancelLast {
    fn name(&self) -> &'static str {
        "reminder.cancel_last"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        // "the latest" is resolved at execution time, so this is op-marker
        // guarded: replay must not cancel a second reminder
        match ctx
            .cell
            .with(|c| mind::reminders::cancel_latest(c, ctx.intent_id).map_err(mind_err))?
        {
            Some(rem) => attested(
                row_evidence(&rem.id, ""),
                ctx.say("reminder_cancelled", &[("about", &rem.about)]),
            ),
            None => attested(
                note_evidence("reminder.cancel_last"),
                ctx.say("reminder_nothing_to_cancel", &[]),
            ),
        }
    }
}
