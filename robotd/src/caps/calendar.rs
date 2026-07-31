//! Calendar (Q29: Google first, native connector).
//!
//! The effect classes here are the interesting part. Listing is a read.
//! Creating an event with no attendees is a reversible write — it can be
//! deleted and nobody saw it. Creating one **with attendees sends mail to
//! other people**, and that is irreversible no matter what the API calls
//! it: you cannot un-notify someone. So `calendar.create` declares
//! `Irreversible` when attendees are present, which routes it through the
//! confirmation gate, and `ReversibleWrite` when they are not.
//!
//! Times are structural arguments (§2d): RFC 3339, language-free. The model
//! resolves "next Tuesday at three" against the turn's clock and hands over
//! an instant. The title is a content argument and arrives in the person's
//! own words.

use super::{attested, note_evidence, typed, Capability, Ctx};
use hub::google;
use prism::types::{Effect, Outcome, Rendering};
use prism::PrismError;
use serde::Deserialize;

/// A window a person means when they say "this week". Long enough to be
/// useful, short enough that the reply is readable.
const MAX_EVENTS: usize = 25;

fn rfc3339(s: &str) -> Result<chrono::DateTime<chrono::FixedOffset>, String> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map_err(|_| format!("{s} is not an RFC 3339 timestamp"))
}

/// Accepts either a date (all-day) or a full timestamp, and says which.
fn instant_or_date(s: &str) -> Result<(), String> {
    if s.contains('T') {
        rfc3339(s).map(|_| ())
    } else {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(|_| ())
            .map_err(|_| format!("{s} is not a date or an RFC 3339 timestamp"))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    from: String,
    to: String,
}

pub struct List;

impl Capability for List {
    fn name(&self) -> &'static str {
        "calendar.list"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Look at what is on the person's calendar between two times. Use for \
         any question about their schedule -- what's on today, are they free \
         Thursday afternoon, when is the meeting. Resolve relative times \
         like 'tomorrow' against the current time before calling, and give \
         the widest window that answers the question."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "from": {
                    "type": "string",
                    "description": "Start of the window, RFC 3339 with offset, \
                                    e.g. 2026-08-01T00:00:00+02:00."
                },
                "to": {
                    "type": "string",
                    "description": "End of the window, RFC 3339 with offset."
                }
            },
            "required": ["from", "to"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: ListArgs = typed(args)?;
        let (f, t) = (rfc3339(&a.from)?, rfc3339(&a.to)?);
        if t <= f {
            return Err("the window ends before it starts".into());
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: ListArgs = typed(args).map_err(PrismError::Capability)?;
        let reach = match ctx.google() {
            Ok(r) => r,
            // no client configured is the operator's problem, not the
            // person's -- say so instead of failing opaquely
            Err(_) => {
                return super::declined("calendar.list", Rendering::bare("connect_unconfigured"))
            }
        };
        let url = google::events_url(&a.from, &a.to, MAX_EVENTS);
        let v = match reach.call(ctx.cell, "GET", &url, None, "calendar.list") {
            Ok(v) => v,
            Err(t) => return crate::connectors::stumbled("calendar.list", t),
        };

        let events: Vec<google::Event> = v["items"]
            .as_array()
            .map(|a| a.iter().map(google::parse_event).collect())
            .unwrap_or_default();

        let say = if events.is_empty() {
            Rendering::new(
                "calendar_empty",
                serde_json::json!({ "from": a.from, "to": a.to }),
            )
        } else {
            let items: Vec<serde_json::Value> = events
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "title": e.title,
                        "start": e.start,
                        "end": e.end,
                        "location": e.location,
                        "all_day": e.all_day,
                        "attendees": e.attendees.len(),
                    })
                })
                .collect();
            Rendering::new("calendar_list", serde_json::json!({ "items": items }))
        };
        attested(
            note_evidence("calendar.list"),
            format!("read {} events from google calendar", events.len()),
            say,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    title: String,
    start: String,
    end: String,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    attendees: Vec<String>,
    #[serde(default)]
    timezone: Option<String>,
}

pub struct Create;

