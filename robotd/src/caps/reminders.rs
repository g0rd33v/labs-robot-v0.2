//! Reminder capabilities: the commitment ledger's write surface.

use super::{attested, mind_err, note_evidence, row_evidence, typed, Capability, Ctx};
use chrono::{DateTime, Local};
use prism::types::{Effect, Outcome, Rendering};
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

/// The pending clarify, in `cell_meta` under one key: at most one time
/// question is open at a time, because two would make "2" ambiguous --
/// which is the exact failure this feature exists to prevent.
pub const CLARIFY_KEY: &str = "reminder:clarify";
/// A question nobody answers should not answer itself later. Ten minutes
/// matches the confirmation gate; past that the person has moved on.
pub const CLARIFY_TTL_MS: i64 = 10 * 60_000;

/// R4.3.1: ask, with options, and never guess.
pub struct Clarify;

impl Capability for Clarify {
    fn name(&self) -> &'static str {
        "reminder.clarify"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Ask which exact time the person meant, when they gave a vague one \
         (\"in the morning\", \"this evening\", \"at lunch\"). Offer two or \
         three concrete times and let them pick. Use this INSTEAD of \
         choosing an hour yourself -- a reminder at a time they did not ask \
         for is worse than a question."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "about": {
                    "type": "string",
                    "description": "What to be reminded of, in their own words."
                },
                "options": {
                    "type": "array",
                    "minItems": 2,
                    "maxItems": 3,
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": { "type": "string", "description": "e.g. 09:00" },
                            "at_ms": { "type": "integer", "description": "epoch ms" }
                        },
                        "required": ["label", "at_ms"],
                        "additionalProperties": false
                    },
                    "description": "Two or three concrete times, soonest first."
                }
            },
            "required": ["about", "options"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let about = args["about"].as_str().unwrap_or_default();
        if about.trim().is_empty() {
            return Err("clarify what, exactly?".into());
        }
        let opts = args["options"].as_array().ok_or("options must be a list")?;
        if !(2..=3).contains(&opts.len()) {
            return Err("offer two or three options -- a list is not a choice".into());
        }
        for o in opts {
            if o["at_ms"].as_i64().unwrap_or(0) <= 0 {
                return Err("each option needs a real timestamp".into());
            }
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let about = args["about"].as_str().unwrap_or_default().to_string();
        let options = args["options"].clone();
        let parked = serde_json::json!({
            "about": about,
            "options": options,
            "asked_at": trust::ids::ts_ms(),
        });
        ctx.cell.with(|c| {
            c.execute(
                "INSERT INTO cell_meta(key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![CLARIFY_KEY, parked.to_string()],
            )
            .map_err(mind_err)
        })?;
        attested(
            note_evidence("reminder.clarify"),
            format!("asked which time was meant for: {about}"),
            Rendering::new(
                "clarify_time",
                serde_json::json!({ "about": about, "options": options }),
            ),
        )
    }
}

/// The answer to an open time question, if this message is one.
///
/// Deliberately narrow: a bare number, or a bare time label. Anything
/// wordier is a new message, not an answer -- a person who typed a
/// sentence is talking, and treating that as a pick would set a reminder
/// they never chose.
pub fn clarify_answer(cell: &prism::Cell, text: &str) -> Option<(String, i64)> {
    let parked: String = cell
        .with(|c| {
            Ok(c.query_row(
                "SELECT value FROM cell_meta WHERE key = ?1",
                rusqlite::params![CLARIFY_KEY],
                |r| r.get::<_, String>(0),
            )
            .ok())
        })
        .ok()
        .flatten()?;
    let v: serde_json::Value = serde_json::from_str(&parked).ok()?;
    let asked_at = v["asked_at"].as_i64().unwrap_or(0);
    if trust::ids::ts_ms() - asked_at > CLARIFY_TTL_MS {
        return None;
    }
    let options = v["options"].as_array()?;
    let about = v["about"].as_str()?.to_string();
    let t = text.trim().trim_end_matches(['.', '!']).trim();

    // "2" -- the universal answer, in any language
    if let Ok(n) = t.parse::<usize>() {
        if n >= 1 && n <= options.len() {
            return options[n - 1]["at_ms"].as_i64().map(|at| (about, at));
        }
    }
    // or the label itself: "09:00", "9:00"
    for o in options {
        if let Some(label) = o["label"].as_str() {
            if t == label || t.trim_start_matches('0') == label.trim_start_matches('0') {
                return o["at_ms"].as_i64().map(|at| (about, at));
            }
        }
    }
    None
}

/// Clear the open question -- answered, or overtaken by events.
pub fn clear_clarify(cell: &prism::Cell) {
    let _ = cell.with(|c| {
        c.execute(
            "DELETE FROM cell_meta WHERE key = ?1",
            rusqlite::params![CLARIFY_KEY],
        )
        .map_err(mind_err)
    });
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
                    "description": "What to be reminded ABOUT -- the thing itself, \
                                    and nothing else. Take the run of words from \
                                    their message that names what they want to do \
                                    or know. Leave out the words asking for a \
                                    reminder and the words giving the time: \
                                    \"remind me tomorrow at 8 to go to the gym\" \
                                    gives \"go to the gym\" -- not \"remind me to go \
                                    to the gym\", not \"tomorrow at 8 go to the \
                                    gym\". Whatever you take must be their OWN \
                                    words, in their own language, unchanged: never \
                                    translated, never rephrased, never tidied up. \
                                    They read this back later and it should sound \
                                    like them."
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
            format!("scheduled a reminder for {} ms", rem.fire_at),
            Rendering::new(
                "reminder_created",
                serde_json::json!({ "when_ms": rem.fire_at, "about": rem.about }),
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
         or what they asked to be reminded of. Only about the FUTURE: a \
         question about the past -- when did i ask, what did i say, did you \
         remind me -- is a memory question, answered from memory with tool \
         \"none\", not a reminders listing."
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
        let say = if all.is_empty() {
            Rendering::bare("reminder_list_empty")
        } else {
            let items: Vec<serde_json::Value> = all
                .iter()
                .map(|r| serde_json::json!({ "when_ms": r.fire_at, "about": r.about }))
                .collect();
            Rendering::new("reminder_list", serde_json::json!({ "items": items }))
        };
        attested(
            note_evidence("reminder.list"),
            format!("listed {} pending reminders", all.len()),
            say,
        )
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
                "cancelled the most recent pending reminder",
                Rendering::new(
                    "reminder_cancelled",
                    serde_json::json!({ "about": rem.about }),
                ),
            ),
            None => attested(
                note_evidence("reminder.cancel_last"),
                "no pending reminder to cancel",
                Rendering::bare("reminder_nothing_to_cancel"),
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
