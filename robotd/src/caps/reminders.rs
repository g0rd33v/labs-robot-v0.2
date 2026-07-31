//! Reminder capabilities: the commitment ledger's write surface.

use super::{attested, mind_err, note_evidence, row_evidence, typed, Capability, Ctx};
use chrono::{DateTime, Local};
use prism::types::{Effect, Outcome};
use prism::PrismError;
use serde::Deserialize;

/// The furthest ahead a reminder may be set. A model that miscomputes a
/// date lands here rather than booking something for the year 3000.
const HORIZON_DAYS: i64 = 730;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    /// RFC 3339 with offset -- structural, so typed and language-free.
    fire_at: String,
    /// Content -- the person's own words, verbatim.
    about: String,
}

/// Parse and sanity-check a proposed fire time. This is where a model's
/// date arithmetic is caught: the schema can say "date-time", but only code
/// can say "and not in the past, and not two centuries out".
fn parse_fire_at(raw: &str) -> Result<i64, String> {
    let dt: DateTime<Local> = DateTime::parse_from_rfc3339(raw)
        .map_err(|e| format!("fire_at is not an RFC 3339 timestamp ({e})"))?
        .into();
    let ms = dt.timestamp_millis();
    let now = trust::ids::ts_ms();
    if ms <= now {
        return Err("fire_at is in the past".into());
    }
    if ms > now + HORIZON_DAYS * 86_400_000 {
        return Err(format!("fire_at is more than {HORIZON_DAYS} days away"));
    }
    Ok(ms)
}

pub struct Create;

impl Capability for Create {
    fn name(&self) -> &'static str {
        "reminder.create"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Schedule a reminder that fires at a specific moment and tells the \
         person what it is about. Use when they ask to be reminded, woken, \
         nudged, or told about something later -- whether they give a clock \
         time (\"at 18:30\"), a delay (\"in ten minutes\"), or a day \
         (\"tomorrow morning\"). Resolve all of those to an absolute \
         timestamp yourself."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "fire_at": {
                    "type": "string",
                    "format": "date-time",
                    "description": "When to fire, RFC 3339 with offset. Compute it \
                                    from the current local time given to you."
                },
                "about": {
                    "type": "string",
                    "description": "What the reminder is about, copied VERBATIM from \
                                    the person's message, in their own language. \
                                    Never translate or paraphrase this."
                }
            },
            "required": ["fire_at", "about"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: CreateArgs = typed(args)?;
        parse_fire_at(&a.fire_at)?;
        if a.about.trim().is_empty() {
            return Err("about is empty".into());
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: CreateArgs = typed(args).map_err(PrismError::Capability)?;
        let fire_at = parse_fire_at(&a.fire_at).map_err(PrismError::Capability)?;
        // idempotent per intent (UNIQUE(intent_id)): replay cannot double-book
        let rem = ctx.cell.with(|c| {
            mind::reminders::create(c, ctx.intent_id, fire_at, &a.about).map_err(mind_err)
        })?;
        attested(
            row_evidence(&rem.id, &trust::ids::sha256_hex(a.about.as_bytes())),
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
    fn description(&self) -> &'static str {
        "List the reminders that are still pending, with their times. Use \
         when the person asks what they have coming up, what is scheduled, \
         or what they asked to be reminded of."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
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
    fn description(&self) -> &'static str {
        "Cancel the most recently scheduled pending reminder. Use when the \
         person wants to call off a reminder without saying which one."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A model's date arithmetic is checked, never trusted. The schema can
    /// say "date-time"; only code can say "and not in the past".
    #[test]
    fn proposed_fire_times_are_checked_not_trusted() {
        let c = Create;
        let soon = (Local::now() + chrono::Duration::minutes(10)).to_rfc3339();
        assert!(c
            .validate(&serde_json::json!({"fire_at": soon, "about": "стретчинг"}))
            .is_ok());

        let hour = (Local::now() + chrono::Duration::hours(1)).to_rfc3339();
        for bad in [
            serde_json::json!({"fire_at": "2020-01-01T00:00:00+00:00", "about": "x"}),
            serde_json::json!({"fire_at": "3000-01-01T00:00:00+00:00", "about": "x"}),
            serde_json::json!({"fire_at": "tomorrow at nine", "about": "x"}),
            serde_json::json!({"fire_at": hour, "about": "  "}),
            serde_json::json!({"about": "x"}),
            serde_json::json!({"fire_at": hour, "about": "x", "surprise": 1}),
        ] {
            assert!(c.validate(&bad).is_err(), "should have been refused: {bad}");
        }
    }
}
