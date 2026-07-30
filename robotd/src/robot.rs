//! RobotCore: the composition of the organs behind the `surfaces::Robot`
//! trait, plus the capability router wiring Prism's steps to Mind's stores.
//! Enforces the boundary-log law on every turn.

use anyhow::anyhow;
use chrono::{Local, TimeZone};
use prism::lifecycle::format_fire_at;
use prism::verdict::FallbackVerdict;
use prism::{CapabilityRouter, Envelope, Evidence, Outcome, PrismError, TurnDeps};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use trust::boundary::{self, Crossing, Direction};

/// The MVP capability set, executed against the member's own cell.
/// Idempotent per intent, as the router contract requires.
#[derive(Default, Clone)]
pub struct Capabilities {
    /// The local embedding seat (hub). Optional: without it the vector door
    /// stays closed and recall degrades to FTS + recency.
    pub embedder: Option<Arc<hub::Embedder>>,
}

impl Capabilities {
    /// Provenance anchor (law #5): the source message id journaled at
    /// intent_open. Knowledge-writing capabilities refuse to run without it.
    fn source_msg_of(cell: &Connection, intent_id: &str) -> Result<String, PrismError> {
        let payload = prism::journal::payload_of(cell, intent_id, "intent_open")?
            .ok_or_else(|| PrismError::Capability("no intent_open journaled".into()))?;
        let v: serde_json::Value = serde_json::from_str(&payload)?;
        v["source_msg_id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| {
                PrismError::Capability(
                    "no source message journaled; refusing to store an unsourced fact (law #5)"
                        .into(),
                )
            })
    }

    fn passage_embedding(&self, text: &str) -> Option<Vec<f32>> {
        self.embedder
            .as_ref()
            .and_then(|e| e.embed_passage(text).ok())
    }

    fn query_embedding(&self, text: &str) -> Option<Vec<f32>> {
        self.embedder
            .as_ref()
            .and_then(|e| e.embed_query(text).ok())
    }
}

fn learned_at(ts_ms: i64) -> String {
    match Local.timestamp_millis_opt(ts_ms).earliest() {
        Some(dt) => dt.format("%d %b %H:%M").to_string(),
        None => "unknown time".into(),
    }
}

