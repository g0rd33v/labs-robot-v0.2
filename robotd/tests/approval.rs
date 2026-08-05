//! The gate for §14's email connector: **a send parks for approval and
//! cannot execute without it.**
//!
//! Unit tests in `caps::email` assert that `email.send` *declares*
//! `Approval::Required`. That is a claim about one method. This drives the
//! whole path — registry, plan, lifecycle, journal — because the declaration
//! is worthless if `apply_approval_policy` never reads it, or if
//! `finish_planned_intent_with` executes before it checks.
//!
//! The router here counts executions. A test that only inspected replies
//! could pass while the send went out; counting is the only way to state
//! "nothing ran" as a fact rather than an inference.

use prism::types::*;
use prism::{Cell, CapabilityRouter, Envelope, PrismError};
use robotd::caps::Registry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Wraps the real registry and records what actually reached execution.
struct Counting {
    inner: Registry,
    executed: Arc<AtomicUsize>,
}

impl CapabilityRouter for Counting {
    fn execute(
        &self,
        cell: &Cell,
        capability: &str,
        args: &serde_json::Value,
        intent_id: &str,
        lang: &str,
    ) -> Result<Outcome, PrismError> {
        self.executed.fetch_add(1, Ordering::SeqCst);
        self.inner.execute(cell, capability, args, intent_id, lang)
    }
    fn describe(&self, cell: &Cell) -> Vec<ToolDef> {
        self.inner.describe(cell)
    }
    fn approval_for(&self, capability: &str) -> Approval {
        self.inner.approval_for(capability)
    }
    fn validate(&self, tool: &str, args: &serde_json::Value) -> Result<Effect, String> {
        self.inner.validate(tool, args)
    }
}

/// A model that proposes exactly one tool call, whatever it is asked.
struct Proposes(&'static str, serde_json::Value);

impl prism::verdict::VerdictProvider for Proposes {
    fn verdict(&self, _text: &str) -> Verdict {
        Verdict {
            action: VerdictAction::Task,
            ..Default::default()
        }
    }
    fn route(&self, _content: &str, _tools: &[ToolDef], _now: &str, _standing: Option<&str>) -> Routing {
        Routing {
            verdict: Verdict {
                action: VerdictAction::Task,
                ..Default::default()
            },
            call: Some(ToolCall {
                tool: self.0.to_string(),
                args: self.1.clone(),
            }),
        }
    }
}

fn cell() -> Cell {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    prism::init_cell_schema(&conn).unwrap();
    mind::init_cell_schema(&conn).unwrap();
    soul::init_cell_schema(&conn).unwrap();
    Cell::new(conn)
}

fn envelope(cell: &Cell, text: &str) -> Envelope {
    let msg_id = cell
        .with(|c| Ok(mind::record_message(c, "in", "web", text).unwrap()))
        .unwrap();
    Envelope {
        surface: "web".into(),
        principal_id: 1,
        modality: "text".into(),
        content: text.into(),
        ts: trust::ids::ts_ms(),
        device_trust: "session".into(),
        source_msg_id: Some(msg_id),
    }
}

fn send_args() -> serde_json::Value {
    serde_json::json!({
        "to": "someone@example.com",
        "subject": "the quarterly numbers",
        "body": "attached."
    })
}

#[test]
fn a_send_parks_for_approval_and_cannot_execute_without_one() {
    let cell = cell();
    let executed = Arc::new(AtomicUsize::new(0));
    let router = Counting {
        inner: Registry::offline(),
        executed: executed.clone(),
    };
    let verdicts = Proposes("email.send", send_args());
    let speak = robotd::render::Speak::offline();
    let deps = prism::TurnDeps {
        router: &router,
        verdicts: &verdicts,
        renderer: &speak,
        crash: None,
            standing: None,
        on_early: None,
    };

    let env = envelope(&cell, "email someone@example.com the quarterly numbers");
    let out = prism::lifecycle::run_turn(&cell, &env, &deps).unwrap();

    // 1. nothing ran
    assert_eq!(
        executed.load(Ordering::SeqCst),
        0,
        "a send reached execution before anyone approved it"
    );

    // 2. the receipt is non-terminal: the turn is waiting, not finished
    assert_eq!(out.receipt.status, ReceiptStatus::Proposed);

    // 3. the person was told, and told exactly what they are approving.
    //    "yes" to the bare words "email.send" is consent to nothing in
    //    particular, so the recipient and subject have to be in front of
    //    them at the moment they answer.
    for needed in ["send an email", "someone@example.com", "the quarterly numbers"] {
        assert!(
            out.reply.contains(needed),
            "the approval prompt never mentions {needed:?}: {}",
            out.reply
        );
    }

    // 4. it is durably parked -- in the journal, so it survives a restart
    let waiting = prism::approval::waiting(&cell).unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].capability, "email.send");
    assert_eq!(waiting[0].args["to"], "someone@example.com");
    assert_eq!(waiting[0].effect, Effect::Irreversible);

    // 5. replay must NOT resume it. A parked intent is not a stalled one,
    //    and resuming on boot would perform the very thing being gated.
    let resumed = prism::replay::resume_incomplete(&cell, &router, &speak).unwrap();
    assert_eq!(
        executed.load(Ordering::SeqCst),
        0,
        "crash replay executed a step that was waiting for a person ({resumed:?})"
    );
}

