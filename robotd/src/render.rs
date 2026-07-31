//! The surface: where structure becomes sentences.
//!
//! This is the only file in the robot that contains things a person reads.
//! The kernel hands over `ReplyPart`s -- an English id and typed slots --
//! and this turns them into words:
//!
//! * **English** from templates below. Free, instant, exact, offline. It is
//!   in code rather than in a catalog because English is the kernel's own
//!   language: these are not translations, they are the originals.
//! * **Every other language** from one model call, given the same structure.
//!   No file to author, no list to keep in step with the capabilities, and
//!   no count of supported languages anywhere.
//!
//! Model prose passes through untouched -- it is already in the person's
//! language, and re-rendering it would be a second chance to get it wrong.

use chrono::{Local, TimeZone};
use prism::lifecycle::Renderer;
use prism::types::{ActionRecord, Rendering, ReplyPart};
use std::sync::Arc;

pub struct Speak {
    pub gateway: Option<Arc<hub::ModelGateway>>,
}

impl Speak {
    pub fn offline() -> Self {
        Self { gateway: None }
    }
}

/// A local timestamp in English. Other languages get their dates from the
/// model, which knows every calendar without us shipping one.
fn when(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).earliest() {
        Some(dt) => dt.format("%H:%M on %a, %-d %b").to_string(),
        None => format!("t+{ms}ms"),
    }
}

fn when_full(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).earliest() {
        Some(dt) => dt.format("%H:%M on %A, %-d %B %Y").to_string(),
        None => format!("t+{ms}ms"),
    }
}

fn s(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn n(v: &serde_json::Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64()).unwrap_or(0)
}

fn items(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v.get("items")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default()
}

/// The English original for one rendering.
///
/// An id with no arm here still produces something usable rather than
/// silence -- a missing template is a bug, not a reason to say nothing.
pub fn english(r: &Rendering) -> String {
    let a = &r.slots;
    match r.id.as_str() {
        // ---- basics ----
        "time_now" => format!("it's {}", when_full(n(a, "at_ms"))),
        "self_meta" => "i'm bender -- your robot, v0.2. i run on this machine only; \
             your words live in an encrypted cell here, every crossing is in the \
             boundary log, and everything i claim to have done carries a receipt. \
             i can tell time, keep reminders, and remember facts you tell me -- \
             every fact with its source, listed in my registry, correctable and \
             deletable for real."
            .into(),
        "help" => "here's what works today:\n\
             - time and date\n\
             - reminders -- \"remind me in 10 minutes to stretch\", \"remind me at \
             18:30 to call mark\"\n\
             - \"my reminders\" / \"cancel reminder\"\n\
             - memory -- \"remember that i drink green tea\", \"what do you remember \
             about tea\"\n\
             - registry -- \"my facts\" (every fact and its source), \"forget fact \
             2\", \"correct fact 1: ...\"\n\
             - search -- \"look up the weather in porto\"\n\
             - \"who are you\" -- about me\n\
             write in any language; the english phrasings above are just the ones \
             that answer without asking a model."
            .into(),

        // ---- reminders ----
        "reminder_created" => format!(
            "done -- i'll remind you at {}: {}",
            when(n(a, "when_ms")),
            s(a, "about")
        ),
        "reminder_list" => {
            let lines: Vec<String> = items(a)
                .iter()
                .enumerate()
                .map(|(i, it)| format!("{}. {} -- {}", i + 1, when(n(it, "when_ms")), s(it, "about")))
                .collect();
            format!("your reminders:\n{}", lines.join("\n"))
        }
        "reminder_list_empty" => "no active reminders.".into(),
        "reminder_cancelled" => format!("cancelled: {}", s(a, "about")),
        "reminder_nothing_to_cancel" => "nothing to cancel -- no active reminders.".into(),
        "reminder_fired" => format!("⏰ reminder: {}", s(a, "about")),

        // ---- memory ----
        "remembered" => format!(
            "remembered: {}\n(source kept -- see your registry; \"forget fact N\" \
             deletes for real)",
            s(a, "content")
        ),
        "recall" => {
            let lines: Vec<String> = items(a)
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    format!("{}. {} (learned {})", i + 1, s(it, "fact"), when(n(it, "when_ms")))
                })
                .collect();
            format!("here's what i remember:\n{}", lines.join("\n"))
        }
        "recall_empty" => "nothing in memory yet -- tell me \"remember ...\" and i'll \
             keep it, with its source."
            .into(),
        "registry" => {
            let lines: Vec<String> = items(a)
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    format!(
                        "{}. {} -- from your words: \"{}\" ({})",
                        i + 1,
                        s(it, "fact"),
                        s(it, "source"),
                        when(n(it, "when_ms"))
                    )
                })
                .collect();
            format!(
                "registry -- every fact and its source:\n{}\n(\"forget fact N\" \
                 deletes for real; \"correct fact N: ...\" supersedes)",
                lines.join("\n")
            )
        }
        "registry_empty" => "registry is empty -- no facts stored about you.".into(),
        "forgotten" => format!(
            "forgotten for real: {} -- the row is deleted, not hidden.",
            s(a, "content")
        ),
        "forget_missing" => format!("no fact #{} to forget.", n(a, "n")),
        "corrected" => format!(
            "corrected: \"{}\" -> \"{}\" (the old fact is kept as superseded -- \
             history stays inspectable)",
            s(a, "old"),
            s(a, "new")
        ),
        "correct_missing" => format!("no fact #{} to correct.", n(a, "n")),

        // ---- admin ----
        "invite_created" => format!(
            "one-time invite link (works once, member role, their own sealed \
             cell):\n{}",
            s(a, "link")
        ),
        "telegram_bind_code" => format!(
            "telegram bind code: {}\nsend this code to your bot in telegram within \
             10 minutes and that chat becomes yours. (the bot needs \
             TELEGRAM_BOT_TOKEN in the environment.)",
            s(a, "code")
        ),
        "owner_only" => format!("only the owner can do that ({}).", s(a, "what")),
        "not_available_here" => format!("that isn't available in this context ({}).", s(a, "what")),

        // ---- degradation, honestly ----
        "brain_offline" => "my model brain is offline (no OPENROUTER_API_KEY in the \
             environment). the deterministic floor still works -- time, reminders, \
             memory, registry. try \"help\"."
            .into(),
        "provider_failure" => format!(
            "i'm having trouble thinking right now ({}). the deterministic floor \
             still works -- try \"help\".",
            s(a, "error")
        ),
        "search_offline" => "web search is off (no SERPER_API_KEY in the environment); \
             i can only answer from what i already know."
            .into(),
        "search_empty" => "the web search came back empty for that.".into(),
        "search_failed" => format!("the web search failed: {}", s(a, "error")),
        "research_failed" => format!(
            "i found sources but couldn't think about them ({}).",
            s(a, "error")
        ),
        "sources" => {
            let lines: Vec<String> = items(a)
                .iter()
                .enumerate()
                .map(|(i, it)| format!("{}. {}", i + 1, s(it, "link")))
                .collect();
            format!("sources:\n{}", lines.join("\n"))
        }
        "ultra_quota" => "(daily ultra budget exhausted -- answered on super; the \
             receipt names it.)"
            .into(),

        // ---- turn outcomes ----
        "done" => "done.".into(),
        "partial_note" => "(some of that failed -- the receipt has the honest detail.)".into(),
        "failed_note" => format!(
            "(nothing was changed; the receipt records this as {}.)",
            s(a, "status")
        ),
        "fallback" => "i can't do that one. try \"help\" to see what i can do.".into(),

        other => format!("[{other}]"),
    }
}