impl CapabilityRouter for Capabilities {
    fn execute(
        &self,
        cell: &Connection,
        capability: &str,
        args: &serde_json::Value,
        intent_id: &str,
    ) -> Result<Outcome, PrismError> {
        let evidence = |id: &str, hash: &str| Evidence {
            kind: "row".into(),
            provider: "cell".into(),
            external_id: id.into(),
            hash: hash.into(),
            ts: trust::ids::ts_ms(),
        };
        let ok = |evidence: Vec<Evidence>, detail: String| {
            Ok(Outcome {
                step_id: String::new(),
                ok: true,
                evidence,
                detail,
            })
        };
        match capability {
            "reminder.create" => {
                let fire_at = args["fire_at"].as_i64().ok_or_else(|| {
                    PrismError::Capability("reminder.create: fire_at missing".into())
                })?;
                let about = args["about"].as_str().ok_or_else(|| {
                    PrismError::Capability("reminder.create: about missing".into())
                })?;
                let rem = mind::reminders::create(cell, intent_id, fire_at, about)
                    .map_err(|e| PrismError::Capability(e.to_string()))?;
                ok(
                    vec![evidence(&rem.id, &trust::ids::sha256_hex(about.as_bytes()))],
                    format!(
                        "done -- i'll remind you at {}: {}",
                        format_fire_at(rem.fire_at),
                        rem.about
                    ),
                )
            }
            "reminder.list" => {
                let all = mind::reminders::list_active(cell)
                    .map_err(|e| PrismError::Capability(e.to_string()))?;
                let detail = if all.is_empty() {
                    "no active reminders.".to_string()
                } else {
                    let lines: Vec<String> = all
                        .iter()
                        .enumerate()
                        .map(|(i, r)| {
                            format!("{}. {} -- {}", i + 1, format_fire_at(r.fire_at), r.about)
                        })
                        .collect();
                    format!("your reminders:\n{}", lines.join("\n"))
                };
                ok(vec![evidence("reminder.list", "")], detail)
            }
            "reminder.cancel_last" => {
                match mind::reminders::cancel_latest(cell)
                    .map_err(|e| PrismError::Capability(e.to_string()))?
                {
                    Some(rem) => ok(
                        vec![evidence(&rem.id, "")],
                        format!("cancelled: {}", rem.about),
                    ),
                    None => ok(
                        vec![evidence("reminder.cancel_last", "")],
                        "nothing to cancel -- no active reminders.".into(),
                    ),
                }
            }
            "memory.remember" => {
                let content = args["content"].as_str().ok_or_else(|| {
                    PrismError::Capability("memory.remember: content missing".into())
                })?;
                let source = Self::source_msg_of(cell, intent_id)?;
                let emb = self.passage_embedding(content);
                let fact = mind::facts::remember(cell, content, &source, intent_id, emb.as_deref())
                    .map_err(|e| PrismError::Capability(e.to_string()))?;
                ok(
                    vec![evidence(&fact.id, &trust::ids::sha256_hex(content.as_bytes()))],
                    format!(
                        "remembered: {content}\n(source kept -- see \"my facts\"; \
                         \"forget fact N\" deletes for real)"
                    ),
                )
            }
            "memory.recall" => {
                let query = args["query"].as_str().unwrap_or("");
                let emb = if query.trim().is_empty() {
                    None
                } else {
                    self.query_embedding(query)
                };
                let found = mind::facts::recall(cell, query, emb.as_deref(), 5)
                    .map_err(|e| PrismError::Capability(e.to_string()))?;
                let detail = if found.is_empty() {
                    "nothing in memory yet -- tell me \"remember ...\" and i'll keep it, \
                     with its source."
                        .to_string()
                } else {
                    let lines: Vec<String> = found
                        .iter()
                        .enumerate()
                        .map(|(i, f)| {
                            format!("{}. {} (learned {})", i + 1, f.content, learned_at(f.created_at))
                        })
                        .collect();
                    format!("here's what i remember:\n{}", lines.join("\n"))
                };
                ok(vec![evidence("memory.recall", "")], detail)
            }
            "registry.list" => {
                let listed = mind::facts::registry_list(cell, 50)
                    .map_err(|e| PrismError::Capability(e.to_string()))?;
                let detail = if listed.is_empty() {
                    "registry is empty -- no facts stored about you.".to_string()
                } else {
                    let lines: Vec<String> = listed
                        .iter()
                        .enumerate()
                        .map(|(i, (f, src, ts))| {
                            let snippet: String = src.chars().take(48).collect();
                            format!(
                                "{}. {} -- from your words: \"{}\" ({})",
                                i + 1,
                                f.content,
                                snippet,
                                learned_at(*ts)
                            )
                        })
                        .collect();
                    format!(
                        "registry -- every fact and its source:\n{}\n\
                         (\"forget fact N\" deletes for real; \"correct fact N: ...\" supersedes)",
                        lines.join("\n")
                    )
                };
                ok(vec![evidence("registry.list", "")], detail)
            }
            "memory.forget" => {
                let index = args["index"].as_u64().unwrap_or(0) as usize;
                match mind::facts::forget_by_index(cell, index, intent_id)
                    .map_err(|e| PrismError::Capability(e.to_string()))?
                {
                    Some(content) => ok(
                        vec![evidence("memory.forget", "")],
                        format!("forgotten for real: {content} -- the row is deleted, not hidden."),
                    ),
                    None => ok(
                        vec![evidence("memory.forget", "")],
                        format!("no fact #{index} to forget."),
                    ),
                }
            }
            "memory.correct" => {
                let index = args["index"].as_u64().unwrap_or(0) as usize;
                let content = args["content"].as_str().ok_or_else(|| {
                    PrismError::Capability("memory.correct: content missing".into())
                })?;
                let source = Self::source_msg_of(cell, intent_id)?;
                let emb = self.passage_embedding(content);
                match mind::facts::correct_by_index(
                    cell,
                    index,
                    content,
                    &source,
                    intent_id,
                    emb.as_deref(),
                )
                .map_err(|e| PrismError::Capability(e.to_string()))?
                {
                    Some((old_content, new)) => ok(
                        vec![evidence(&new.id, "")],
                        format!(
                            "corrected: \"{old_content}\" -> \"{}\" \
                             (the old fact is kept as superseded -- history stays inspectable)",
                            new.content
                        ),
                    ),
                    None => ok(
                        vec![evidence("memory.correct", "")],
                        format!("no fact #{index} to correct."),
                    ),
                }
            }
            other => Err(PrismError::Capability(format!("unknown capability: {other}"))),
        }
    }
}

pub struct RobotCore {
    pub owner_principal: i64,
    pub core: Mutex<Connection>,
    pub owner_cell: Mutex<Connection>,
    pub embedder: Option<Arc<hub::Embedder>>,
}

fn chat_crossing(direction: Direction, payload_hash: String, size: i64) -> Crossing {
    Crossing {
        direction,
        channel: "chat".into(),
        counterparty: "local-web".into(),
        purpose: "conversation".into(),
        categories: "message".into(),
        payload_hash,
        size,
        // the local owner session; remote/unknown surfaces get `untrusted`
        trust_tag: "owner".into(),
    }
}