#[test]
fn declining_closes_the_intent_and_still_nothing_runs() {
    let cell = cell();
    let executed = Arc::new(AtomicUsize::new(0));
    let router = Counting {
        inner: Registry::offline(),
        executed: executed.clone(),
    };
    let verdicts = Proposes("email.send", send_args());
    let speak = robotd::render::Speak::offline();
    let deps = prism::TurnDeps {
        router: &router,
        verdicts: &verdicts,
        renderer: &speak,
        crash: None,
            standing: None,
        on_early: None,
    };
    let env = envelope(&cell, "send it");
    let out = prism::lifecycle::run_turn(&cell, &env, &deps).unwrap();

    let after = prism::approval::respond(&cell, &out.intent_id, false, &deps)
        .unwrap()
        .expect("a parked intent should answer");
    assert_eq!(executed.load(Ordering::SeqCst), 0, "declined, yet it ran");
    assert_eq!(after.receipt.status, ReceiptStatus::Failed);
    assert!(prism::approval::waiting(&cell).unwrap().is_empty());

    // and the answer cannot be spent twice
    assert!(prism::approval::respond(&cell, &out.intent_id, true, &deps)
        .unwrap()
        .is_none());
    assert_eq!(
        executed.load(Ordering::SeqCst),
        0,
        "a second answer resurrected a declined send"
    );
}

/// Approving is what makes the difference — otherwise the test above would
/// pass on a robot that simply cannot send at all.
#[test]
fn approving_is_what_lets_it_through() {
    let cell = cell();
    let executed = Arc::new(AtomicUsize::new(0));
    let router = Counting {
        inner: Registry::offline(),
        executed: executed.clone(),
    };
    let verdicts = Proposes("email.send", send_args());
    let speak = robotd::render::Speak::offline();
    let deps = prism::TurnDeps {
        router: &router,
        verdicts: &verdicts,
        renderer: &speak,
        crash: None,
            standing: None,
        on_early: None,
    };
    let env = envelope(&cell, "send it");
    let out = prism::lifecycle::run_turn(&cell, &env, &deps).unwrap();
    assert_eq!(executed.load(Ordering::SeqCst), 0);

    prism::approval::respond(&cell, &out.intent_id, true, &deps)
        .unwrap()
        .expect("a parked intent should answer");

    // It reached execution. This registry has no Google client, so the call
    // fails there -- which is the proof: the failure is a connector failure,
    // not an approval one, so the gate is what was holding it and nothing
    // else.
    assert_eq!(
        executed.load(Ordering::SeqCst),
        1,
        "approval did not release the step"
    );
    assert!(prism::approval::waiting(&cell).unwrap().is_empty());
}

