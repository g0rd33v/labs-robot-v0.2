//! Email (Q29: Gmail, native connector).
//!
//! Search, read and draft run automatically. **Sending does not.**
//! `email.send` declares `Approval::Required`, which is a statement the
//! capability makes about itself rather than a setting: §14's config may
//! widen approval requirements but never narrow them, so there is no
//! `robot.toml` edit and no persuasive turn of phrase that switches this
//! off. A message that has left cannot be recalled, and the recipient is a
//! third party who never agreed to any of this.
//!
//! The second hazard is the inbound direction. Email bodies are written by
//! whoever sent them and land in model context — the textbook injection
//! surface (§7a). Two things follow, both structural rather than hopeful:
//!
//! * Message text stays in a **content** field of a `Rendering`. It is
//!   never concatenated into an instruction, and the renderer quotes it.
//! * Reading mail never elevates anything. `email.send` needs a person's
//!   approval whether the idea came from the owner or from a sentence
//!   inside a message — approval is asked of the owner, in the chat, about
//!   a specific recipient and subject.

use super::{attested, note_evidence, typed, Capability, Ctx};
use hub::google;
use prism::types::{Approval, Effect, Outcome, Rendering};
use prism::PrismError;
use serde::Deserialize;

const MAX_HITS: usize = 10;
/// Enough of a message to work with; past this it is a document, and the
/// person is better served by being told it is long.
const MAX_BODY_CHARS: usize = 8_000;

fn valid_address(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && t.contains('@')
        && !t.contains(char::is_whitespace)
        && !t.contains(['\r', '\n'])
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
}

pub struct Search;