impl surfaces::Robot for RobotCore {
    fn handle_message(&self, text: String) -> anyhow::Result<String> {
        // 1. boundary log: the inbound crossing, before anything else (law #3)
        {
            let core = self
                .core
                .lock()
                .map_err(|_| anyhow!("core lock poisoned"))?;
            boundary::append(
                &core,
                &chat_crossing(
                    Direction::In,
                    trust::ids::sha256_hex(text.as_bytes()),
                    text.len() as i64,
                ),
            )?;
        }

        // 2. the governed turn, inside the owner's encrypted cell
        let reply = {
            let cell = self
                .owner_cell
                .lock()
                .map_err(|_| anyhow!("cell lock poisoned"))?;
            let msg_id = mind::record_message(&cell, "in", "chat", &text)?;
            let env = Envelope {
                surface: "chat".into(),
                principal_id: self.owner_principal,
                modality: "text".into(),
                content: text,
                ts: trust::ids::ts_ms(),
                device_trust: "owner-session".into(),
                source_msg_id: Some(msg_id),
            };
            let router = Capabilities {
                embedder: self.embedder.clone(),
            };
            let deps = TurnDeps {
                router: &router,
                verdicts: &FallbackVerdict,
                crash: None,
            };
            let out = prism::run_turn(&cell, &env, &deps)?;
            // the reply effect leaves through the outbox: sent -> confirmed
            // (local synchronous delivery; Telegram's message_id path is M5)
            prism::outbox::mark(&cell, &out.reply_effect_id, "sent", None)?;
            prism::outbox::mark(&cell, &out.reply_effect_id, "confirmed", None)?;
            mind::record_message(&cell, "out", "chat", &out.reply)?;
            out.reply
        };

        // 3. boundary log: the outbound crossing, before the reply leaves
        {
            let core = self
                .core
                .lock()
                .map_err(|_| anyhow!("core lock poisoned"))?;
            boundary::append(
                &core,
                &chat_crossing(
                    Direction::Out,
                    trust::ids::sha256_hex(reply.as_bytes()),
                    reply.len() as i64,
                ),
            )?;
        }

        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism::CRASH_POINTS;

    fn file_cell(name: &str) -> (Connection, std::path::PathBuf) {
        mind::install_vec();
        let path = std::env::temp_dir().join(format!(
            "killtest-{}-{name}.db",
            trust::ids::random_hex(6)
        ));
        let key = trust::keys::KeyChain::new_dek();
        let conn = trust::cells::open_encrypted(&path, &key).unwrap();
        prism::init_cell_schema(&conn).unwrap();
        mind::init_cell_schema(&conn).unwrap();
        (conn, path)
    }

    /// An envelope whose content is first recorded as a message, so
    /// provenance-requiring capabilities have their anchor.
    fn envelope(cell: &Connection, content: &str) -> Envelope {
        let msg_id = mind::record_message(cell, "in", "chat", content).unwrap();
        Envelope {
            surface: "chat".into(),
            principal_id: 1,
            modality: "text".into(),
            content: content.into(),
            ts: trust::ids::ts_ms(),
            device_trust: "owner-session".into(),
            source_msg_id: Some(msg_id),
        }
    }

    fn live_deps(router: &Capabilities) -> TurnDeps<'_> {
        TurnDeps {
            router,
            verdicts: &FallbackVerdict,
            crash: None,
        }
    }

    /// M3 GATE PART 1: every turn class ends with a terminal receipt --
    /// now including the memory set.
    #[test]
    fn every_turn_ends_with_a_terminal_receipt() {
        let (cell, path) = file_cell("receipts");
        let router = Capabilities::default();
        for text in [
            "what time is it?",
            "who are you",
            "help",
            "remind me in 10 minutes to stretch",
            "my reminders",
            "cancel reminder",
            "remember that i drink green tea",
            "what do you remember about tea",
            "my facts",
            "correct fact 1: i drink black tea",
            "forget fact 1",
            "tell me a joke", // fallback path
        ] {
            let out = prism::run_turn(&cell, &envelope(&cell, text), &live_deps(&router)).unwrap();
            assert!(out.receipt.status.is_terminal(), "{text}");
            assert!(!out.reply.is_empty(), "{text}");
            let kinds = prism::journal::kinds_for_intent(&cell, &out.intent_id).unwrap();
            assert_eq!(kinds.first().map(String::as_str), Some("intent_open"), "{text}");
            assert_eq!(kinds.last().map(String::as_str), Some("intent_close"), "{text}");
            assert!(kinds.iter().any(|k| k == "receipt"), "{text}");
        }
        let _ = std::fs::remove_file(path);
    }