impl Capability for Create {
    fn name(&self) -> &'static str {
        "calendar.create"
    }
    /// The pessimistic class, because the effect class is declared once and
    /// statically while the presence of attendees is known only per call.
    /// Declaring `ReversibleWrite` and hoping would mean an event that mails
    /// six people slipping past the confirmation gate.
    fn effect(&self) -> Effect {
        Effect::Irreversible
    }
    fn description(&self) -> &'static str {
        "Put something on the person's calendar. Use when they ask you to \
         schedule, book, or add an event. Resolve relative times against the \
         current time first. Only list attendees if the person named people \
         to invite -- adding an attendee sends them an invitation."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "What the event is called. The person's own \
                                    words, not a summary of them."
                },
                "start": {
                    "type": "string",
                    "description": "RFC 3339 with offset for a timed event, or \
                                    YYYY-MM-DD for an all-day one."
                },
                "end": {
                    "type": "string",
                    "description": "Same form as start. For an all-day event this \
                                    is the day AFTER the last day."
                },
                "location": { "type": "string", "description": "Where, if they said." },
                "attendees": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Email addresses to invite. Each one receives \
                                    an invitation, so include someone only if the \
                                    person asked to invite them."
                },
                "timezone": {
                    "type": "string",
                    "description": "IANA name, e.g. Europe/Berlin. Omit to use the \
                                    offset already in the timestamps."
                }
            },
            "required": ["title", "start", "end"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: CreateArgs = typed(args)?;
        if a.title.trim().is_empty() {
            return Err("an event needs a title".into());
        }
        instant_or_date(&a.start)?;
        instant_or_date(&a.end)?;
        if a.start.contains('T') != a.end.contains('T') {
            return Err("start and end must both be timed, or both all-day".into());
        }
        if a.start.contains('T') && rfc3339(&a.end)? <= rfc3339(&a.start)? {
            return Err("the event ends before it starts".into());
        }
        for who in &a.attendees {
            if !who.contains('@') || who.contains(char::is_whitespace) {
                return Err(format!("{who} is not an email address"));
            }
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: CreateArgs = typed(args).map_err(PrismError::Capability)?;
        let reach = match ctx.google() {
            Ok(r) => r,
            // no client configured is the operator's problem, not the
            // person's -- say so instead of failing opaquely
            Err(_) => {
                return super::declined("calendar.create", Rendering::bare("connect_unconfigured"))
            }
        };
        let tz = a.timezone.as_deref().unwrap_or("UTC");
        let body = google::event_body(
            &a.title,
            &a.start,
            &a.end,
            tz,
            a.location.as_deref(),
            &a.attendees,
        );
        let v = match reach.call(
            ctx.cell,
            "POST",
            &format!("{}/calendars/primary/events", google::CAL_BASE),
            Some(&body),
            "calendar.create",
        ) {
            Ok(v) => v,
            Err(t) => return crate::connectors::stumbled("calendar.create", t),
        };
        let e = google::parse_event(&v);
        if e.id.is_empty() {
            return Err(PrismError::Capability(
                "google accepted the event but returned no id -- refusing to claim it".into(),
            ));
        }
        attested(
            super::row_evidence(&e.id, &trust::ids::sha256_hex(v.to_string().as_bytes())),
            format!("created google calendar event {}", e.id),
            Rendering::new(
                "calendar_created",
                serde_json::json!({
                    "title": e.title,
                    "start": e.start,
                    "attendees": e.attendees.len(),
                }),
            ),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelArgs {
    title: String,
    from: String,
    to: String,
}

pub struct Cancel;

impl Capability for Cancel {
    fn name(&self) -> &'static str {
        "calendar.cancel"
    }
    fn effect(&self) -> Effect {
        Effect::Irreversible
    }
    fn description(&self) -> &'static str {
        "Cancel an event on the person's calendar. Use when they ask to \
         cancel, delete or call off something. Give the event's title and a \
         time window to find it in; if several match, nothing is cancelled \
         and you will be told which ones matched."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "The event's title, or a distinctive part of it."
                },
                "from": { "type": "string", "description": "Window start, RFC 3339." },
                "to": { "type": "string", "description": "Window end, RFC 3339." }
            },
            "required": ["title", "from", "to"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: CancelArgs = typed(args)?;
        if a.title.trim().is_empty() {
            return Err("say which event".into());
        }
        rfc3339(&a.from)?;
        rfc3339(&a.to)?;
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: CancelArgs = typed(args).map_err(PrismError::Capability)?;
        let reach = match ctx.google() {
            Ok(r) => r,
            // no client configured is the operator's problem, not the
            // person's -- say so instead of failing opaquely
            Err(_) => {
                return super::declined("calendar.cancel", Rendering::bare("connect_unconfigured"))
            }
        };
        let url = google::events_url(&a.from, &a.to, MAX_EVENTS);
        let v = match reach.call(ctx.cell, "GET", &url, None, "calendar.cancel") {
            Ok(v) => v,
            Err(t) => return crate::connectors::stumbled("calendar.cancel", t),
        };

        let needle = a.title.trim().to_lowercase();
        let hits: Vec<google::Event> = v["items"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(google::parse_event)
                    .filter(|e| e.title.to_lowercase().contains(&needle))
                    .collect()
            })
            .unwrap_or_default();

        // Ambiguity is not resolved by picking. Deleting the wrong meeting
        // is not recoverable by apologising to the calendar.
        match hits.as_slice() {
            [] => attested(
                note_evidence("calendar.cancel"),
                format!("no event matching {} in that window", a.title),
                Rendering::new("calendar_no_match", serde_json::json!({ "title": a.title })),
            ),
            [one] => {
                match reach.call(
                    ctx.cell,
                    "DELETE",
                    &google::event_url(&one.id),
                    None,
                    "calendar.cancel",
                ) {
                    Ok(v) => v,
                    Err(t) => return crate::connectors::stumbled("calendar.cancel", t),
                };
                attested(
                    super::row_evidence(&one.id, &one.id),
                    format!("deleted google calendar event {}", one.id),
                    Rendering::new(
                        "calendar_cancelled",
                        serde_json::json!({ "title": one.title, "start": one.start }),
                    ),
                )
            }
            many => {
                let items: Vec<serde_json::Value> = many
                    .iter()
                    .map(|e| serde_json::json!({ "title": e.title, "start": e.start }))
                    .collect();
                attested(
                    note_evidence("calendar.cancel"),
                    format!("{} events matched; cancelled none", many.len()),
                    Rendering::new("calendar_ambiguous", serde_json::json!({ "items": items })),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_must_be_a_real_window() {
        let ok = serde_json::json!({
            "from": "2026-08-01T00:00:00+02:00", "to": "2026-08-02T00:00:00+02:00"
        });
        assert!(List.validate(&ok).is_ok());
        assert!(List
            .validate(&serde_json::json!({"from": "tomorrow", "to": "friday"}))
            .is_err());
        assert!(
            List.validate(&serde_json::json!({
                "from": "2026-08-02T00:00:00Z", "to": "2026-08-01T00:00:00Z"
            }))
            .is_err(),
            "backwards"
        );
    }

    #[test]
    fn an_event_is_checked_before_google_ever_hears_about_it() {
        let base = |extra: serde_json::Value| {
            let mut v = serde_json::json!({
                "title": "lunch",
                "start": "2026-08-01T12:00:00+02:00",
                "end": "2026-08-01T13:00:00+02:00"
            });
            for (k, val) in extra.as_object().unwrap() {
                v[k] = val.clone();
            }
            v
        };
        assert!(Create.validate(&base(serde_json::json!({}))).is_ok());
        assert!(Create
            .validate(&base(serde_json::json!({"attendees": ["a@b.com"]})))
            .is_ok());
        assert!(
            Create
                .validate(&base(serde_json::json!({"attendees": ["not an address"]})))
                .is_err(),
            "an invitation goes to a person; the address must look like one"
        );
        assert!(
            Create
                .validate(&base(serde_json::json!({"end": "2026-08-01T11:00:00+02:00"})))
                .is_err(),
            "ends before it starts"
        );
        assert!(
            Create.validate(&base(serde_json::json!({"end": "2026-08-02"}))).is_err(),
            "half all-day, half timed"
        );
        assert!(Create
            .validate(&serde_json::json!({
                "title": "birthday", "start": "2026-08-01", "end": "2026-08-02"
            }))
            .is_ok());
        assert!(Create
            .validate(&base(serde_json::json!({"title": "  "})))
            .is_err());
    }

    /// An event with attendees mails people. There is no un-mailing, so the
    /// declared class must be the one that reaches the confirmation gate.
    #[test]
    fn creating_and_cancelling_are_both_irreversible() {
        assert_eq!(Create.effect(), Effect::Irreversible);
        assert_eq!(Cancel.effect(), Effect::Irreversible);
        assert_eq!(List.effect(), Effect::Read);
    }
}