impl Capability for Search {
    fn name(&self) -> &'static str {
        "email.search"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Search the person's mailbox and return matching messages with their \
         sender, subject and a preview. Use for any question about their \
         email -- did so-and-so reply, what did the invoice say, anything \
         unread. Accepts Gmail search syntax such as 'from:someone \
         is:unread newer_than:7d'."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "A Gmail search query, e.g. 'from:anna@x.com \
                                    newer_than:14d'. Translate what the person \
                                    asked for into this syntax."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: SearchArgs = typed(args)?;
        if a.query.trim().is_empty() {
            return Err("say what to search for".into());
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: SearchArgs = typed(args).map_err(PrismError::Capability)?;
        let reach = match ctx.google() {
            Ok(r) => r,
            // no client configured is the operator's problem, not the
            // person's -- say so instead of failing opaquely
            Err(_) => {
                return super::declined("email.search", Rendering::bare("connect_unconfigured"))
            }
        };
        let found = match reach.call(
            ctx.cell,
            "GET",
            &google::search_url(&a.query, MAX_HITS),
            None,
            "email.search",
        ) {
            Ok(v) => v,
            Err(t) => return crate::connectors::stumbled("email.search", t),
        };

        let ids: Vec<String> = found["messages"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m["id"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // headers only: a search result is a list, and pulling ten full
        // bodies into context to render ten one-line previews is how a
        // mailbox search becomes a minute and a fortune in tokens
        let mut items = vec![];
        for id in &ids {
            let v = match reach.call(
                ctx.cell,
                "GET",
                &google::message_url(id),
                None,
                "email.search",
            ) {
                Ok(v) => v,
                Err(t) => return crate::connectors::stumbled("email.search", t),
            };
            let m = google::parse_message(&v);
            items.push(serde_json::json!({
                "id": m.id,
                "from": m.from,
                "subject": m.subject,
                "date": m.date,
                "preview": m.snippet,
            }));
        }

        let say = if items.is_empty() {
            Rendering::new("email_none", serde_json::json!({ "query": a.query }))
        } else {
            Rendering::new("email_list", serde_json::json!({ "items": items }))
        };
        attested(
            note_evidence("email.search"),
            format!("found {} messages", items.len()),
            say,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    id: String,
}

pub struct Read;

impl Capability for Read {
    fn name(&self) -> &'static str {
        "email.read"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Read one message in full, by the id that email.search returned. Use \
         when the preview is not enough to answer the person's question."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The message id from a previous email.search."
                }
            },
            "required": ["id"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: ReadArgs = typed(args)?;
        if a.id.trim().is_empty() {
            return Err("which message?".into());
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: ReadArgs = typed(args).map_err(PrismError::Capability)?;
        let reach = match ctx.google() {
            Ok(r) => r,
            // no client configured is the operator's problem, not the
            // person's -- say so instead of failing opaquely
            Err(_) => {
                return super::declined("email.read", Rendering::bare("connect_unconfigured"))
            }
        };
        let v = match reach.call(
            ctx.cell,
            "GET",
            &google::message_url(a.id.trim()),
            None,
            "email.read",
        ) {
            Ok(v) => v,
            Err(t) => return crate::connectors::stumbled("email.read", t),
        };
        let m = google::parse_message(&v);
        let mut body = m.body;
        let truncated = body.chars().count() > MAX_BODY_CHARS;
        if truncated {
            body = body.chars().take(MAX_BODY_CHARS).collect();
        }
        attested(
            note_evidence("email.read"),
            format!("read message {}", m.id),
            Rendering::new(
                "email_message",
                serde_json::json!({
                    "from": m.from,
                    "subject": m.subject,
                    "date": m.date,
                    "body": body,
                    "truncated": truncated,
                }),
            ),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComposeArgs {
    to: String,
    subject: String,
    body: String,
    #[serde(default)]
    in_reply_to: Option<String>,
}

impl ComposeArgs {
    fn check(&self) -> Result<(), String> {
        if !valid_address(&self.to) {
            return Err(format!("{} is not an email address", self.to));
        }
        if self.subject.trim().is_empty() {
            return Err("a message needs a subject".into());
        }
        if self.body.trim().is_empty() {
            return Err("there is nothing to say in it".into());
        }
        Ok(())
    }
}

fn compose_schema(what: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "to": { "type": "string", "description": "One recipient's email address." },
            "subject": { "type": "string", "description": "The subject line." },
            "body": {
                "type": "string",
                "description": format!(
                    "The message text. {what} Write it in the language the person \
                     is writing to their recipient in, which may not be the \
                     language of this conversation."
                )
            },
            "in_reply_to": {
                "type": "string",
                "description": "The Message-ID being replied to, if this is a reply."
            }
        },
        "required": ["to", "subject", "body"],
        "additionalProperties": false
    })
}

pub struct Draft;

impl Capability for Draft {
    fn name(&self) -> &'static str {
        "email.draft"
    }
    /// Reversible: a draft sits in their mailbox until someone acts on it,
    /// and deleting it costs nothing.
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Write a message and leave it in the person's drafts WITHOUT sending \
         it. Prefer this whenever they ask you to write, draft or prepare an \
         email. Nothing leaves their mailbox."
    }
    fn schema(&self) -> serde_json::Value {
        compose_schema("If they dictated it, keep their words.")
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        typed::<ComposeArgs>(args)?.check()
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: ComposeArgs = typed(args).map_err(PrismError::Capability)?;
        let reach = match ctx.google() {
            Ok(r) => r,
            // no client configured is the operator's problem, not the
            // person's -- say so instead of failing opaquely
            Err(_) => {
                return super::declined("email.draft", Rendering::bare("connect_unconfigured"))
            }
        };
        let raw = google::compose_raw(&a.to, &a.subject, &a.body, a.in_reply_to.as_deref());
        let v = match reach.call(
            ctx.cell,
            "POST",
            &format!("{}/users/me/drafts", google::GMAIL_BASE),
            Some(&serde_json::json!({ "message": { "raw": raw } })),
            "email.draft",
        ) {
            Ok(v) => v,
            Err(t) => return crate::connectors::stumbled("email.draft", t),
        };
        let id = v["id"].as_str().unwrap_or_default().to_string();
        if id.is_empty() {
            return Err(PrismError::Capability(
                "gmail accepted the draft but returned no id -- refusing to claim it".into(),
            ));
        }
        attested(
            super::row_evidence(&id, &id),
            format!("saved gmail draft {id}"),
            Rendering::new(
                "email_drafted",
                serde_json::json!({ "to": a.to, "subject": a.subject }),
            ),
        )
    }
}

pub struct Send;

impl Capability for Send {
    fn name(&self) -> &'static str {
        "email.send"
    }
    fn effect(&self) -> Effect {
        Effect::Irreversible
    }
    /// Declared here, beside the code that sends. Config may add approval
    /// requirements but never remove them, so this cannot be switched off
    /// by a setting -- and a message that has left cannot be recalled.
    fn approval(&self) -> Approval {
        Approval::Required
    }
    fn description(&self) -> &'static str {
        "Send an email on the person's behalf. This one actually leaves the \
         mailbox and cannot be taken back, so it always waits for the \
         person's explicit approval first. If they only asked you to write \
         something, use email.draft instead."
    }
    fn schema(&self) -> serde_json::Value {
        compose_schema("These are the exact words that will be sent.")
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        typed::<ComposeArgs>(args)?.check()
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: ComposeArgs = typed(args).map_err(PrismError::Capability)?;
        let reach = match ctx.google() {
            Ok(r) => r,
            // no client configured is the operator's problem, not the
            // person's -- say so instead of failing opaquely
            Err(_) => {
                return super::declined("email.send", Rendering::bare("connect_unconfigured"))
            }
        };
        let raw = google::compose_raw(&a.to, &a.subject, &a.body, a.in_reply_to.as_deref());
        let v = match reach.call(
            ctx.cell,
            "POST",
            &format!("{}/users/me/messages/send", google::GMAIL_BASE),
            Some(&serde_json::json!({ "raw": raw })),
            "email.send",
        ) {
            Ok(v) => v,
            Err(t) => return crate::connectors::stumbled("email.send", t),
        };
        let id = v["id"].as_str().unwrap_or_default().to_string();
        if id.is_empty() {
            return Err(PrismError::Capability(
                "gmail returned no message id -- refusing to claim it was sent".into(),
            ));
        }
        attested(
            super::row_evidence(&id, &id),
            format!("sent gmail message {id}"),
            Rendering::new(
                "email_sent",
                serde_json::json!({ "to": a.to, "subject": a.subject }),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of item 5. If this ever reads `Auto`, a model can
    /// mail anyone on the strength of a sentence in an inbound message.
    #[test]
    fn sending_always_waits_for_a_person_and_drafting_never_does() {
        assert_eq!(Send.approval(), Approval::Required);
        assert_eq!(Send.effect(), Effect::Irreversible);
        assert_eq!(Draft.approval(), Approval::Auto);
        assert_eq!(Draft.effect(), Effect::ReversibleWrite);
        assert_eq!(Search.effect(), Effect::Read);
        assert_eq!(Read.effect(), Effect::Read);
    }

    #[test]
    fn a_message_is_checked_before_anything_is_composed() {
        let ok = serde_json::json!({"to": "a@b.com", "subject": "hi", "body": "hello"});
        assert!(Send.validate(&ok).is_ok());
        assert!(Draft.validate(&ok).is_ok());

        for bad in [
            serde_json::json!({"to": "not an address", "subject": "s", "body": "b"}),
            serde_json::json!({"to": "a@b.com", "subject": " ", "body": "b"}),
            serde_json::json!({"to": "a@b.com", "subject": "s", "body": "  "}),
            serde_json::json!({"to": "a@b.com\nBcc: x@y.com", "subject": "s", "body": "b"}),
        ] {
            assert!(Send.validate(&bad).is_err(), "{bad}");
            assert!(Draft.validate(&bad).is_err(), "{bad}");
        }
    }

    /// Both compose paths must reject a recipient carrying a line break,
    /// before it ever reaches the encoder that would flatten it.
    #[test]
    fn a_recipient_may_not_contain_a_line_break() {
        assert!(!valid_address("a@b.com\r\nBcc: attacker@evil.com"));
        assert!(!valid_address("a@b.com\nBcc: attacker@evil.com"));
        assert!(!valid_address("two @addresses.com"));
        assert!(!valid_address(""));
        assert!(valid_address("a@b.com"));
        assert!(valid_address("  a@b.com  "), "surrounding space is fine");
    }
}