/// The action record: language-free by construction. A tool name and a
/// tick. If a sentence above claims something changed and no line appears
/// here, the discrepancy is visible without reading either.
fn action_block(actions: &[ActionRecord]) -> String {
    if actions.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = actions
        .iter()
        .map(|a| {
            let mark = if a.status == "failed" { "✗" } else { "✓" };
            format!("{mark} {}", a.tool)
        })
        .collect();
    format!("\n\n― {}", lines.join("  "))
}

/// Does this tag mean English? BCP 47, so `en`, `en-GB`, `EN` all count.
fn is_english(lang: &str) -> bool {
    let l = lang.trim().to_lowercase();
    l.is_empty() || l == "en" || l.starts_with("en-")
}

impl Renderer for Speak {
    fn render(&self, lang: &str, parts: &[ReplyPart], actions: &[ActionRecord]) -> String {
        let mut rendered: Vec<String> = Vec::with_capacity(parts.len());
        // model prose is already in their language; only kernel structure
        // needs saying
        let needs_saying: Vec<&Rendering> = parts
            .iter()
            .filter_map(|p| match p {
                ReplyPart::Say(r) => Some(r),
                ReplyPart::Prose(_) => None,
            })
            .collect();

        let said: Vec<String> = if is_english(lang) || needs_saying.is_empty() {
            needs_saying.iter().map(|r| english(r)).collect()
        } else {
            match self.say_in(lang, &needs_saying) {
                Some(v) => v,
                // no model, or the call failed: English is a worse answer
                // than their own language, and a much better one than silence
                None => needs_saying.iter().map(|r| english(r)).collect(),
            }
        };

        let mut said = said.into_iter();
        for p in parts {
            match p {
                ReplyPart::Say(_) => {
                    if let Some(text) = said.next() {
                        rendered.push(text)
                    }
                }
                ReplyPart::Prose(text) => rendered.push(text.clone()),
            }
        }
        format!("{}{}", rendered.join("\n"), action_block(actions))
    }
}