/// THE GATE for gap item 9: **the Second Law is a screen — nothing asked
/// for is silently dropped.** Every deferred ask lands in the ledger the
/// moment it is deferred, and every way it can end — approval, decline —
/// writes a reason a person can read. (The reminder and sweeper paths have
/// their own hooks, tested beside them; this drives the approval path end
/// to end because it is the one with the most ways to end.)
#[test]
fn every_deferred_ask_is_in_the_ledger_and_closes_with_a_reason() {
    let cell = cell();
    let executed = Arc::new(AtomicUsize::new(0));
    let router = Counting {
        inner: Registry::offline(),
        executed,
    };
    let verdicts = Proposes("email.send", send_args());
    let speak = robotd::render::Speak::offline();
    let deps = prism::TurnDeps {
        router: &router,
        verdicts: &verdicts,
        renderer: &speak,
        crash: None,
        standing: None,
        on_early: None,
    };

    // hooks live in robotd's orchestration; this test mirrors them the way
    // robot.rs runs them, then asserts the screen a person would see
    let env = envelope(&cell, "отправь письмо анне про демо");
    let out = prism::lifecycle::run_turn(&cell, &env, &deps).unwrap();
    cell.with(|c| {
        mind::commitments::open(
            c,
            &out.intent_id,
            &env.content,
            "approval",
            "waiting",
            env.source_msg_id.as_deref(),
            Some(&out.intent_id),
            None,
        )
        .map_err(|e| prism::PrismError::Capability(e.to_string()))
    })
    .unwrap();

    // the ask is on the screen, verbatim, while it waits
    let owed = cell
        .with(|c| {
            mind::commitments::outstanding(c)
                .map_err(|e| prism::PrismError::Capability(e.to_string()))
        })
        .unwrap();
    assert_eq!(owed.len(), 1);
    assert_eq!(owed[0].what, "отправь письмо анне про демо", "their words, not a summary");

    // declined -> off the owed list, onto the closed list, with the reason
    prism::approval::respond(&cell, &out.intent_id, false, &deps)
        .unwrap()
        .unwrap();
    cell.with(|c| {
        mind::commitments::close(c, &out.intent_id, "declined", "you declined it; nothing ran")
            .map_err(|e| prism::PrismError::Capability(e.to_string()))
    })
    .unwrap();

    let (owed, settled) = cell
        .with(|c| {
            Ok((
                mind::commitments::outstanding(c).unwrap(),
                mind::commitments::recently_closed(c, 5).unwrap(),
            ))
        })
        .unwrap();
    assert!(owed.is_empty(), "answered means no longer owed");
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].closed_why.as_deref(), Some("you declined it; nothing ran"));

    // and the reminder path opens/closes through its own hooks
    cell.with(|c| {
        let r = mind::reminders::create(c, "int_r1", 12345, "проверить почту").unwrap();
        assert!(mind::commitments::is_open(c, &r.id).unwrap(), "created = ledgered");
        mind::reminders::mark_fired(c, &r.id).unwrap();
        assert!(!mind::commitments::is_open(c, &r.id).unwrap());
        // looked up by id, not by ordering: two closes in one millisecond
        // tie on closed_at, and the screen's order is not the assertion
        let closed = mind::commitments::recently_closed(c, 10).unwrap();
        let mine = closed
            .iter()
            .find(|x| x.id == mind::commitments::id_for(&r.id))
            .unwrap();
        assert_eq!(mine.closed_why.as_deref(), Some("fired on time"));
        Ok(())
    })
    .unwrap();
}

/// Config may widen approval requirements and never narrow them. Without
/// this, `approval_required = []` in `robot.toml` would read as "nothing
/// needs approval" and quietly disarm the gate.
#[test]
fn no_configuration_can_switch_the_gate_off() {
    let mut reg = Registry::offline();
    reg.approval_policy = vec![];
    assert_eq!(reg.approval_for("email.send"), Approval::Required);

    // and it can still be widened onto something that did not ask for it
    reg.approval_policy = vec!["calendar.create".into()];
    assert_eq!(reg.approval_for("calendar.create"), Approval::Required);
    assert_eq!(reg.approval_for("email.send"), Approval::Required);
    assert_eq!(reg.approval_for("email.draft"), Approval::Auto);
    assert_eq!(reg.approval_for("calendar.list"), Approval::Auto);
}
