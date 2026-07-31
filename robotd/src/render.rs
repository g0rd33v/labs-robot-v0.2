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
use prism::lifecycle::{Rendered, Renderer};
use prism::types::{ActionRecord, Rendering, ReplyPart};
use std::sync::Arc;

pub struct Speak {
    pub gateway: Option<Arc<hub::ModelGateway>>,
    /// Soul's instruction for this turn, or `None` at the default dial with
    /// no role -- in which case English uses its templates, which are free,
    /// instant and work with no network. Asking for a different voice is
    /// what buys the model call that shaping needs.
    pub voice: Option<String>,
}

impl Speak {
    pub fn offline() -> Self {
        Self {
            gateway: None,
            voice: None,
        }
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

        // ---- confirmation (sec 6b) ----
        "confirm_irreversible" => format!(
            "that would permanently delete something ({}), and i inferred it \
             rather than being told outright -- say yes and i'll do it, or no \
             and i won't. nothing has happened yet.",
            s(a, "tool")
        ),
        "confirmation_declined" => "alright -- nothing was deleted.".into(),
        "confirmation_stale" => "that yes came too late to use -- nothing was \
             deleted. ask me again if you still want it gone."
            .into(),

        // ---- soul (sec 5 / Q27) ----
        "soul_dial" => {
            let on = a.get("evolution").and_then(|v| v.as_bool()).unwrap_or(true);
            let lines: Vec<String> = items(a)
                .iter()
                .map(|it| {
                    let v = n(it, "value");
                    let pin = if it.get("pinned").and_then(|x| x.as_bool()) == Some(true) {
                        "  (pinned)"
                    } else {
                        ""
                    };
                    format!(
                        "{:<11} {:>3}   {}..{}{}\n              0 = {} · 100 = {}",
                        s(it, "dimension"),
                        v,
                        n(it, "floor"),
                        n(it, "ceiling"),
                        pin,
                        s(it, "low"),
                        s(it, "high")
                    )
                })
                .collect();
            format!(
                "i'm speaking as: {}\n\nhow i'm set to speak:\n{}\n\nself-adjustment: {}\n(say \"be blunter\", \
                 \"be warmer\", \"shorter\" to move one; \"pin brevity\" to freeze it; \
                 \"stop adjusting yourself\" to switch adaptation off)",
                s(a, "stance"),
                lines.join("\n"),
                if on { "on" } else { "off" }
            )
        }
        "soul_set" => format!(
            "{} is now {}. that changes how i word things, not what i tell you.",
            s(a, "dimension"),
            n(a, "value")
        ),
        "soul_bounds" => format!(
            "{} may now move between {} and {} (it's at {}).",
            s(a, "dimension"),
            n(a, "floor"),
            n(a, "ceiling"),
            n(a, "value")
        ),
        "soul_pinned" => format!(
            "{} is pinned at {} -- i won't move it, and neither will anything else.",
            s(a, "dimension"),
            n(a, "value")
        ),
        "soul_evolution" => {
            if a.get("on").and_then(|v| v.as_bool()) == Some(true) {
                "self-adjustment is on. i'll adapt how i speak within the bounds \
                 you've set, and every change is recorded and reversible."
                    .into()
            } else {
                "self-adjustment is off. the dial stays exactly where it is until \
                 you move it."
                    .into()
            }
        }
        "soul_stance" => format!(
            "alright -- i'm speaking as {} now. that changes how i say things, \
             not what's true.",
            s(a, "stance")
        ),
        "soul_refused" => format!("i can't: {}", s(a, "why")),

        // ---- turn outcomes ----
        "done" => "done.".into(),
        "partial_note" => "(some of that failed -- the receipt has the honest detail.)".into(),
        "failed_note" => format!(
            "(nothing was changed; the receipt records this as {}.)",
            s(a, "status")
        ),
        "fallback" => "i can't do that one. try \"help\" to see what i can do.".into(),
        "unsupported_note" => "[note from my own checks: the line above reads like \
             something changed, but this turn performed no such action. nothing was \
             created, saved or deleted, and the receipt records that.]"
            .into(),
        "approval_needed" => format!(
            "that one needs your say-so before i run it ({}). reply yes and \
             i'll do it, or no and i won't -- it'll wait as long as it takes, \
             even if i restart. nothing has happened yet.",
            s(a, "capability")
        ),
        "approval_declined" => format!(
            "alright -- i didn't run {}. nothing changed.",
            s(a, "capability")
        ),
        "approval_none" => "nothing is waiting for your approval.".into(),
        "approval_list" => {
            let lines: Vec<String> = items(a)
                .iter()
                .enumerate()
                .map(|(i, it)| format!("{}. {}", i + 1, s(it, "capability")))
                .collect();
            format!(
                "waiting for you:\n{}\n(reply yes or no)",
                lines.join("\n")
            )
        }
        "grant_refused" => format!(
            "i didn't do that -- {}. nothing was changed. ask me again and \
             i'll take it from the top.",
            s(a, "why")
        ),
        "ops_notice" => "(operational notice)".into(),
        "media_stored" => format!("stored: {}", s(a, "filename")),

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

/// Renderings that report the robot's own configuration.
///
/// These are **never re-voiced**, whatever stance is set. Two reasons, and
/// the second is the important one:
///
/// 1. `/soul` promises an answer from stored state with no model call.
///    Shaping it broke that -- measured at 22 seconds for a query that is
///    three SQL reads.
/// 2. If the mentor persona could reword the dial readout, you would be
///    reading the instrument through the thing you are trying to inspect.
///    A gauge that changes its wording depending on the setting it is
///    reporting is not a gauge.
///
/// They are still TRANSLATED for a person who does not read English --
/// translating a readout is not the same as re-voicing it.
fn is_control_surface(id: &str) -> bool {
    matches!(
        id,
        "soul_dial"
            | "soul_set"
            | "soul_bounds"
            | "soul_pinned"
            | "soul_evolution"
            | "soul_stance"
            | "soul_refused"
    )
}

/// Does this tag mean English? BCP 47, so `en`, `en-GB`, `EN` all count.
fn is_english(lang: &str) -> bool {
    let l = lang.trim().to_lowercase();
    l.is_empty() || l == "en" || l.starts_with("en-")
}

impl Renderer for Speak {
    fn render(&self, lang: &str, parts: &[ReplyPart], actions: &[ActionRecord]) -> Rendered {
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

        // English is rendered here, on this machine, from these templates:
        // nothing about the person leaves to produce a sentence.
        let mut disclosed: Vec<String> = vec![];
        // English at the default dial uses templates: free, instant, offline.
        // A moved dial or a stance is what buys the model call -- except for
        // the control surface, which is an instrument reading and reads the
        // same however the robot is set.
        let all_control = needs_saying.iter().all(|r| is_control_surface(&r.id));
        let voice = if all_control {
            None
        } else {
            self.voice.as_deref()
        };
        let shape_english = voice.is_some();
        let said: Vec<String> = if needs_saying.is_empty()
            || (is_english(lang) && !shape_english)
        {
            needs_saying.iter().map(|r| english(r)).collect()
        } else {
            match self.say_in(lang, &needs_saying, voice) {
                Some(v) => {
                    // it worked, which means their slots -- facts, reminders,
                    // registry sources -- went to a model as the material
                    disclosed = needs_saying.iter().map(|r| r.id.clone()).collect();
                    v
                }
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
        Rendered {
            text: format!("{}{}", rendered.join("\n"), action_block(actions)),
            disclosed,
        }
    }
}

impl Speak {
    /// One call, all of the turn's system messages at once.
    fn say_in(
        &self,
        lang: &str,
        parts: &[&Rendering],
        voice: Option<&str>,
    ) -> Option<Vec<String>> {
        let gw = self.gateway.as_ref()?;
        let payload: Vec<serde_json::Value> = parts
            .iter()
            .map(|r| serde_json::json!({ "english": english(r), "data": r.slots }))
            .collect();
        let voice = voice.unwrap_or(
            "keep the robot's voice: lower-case, warm, brief, no corporate padding.",
        );
        let system = format!(
            "you re-voice a robot's system messages into the language tagged \
             `{lang}` (BCP 47). if that tag is english, keep them in english and \
             only re-voice them.\n\
             for each item you get the ENGLISH original and the structured DATA \
             behind it. write the same message, keeping every number, time, name \
             and quoted fragment exactly as it is in the data -- especially text \
             the person themselves wrote, which must appear unchanged. you are \
             changing HOW it is said and nothing about what it says.\n\
             render dates and times naturally for that language.\n\
             {voice}\n\
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
            ("confirm_irreversible", serde_json::json!({"tool": "memory.forget"})),
            ("confirmation_declined", serde_json::json!({})),
            ("confirmation_stale", serde_json::json!({})),
            ("unsupported_note", serde_json::json!({})),
            ("soul_dial", serde_json::json!({"items": [], "evolution": true, "stance": "its own"})),
            ("soul_set", serde_json::json!({"dimension": "warmth", "value": 60})),
            ("soul_bounds", serde_json::json!({"dimension": "warmth", "floor": 0, "ceiling": 100, "value": 55})),
            ("soul_pinned", serde_json::json!({"dimension": "warmth", "value": 55})),
            ("soul_evolution", serde_json::json!({"on": true})),
            ("soul_stance", serde_json::json!({"stance": "mentor"})),
            ("soul_refused", serde_json::json!({"why": "x"})),
            ("approval_needed", serde_json::json!({"capability": "member.invite"})),
            ("approval_declined", serde_json::json!({"capability": "member.invite"})),
            ("approval_none", serde_json::json!({})),
            ("approval_list", serde_json::json!({"items": []})),
            ("grant_refused", serde_json::json!({"why": "x"})),
            ("ops_notice", serde_json::json!({})),
            ("media_stored", serde_json::json!({"filename": "x.png"})),
        ] {
            let text = english(&Rendering::new(id, slots));
            assert!(!text.is_empty(), "{id}");
            // exact, not a prefix guess: a real template may legitimately
            // begin with a bracket, and one of them does
            assert_ne!(text, format!("[{id}]"), "{id} has no template");
        }
    }

    /// The hand-written list above can drift from what the code actually
    /// emits. This scans every crate for literal rendering ids at their
    /// construction sites and demands a template for each, so a new
    /// capability cannot ship a reply that renders as a bare `[id]`.
    ///
    /// (The scan looks for the constructor followed by a string literal.
    /// This comment deliberately does not spell that pattern out, because
    /// it would then find itself.)
    #[test]
    fn every_id_the_code_emits_has_a_template() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let mut ids: Vec<String> = vec![];
        let mut stack = vec![root];
        while let Some(p) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&p) else { continue };
            for e in entries.flatten() {
                let path = e.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if path.is_dir() && !matches!(name, "target" | ".git" | "data" | "demo") {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    let src = std::fs::read_to_string(&path).unwrap_or_default();
                    for marker in ["Rendering::new(\"", "Rendering::bare(\""] {
                        let mut rest = src.as_str();
                        while let Some(i) = rest.find(marker) {
                            rest = &rest[i + marker.len()..];
                            if let Some(end) = rest.find('"') {
                                ids.push(rest[..end].to_string());
                            }
                        }
                    }
                }
            }
        }
        ids.sort();
        ids.dedup();
        assert!(ids.len() > 20, "the scan found almost nothing: {ids:?}");
        let missing: Vec<&String> = ids
            .iter()
            .filter(|id| english(&Rendering::bare(id)) == format!("[{id}]"))
            .collect();
        assert!(missing.is_empty(), "no english template for: {missing:?}");
    }

    /// With no model, a non-English turn still gets an answer -- in English.
    /// Degraded, never silent.
    #[test]
    fn without_a_model_other_languages_fall_back_to_english() {
        let sp = Speak::offline();
        let parts = [ReplyPart::Say(Rendering::bare("reminder_list_empty"))];
        let out = sp.render("ru", &parts, &[]);
        assert_eq!(out.text, "no active reminders.");
        assert!(out.disclosed.is_empty(), "english templates disclose nothing");
    }

    /// Model prose is passed through, never re-rendered.
    #[test]
    fn model_prose_is_not_touched() {
        let sp = Speak::offline();
        let parts = [ReplyPart::Prose("сейчас 10 утра".into())];
        assert_eq!(sp.render("ru", &parts, &[]).text, "сейчас 10 утра");
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
        let out = sp.render("ja", &parts, &acted).text;
        assert!(out.contains("✓ reminder.create"), "{out}");

        // and the same claim with nothing behind it shows no record at all
        let bare = sp.render("ja", &parts, &[]).text;
        assert!(!bare.contains("reminder.create"), "{bare}");
    }
}

#[cfg(test)]
mod soul_tests {
    use super::*;
    use prism::types::Rendering;
    use soul::dial::{Dial, Dimension, Setting};
    use soul::stance::Stance;