    /// M3 GATE PART 2: the memory law walk -- remember with provenance,
    /// recall finds it, registry shows the source, forget deletes for real.
    #[test]
    fn memory_walk_remember_recall_registry_forget() {
        let (cell, path) = file_cell("memory");
        let router = Capabilities::default();
        let run = |text: &str| {
            prism::run_turn(&cell, &envelope(&cell, text), &live_deps(&router))
                .unwrap()
                .reply
        };
        let r = run("remember that the demo is on friday");
        assert!(r.contains("remembered: the demo is on friday"), "{r}");

        let r = run("what do you remember about the demo");
        assert!(r.contains("the demo is on friday"), "{r}");

        let r = run("my facts");
        assert!(r.contains("the demo is on friday"), "{r}");
        assert!(r.contains("from your words"), "{r}"); // source chain visible

        let r = run("correct fact 1: the demo moved to monday");
        assert!(r.contains("superseded"), "{r}");
        let r = run("what do you remember about the demo");
        assert!(r.contains("moved to monday"), "{r}");
        assert!(!r.contains("on friday"), "{r}"); // superseded is out of recall

        let r = run("forget fact 1");
        assert!(r.contains("forgotten for real"), "{r}");
        assert_eq!(mind::facts::count_active(&cell).unwrap(), 0);
        let _ = std::fs::remove_file(path);
    }

    /// A fact may never exist without its source message (law #5): a turn
    /// whose envelope has no recorded message must fail the remember step
    /// and say so honestly.
    #[test]
    fn remember_without_provenance_fails_honestly() {
        let (cell, path) = file_cell("noprov");
        let router = Capabilities::default();
        let env = Envelope {
            surface: "chat".into(),
            principal_id: 1,
            modality: "text".into(),
            content: "remember that x is y".into(),
            ts: trust::ids::ts_ms(),
            device_trust: "owner-session".into(),
            source_msg_id: None, // no anchor
        };
        let err = prism::run_turn(&cell, &env, &live_deps(&router));
        assert!(err.is_err(), "unsourced remember must not succeed");
        assert_eq!(mind::facts::count_active(&cell).unwrap(), 0);
        let _ = std::fs::remove_file(path);
    }

    /// M2's kill-test, carried forward: crash at every boundary, replay
    /// exactly-once -- for the reminder path and the remember path.
    #[test]
    fn kill_test_crash_at_every_boundary_replays_exactly_once() {
        type EffectCount = fn(&Connection) -> i64;
        let cases: [(&str, EffectCount); 2] = [
            ("remind me in 10 minutes to call mark", |c| {
                mind::reminders::count_active(c).unwrap()
            }),
            ("remember that mark prefers mornings", |c| {
                mind::facts::count_active(c).unwrap()
            }),
        ];
        for (text, check) in cases {
            for point in CRASH_POINTS {
                let (cell, path) = file_cell(point);
                let router = Capabilities::default();
                let crash = |p: &str| p == point;
                let deps = TurnDeps {
                    router: &router,
                    verdicts: &FallbackVerdict,
                    crash: Some(&crash),
                };
                let err = prism::run_turn(&cell, &envelope(&cell, text), &deps).unwrap_err();
                assert!(
                    matches!(err, PrismError::SimulatedCrash(_)),
                    "{text} @ {point}: expected crash"
                );

                let s1 = prism::replay::resume_incomplete(&cell, &router).unwrap();
                assert_eq!(s1.resumed + s1.closed_failed, 1, "{text} @ {point}");
                let s2 = prism::replay::resume_incomplete(&cell, &router).unwrap();
                assert_eq!(s2.resumed + s2.closed_failed, 0, "{text} @ {point}");

                let expected = if point == "after_open" { 0 } else { 1 };
                assert_eq!(check(&cell), expected, "{text} @ {point}");

                assert!(
                    prism::journal::open_intents(&cell).unwrap().is_empty(),
                    "{text} @ {point}"
                );
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// The reply effect is deduped in the outbox (Q11).
    #[test]
    fn reply_effect_is_unique_per_intent() {
        let (cell, path) = file_cell("outbox");
        let router = Capabilities::default();
        let out =
            prism::run_turn(&cell, &envelope(&cell, "what time is it"), &live_deps(&router))
                .unwrap();
        let (again_id, fresh) =
            prism::outbox::enqueue(&cell, &out.intent_id, "surface:chat", &out.reply).unwrap();
        assert!(!fresh);
        assert_eq!(again_id, out.reply_effect_id);
        let _ = std::fs::remove_file(path);
    }
}