impl Speak {
    /// One call, all of the turn's system messages at once.
    fn say_in(&self, lang: &str, parts: &[&Rendering]) -> Option<Vec<String>> {
        let gw = self.gateway.as_ref()?;
        let payload: Vec<serde_json::Value> = parts
            .iter()
            .map(|r| serde_json::json!({ "english": english(r), "data": r.slots }))
            .collect();
        let system = format!(
            "you render a robot's system messages into the language tagged \
             `{lang}` (BCP 47).\n\
             for each item you get the ENGLISH original and the structured DATA \
             behind it. write the same message in {lang}, keeping every number, \
             time, name and quoted fragment exactly as it is in the data -- \
             especially text the person themselves wrote, which must appear \
             unchanged.\n\
             keep the robot's voice: lower-case, warm, brief, no corporate \
             padding. render dates and times naturally for that language.\n\
             return ONLY a JSON array of strings, one per item, in order."
        );
        let messages = [
            hub::gateway::Msg {
                role: "system",
                content: system,
            },
            hub::gateway::Msg {
                role: "user",
                content: serde_json::to_string(&payload).ok()?,
            },
        ];
        // temperature 0: this is transcription, not composition
        let out = gw
            .chat_at(hub::gateway::Role::Answer, &messages, None, 1200, 0.0)
            .map_err(|e| tracing::warn!("rendering call failed: {e}"))
            .ok()?;
        let arr: Vec<String> = serde_json::from_value(hub::verdicts::salvage_array(&out.content)?)
            .ok()?;
        (arr.len() == parts.len()).then_some(arr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every id the code can emit must have a template. A missing one is a
    /// bracketed placeholder, which is visible but wrong.
    #[test]
    fn every_rendering_id_has_an_english_original() {
        for (id, slots) in [
            ("time_now", serde_json::json!({"at_ms": 0})),
            ("self_meta", serde_json::json!({})),
            ("help", serde_json::json!({})),
            ("reminder_created", serde_json::json!({"when_ms": 0, "about": "x"})),
            ("reminder_list", serde_json::json!({"items": []})),
            ("reminder_list_empty", serde_json::json!({})),
            ("reminder_cancelled", serde_json::json!({"about": "x"})),
            ("reminder_nothing_to_cancel", serde_json::json!({})),
            ("reminder_fired", serde_json::json!({"about": "x"})),
            ("remembered", serde_json::json!({"content": "x"})),
            ("recall", serde_json::json!({"items": []})),
            ("recall_empty", serde_json::json!({})),
            ("registry", serde_json::json!({"items": []})),
            ("registry_empty", serde_json::json!({})),
            ("forgotten", serde_json::json!({"content": "x"})),
            ("forget_missing", serde_json::json!({"n": 1})),
            ("corrected", serde_json::json!({"old": "a", "new": "b"})),
            ("correct_missing", serde_json::json!({"n": 1})),
            ("invite_created", serde_json::json!({"link": "http://x"})),
            ("telegram_bind_code", serde_json::json!({"code": "123456"})),
            ("owner_only", serde_json::json!({"what": "x"})),
            ("not_available_here", serde_json::json!({"what": "x"})),
            ("brain_offline", serde_json::json!({})),
            ("provider_failure", serde_json::json!({"error": "x"})),
            ("search_offline", serde_json::json!({})),
            ("search_empty", serde_json::json!({})),
            ("search_failed", serde_json::json!({"error": "x"})),
            ("research_failed", serde_json::json!({"error": "x"})),
            ("sources", serde_json::json!({"items": []})),
            ("ultra_quota", serde_json::json!({})),
            ("done", serde_json::json!({})),
            ("partial_note", serde_json::json!({})),
            ("failed_note", serde_json::json!({"status": "failed"})),
            ("fallback", serde_json::json!({})),
        ] {
            let text = english(&Rendering::new(id, slots));
            assert!(!text.is_empty(), "{id}");
            assert!(!text.starts_with('['), "{id} has no template");
        }
    }

    /// With no model, a non-English turn still gets an answer -- in English.
    /// Degraded, never silent.
    #[test]
    fn without_a_model_other_languages_fall_back_to_english() {
        let sp = Speak::offline();
        let parts = [ReplyPart::Say(Rendering::bare("reminder_list_empty"))];
        assert_eq!(sp.render("ru", &parts, &[]), "no active reminders.");
    }

    /// Model prose is passed through, never re-rendered.
    #[test]
    fn model_prose_is_not_touched() {
        let sp = Speak::offline();
        let parts = [ReplyPart::Prose("сейчас 10 утра".into())];
        assert_eq!(sp.render("ru", &parts, &[]), "сейчас 10 утра");
    }

    /// The action record is the receipts law made visible, and it is not in
    /// any language.
    #[test]
    fn the_action_record_carries_no_language() {
        let sp = Speak::offline();
        let parts = [ReplyPart::Prose("i've set that for you".into())];
        let acted = [ActionRecord {
            tool: "reminder.create".into(),
            status: "verified".into(),
            detail: "created".into(),
        }];
        let out = sp.render("ja", &parts, &acted);
        assert!(out.contains("✓ reminder.create"), "{out}");

        // and the same claim with nothing behind it shows no record at all
        let bare = sp.render("ja", &parts, &[]);
        assert!(!bare.contains("reminder.create"), "{bare}");
    }
}