    fn dial_at(pairs: &[(Dimension, i64)]) -> Dial {
        Dial {
            settings: Dimension::ALL
                .into_iter()
                .map(|d| Setting {
                    dimension: d,
                    value: pairs
                        .iter()
                        .find(|(x, _)| *x == d)
                        .map(|(_, v)| *v)
                        .unwrap_or(d.default_value()),
                    floor: 0,
                    ceiling: 100,
                })
                .collect(),
            evolution: true,
        }
    }

    /// S2's gate, the half that can be checked without a network: the dial
    /// must reach the renderer and change what it asks for, and it must not
    /// touch the data.
    ///
    /// The live half -- that opposite settings produce visibly different
    /// wording with identical slots -- needs a model and runs in
    /// `robotd eval --live`.
    #[test]
    fn the_dial_changes_the_ask_and_never_the_data() {
        let neutral = Speak {
            gateway: None,
            voice: soul::express::shape(&dial_at(&[]), None),
        };
        assert!(
            neutral.voice.is_none(),
            "a default dial with no stance must not ask for shaping -- that is \
             what keeps english free and offline"
        );

        let blunt = soul::express::shape(
            &dial_at(&[(Dimension::Directness, 100), (Dimension::Brevity, 100)]),
            None,
        )
        .unwrap();
        let mentor = soul::express::shape(&dial_at(&[]), Some(&Stance::Mentor)).unwrap();
        assert_ne!(blunt, mentor);

        // whatever the voice, the SLOTS are untouched: the renderer receives
        // the same structure and Soul cannot reach into it
        let r = Rendering::new(
            "reminder_created",
            serde_json::json!({ "when_ms": 1_700_000_000_000i64, "about": "call mark" }),
        );
        let plain = english(&r);
        assert!(plain.contains("call mark"));
        // and with a voice set but no gateway, it falls back to that same
        // english rather than inventing anything
        let shaped = Speak {
            gateway: None,
            voice: Some(blunt),
        };
        let out = shaped.render("en", &[ReplyPart::Say(r)], &[]);
        assert_eq!(out.text, plain, "no model, no shaping -- and no drift");
        assert!(out.disclosed.is_empty());
    }

