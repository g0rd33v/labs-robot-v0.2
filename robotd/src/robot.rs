//! RobotCore: the composition of the organs behind the `surfaces::Robot`
//! trait, plus the capability router wiring Prism's steps to Mind's stores.
//! Enforces the boundary-log law on every turn.

use anyhow::anyhow;
use prism::lifecycle::format_fire_at;
use prism::{CapabilityRouter, Envelope, Evidence, Outcome, PrismError, TurnDeps};
use prism::verdict::FallbackVerdict;
use rusqlite::Connection;
use std::sync::Mutex;
use trust::boundary::{self, Crossing, Direction};

/// The MVP capability set, executed against the member's own cell.
/// Idempotent per intent, as the router contract requires.
pub struct Capabilities;

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
                Ok(Outcome {
                    step_id: String::new(),
                    ok: true,
                    evidence: vec![evidence(&rem.id, &trust::ids::sha256_hex(about.as_bytes()))],
                    detail: format!(
                        "done -- i'll remind you at {}: {}",
                        format_fire_at(rem.fire_at),
                        rem.about
                    ),
                })
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
                Ok(Outcome {
                    step_id: String::new(),
                    ok: true,
                    evidence: vec![evidence("reminder.list", "")],
                    detail,
                })
            }
            "reminder.cancel_last" => {
                match mind::reminders::cancel_latest(cell)
                    .map_err(|e| PrismError::Capability(e.to_string()))?
                {
                    Some(rem) => Ok(Outcome {
                        step_id: String::new(),
                        ok: true,
                        evidence: vec![evidence(&rem.id, "")],
                        detail: format!("cancelled: {}", rem.about),
                    }),
                    None => Ok(Outcome {
                        step_id: String::new(),
                        ok: true,
                        evidence: vec![evidence("reminder.cancel_last", "")],
                        detail: "nothing to cancel -- no active reminders.".into(),
                    }),
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
            mind::record_message(&cell, "in", "chat", &text)?;
            let env = Envelope {
                surface: "chat".into(),
                principal_id: self.owner_principal,
                modality: "text".into(),
                content: text,
                ts: trust::ids::ts_ms(),
                device_trust: "owner-session".into(),
            };
            let deps = TurnDeps {
                router: &Capabilities,
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

    fn envelope(content: &str) -> Envelope {
        Envelope {
            surface: "chat".into(),
            principal_id: 1,
            modality: "text".into(),
            content: content.into(),
            ts: trust::ids::ts_ms(),
            device_trust: "owner-session".into(),
        }
    }

    fn live_deps<'a>() -> TurnDeps<'a> {
        TurnDeps {
            router: &Capabilities,
            verdicts: &FallbackVerdict,
            crash: None,
        }
    }

    /// M2 GATE PART 1: no utterance without a terminal receipt, proven for
    /// every turn class (spec M1 gate, carried by mission M2).
    #[test]
    fn every_turn_ends_with_a_terminal_receipt() {
        let (cell, path) = file_cell("receipts");
        for text in [
            "what time is it?",
            "who are you",
            "help",
            "remind me in 10 minutes to stretch",
            "my reminders",
            "cancel reminder",
            "tell me a joke", // fallback path
        ] {
            let out = prism::run_turn(&cell, &envelope(text), &live_deps()).unwrap();
            assert!(out.receipt.status.is_terminal(), "{text}");
            assert!(!out.reply.is_empty(), "{text}");
            let kinds = prism::journal::kinds_for_intent(&cell, &out.intent_id).unwrap();
            assert_eq!(kinds.first().map(String::as_str), Some("intent_open"), "{text}");
            assert_eq!(kinds.last().map(String::as_str), Some("intent_close"), "{text}");
            assert!(kinds.iter().any(|k| k == "receipt"), "{text}");
        }
        let _ = std::fs::remove_file(path);
    }

    /// M2 GATE PART 2: the kill-test. Murder the turn at every journal
    /// boundary; replay must finish it with exactly-once effects and a
    /// terminal receipt. Replay twice to prove idempotency of replay itself.
    #[test]
    fn kill_test_crash_at_every_boundary_replays_exactly_once() {
        for point in CRASH_POINTS {
            let (cell, path) = file_cell(point);
            let crash = |p: &str| p == point;
            let deps = TurnDeps {
                router: &Capabilities,
                verdicts: &FallbackVerdict,
                crash: Some(&crash),
            };
            let err = prism::run_turn(
                &cell,
                &envelope("remind me in 10 minutes to call mark"),
                &deps,
            )
            .unwrap_err();
            assert!(
                matches!(err, PrismError::SimulatedCrash(_)),
                "{point}: expected crash"
            );

            // the process "restarts": replay resumes every open intent
            let s1 = prism::replay::resume_incomplete(&cell, &Capabilities).unwrap();
            assert_eq!(s1.resumed + s1.closed_failed, 1, "{point}");
            // replay is idempotent: a second boot finds nothing to do
            let s2 = prism::replay::resume_incomplete(&cell, &Capabilities).unwrap();
            assert_eq!(s2.resumed + s2.closed_failed, 0, "{point}");

            // exactly-once: crash before the decision was journaled means the
            // effect never ran (honest failed receipt); at or after the
            // decision, replay completes it -- exactly one reminder, never two
            let expected = if point == "after_open" { 0 } else { 1 };
            assert_eq!(
                mind::reminders::count_active(&cell).unwrap(),
                expected,
                "{point}"
            );

            // and always: intent closed, receipt terminal
            assert!(prism::journal::open_intents(&cell).unwrap().is_empty(), "{point}");
            let intent_id: String = cell
                .query_row(
                    "SELECT intent_id FROM journal WHERE kind='intent_open' LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let receipt = prism::receipts::get(&cell, &intent_id).unwrap().unwrap();
            assert!(receipt.status.is_terminal(), "{point}");
            let _ = std::fs::remove_file(path);
        }
    }

    /// Crash between the material effect and its outcome journal row: the
    /// riskiest window. Re-execution must land on the same reminder (UNIQUE
    /// intent_id), proving double-write is structurally impossible.
    #[test]
    fn re_execution_after_crash_is_idempotent() {
        let (cell, path) = file_cell("idem");
        let out = prism::run_turn(
            &cell,
            &envelope("remind me in 5 minutes to breathe"),
            &live_deps(),
        )
        .unwrap();
        // simulate the lost outcome row: execute the same step again directly
        let again = Capabilities
            .execute(
                &cell,
                "reminder.create",
                &serde_json::json!({"fire_at": trust::ids::ts_ms() + 300_000, "about": "breathe"}),
                &out.intent_id,
            )
            .unwrap();
        assert!(again.ok);
        assert_eq!(mind::reminders::count_active(&cell).unwrap(), 1);
        let _ = std::fs::remove_file(path);
    }

    /// The reply effect is deduped in the outbox: same intent, same payload,
    /// one effect row (Q11: double-send structurally impossible).
    #[test]
    fn reply_effect_is_unique_per_intent() {
        let (cell, path) = file_cell("outbox");
        let out = prism::run_turn(&cell, &envelope("what time is it"), &live_deps()).unwrap();
        let (again_id, fresh) =
            prism::outbox::enqueue(&cell, &out.intent_id, "surface:chat", &out.reply).unwrap();
        assert!(!fresh);
        assert_eq!(again_id, out.reply_effect_id);
        let _ = std::fs::remove_file(path);
    }
}