    /// A stance is a costume, and the fence saying so travels with it every
    /// time -- the person writing "be a pirate" is not thinking about
    /// receipts.
    #[test]
    fn every_stance_carries_its_fence() {
        for st in [
            Stance::Twin,
            Stance::Friend,
            Stance::Mentor,
            Stance::Character("a laconic ship's engineer".into()),
        ] {
            let v = soul::express::shape(&dial_at(&[]), Some(&st)).unwrap();
            assert!(v.contains("nothing about what is true"), "{st:?}");
            assert!(v.contains("answer honestly"), "{st:?}");
        }
    }
}

#[cfg(test)]
mod control_surface_tests {
    use super::*;
    use prism::types::Rendering;

    /// `/soul` promises an answer from stored state with no model call.
    /// Once a stance was set, the shaping rule sent even that through a
    /// model -- 22 seconds for three SQL reads. A gauge whose wording
    /// changes with the setting it reports is not a gauge.
    #[test]
    fn the_control_surface_is_never_revoiced() {
        let shaped = Speak {
            gateway: None,
            voice: Some("you are their mentor".into()),
        };
        // with no gateway the model path falls back to english anyway, so the
        // observable property is the decision itself
        for id in [
            "soul_dial",
            "soul_set",
            "soul_bounds",
            "soul_pinned",
            "soul_evolution",
            "soul_stance",
            "soul_refused",
        ] {
            assert!(is_control_surface(id), "{id} must not be re-voiced");
        }
        // a reply about the world is not the control surface
        for id in ["reminder_created", "recall", "registry", "help"] {
            assert!(!is_control_surface(id), "{id} is ordinary speech");
        }

        let out = shaped.render(
            "en",
            &[ReplyPart::Say(Rendering::new(
                "soul_stance",
                serde_json::json!({ "stance": "mentor" }),
            ))],
            &[],
        );
        assert!(out.text.contains("mentor"));
        assert!(out.disclosed.is_empty(), "and nothing left the machine");
    }
}
