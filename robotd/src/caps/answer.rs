//! The model answer path: memory-grounded, escalation-aware.
//!
//! Its output is an *utterance*, never an attestation -- the receipt records
//! that a model spoke and which one, not what it said (arch sec 3).

use super::{failed, note_evidence, spoke, Capability, Ctx};
use crate::prompts::persona;
use chrono::Local;
use hub::gateway::{Msg, Role};
use prism::types::{Effect, Outcome, Rendering, Tier};
use prism::PrismError;
use rusqlite::{params, Connection};

fn role_for(tier: Tier) -> Role {
    match tier {
        Tier::Fast => Role::Answer,
        Tier::Super => Role::Super,
        Tier::Ultra => Role::Ultra,
    }
}

/// Per-day ultra counter in cell_meta (Q18).
fn bump_ultra(cell: &Connection, cap: u32) -> bool {
    if cap == 0 {
        return false;
    }
    let key = format!("ultra:{}", Local::now().format("%Y-%m-%d"));
    let used: u32 = cell
        .query_row("SELECT value FROM cell_meta WHERE key = ?1", params![key], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if used >= cap {
        return false;
    }
    let _ = cell.execute(
        "INSERT INTO cell_meta(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, (used + 1).to_string()],
    );
    true
}

pub struct ModelAnswer;

impl Capability for ModelAnswer {
    fn name(&self) -> &'static str {
        "answer.model"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Answer from knowledge and memory, with no external action. This is \
         where a turn goes when no tool fits."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "tier":  { "type": "string", "enum": ["fast", "super", "ultra"] }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    /// NOT offered to the model: it is the escape hatch, and a model given
    /// the choice between doing the work and declining will sometimes
    /// decline.
    fn exposed(&self) -> bool {
        false
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let query = args["query"].as_str().unwrap_or("");
        let Some(gw) = &ctx.services.gateway else {
            return super::declined("answer.model", Rendering::bare("brain_offline"));
        };

        // escalation: the verdict's tier merged with deterministic rules
        let vtier: Tier = serde_json::from_value(args["tier"].clone()).unwrap_or(Tier::Fast);
        let mut tier = hub::escalation::merge(vtier, hub::escalation::classify(query));
        let mut quota_note = String::new();
        if tier == Tier::Ultra {
            let allowed = ctx
                .cell
                .with(|c| Ok(bump_ultra(c, ctx.policy.ultra_daily_cap)))?;
            if !allowed {
                tier = Tier::Super;
                quota_note = "\n\n(daily ultra budget exhausted -- answered on \
                              super; the receipt names it.)"
                    .to_string();
            }
        }

        // context compiler-lite: persona + recalled facts + recent turns,
        // all read under short locks so the model call below runs with the
        // person's cell free
        let emb = ctx.services.query_embedding(query);
        let recalled = ctx
            .cell
            .with(|c| Ok(mind::facts::recall(c, query, emb.as_deref(), 5)))?
            .unwrap_or_default();
        // arch sec 6 eligibility filtering / sec 7 data classes: this is the
        // one place a person's own knowledge is put in front of an external
        // model, so it is the one place that has to ask whether it may be.
        // A class that says "not off this machine" is not advice.
        let before = recalled.len();
        let facts: Vec<_> = recalled
            .into_iter()
            .filter(|f| {
                trust::classes::DataClass::parse(&f.class)
                    .unwrap_or_default()
                    .may_leave_the_machine()
            })
            .collect();
        let withheld = before - facts.len();
        if withheld > 0 {
            tracing::debug!("{withheld} fact(s) withheld from model context by class");
        }
        let mut system = persona();
        // standing rules (sec 4.6) shape the answer as they shape proposals;
        // the block is pre-fenced and pre-filtered by class
        if let Some(rules) = ctx
            .cell
            .with(|c| mind::instructions::context_block(c).map_err(super::mind_err))?
        {
            system.push_str("\n\n");
            system.push_str(&rules);
        }
        if !facts.is_empty() {
            system.push_str(
                "\n\nfacts you remember about this person (each has provenance in \
                 your registry):",
            );
            for f in &facts {
                system.push_str(&format!("\n- {}", f.content));
            }
        }
        let mut messages = vec![Msg {
            role: "system",
            content: system,
        }];
        let mut history = ctx
            .cell
            .with(|c| Ok(mind::recent_messages(c, 10)))?
            .unwrap_or_default();
        // the inbound message is already recorded; it goes in as the live
        // user turn rather than twice
        if history.last().map(|(d, c)| d == "in" && c == query) == Some(true) {
            history.pop();
        }
        for (dir, content) in history {
            messages.push(Msg {
                role: if dir == "in" { "user" } else { "assistant" },
                content,
            });
        }
        messages.push(Msg {
            role: "user",
            content: query.into(),
        });

        match gw.chat(role_for(tier), &messages, None, 1200) {
            Ok(out) => spoke(
                vec![super::model_evidence(&out.model, &out.content)],
                format!("{}{quota_note}", out.content),
            ),
            Err(e) => failed(
                note_evidence("provider-failure"),
                format!("the model call failed: {e}"),
                Rendering::new(
                    "provider_failure",
                    serde_json::json!({ "error": e.to_string() }),
                ),
            ),
        }
    }
}

#[cfg(test)]
mod class_tests {
    use trust::classes::DataClass;

    /// Item 3's gate, at the boundary it exists to guard.
    ///
    /// `answer.model` is the one place a person's own knowledge is put in
    /// front of an external model. This asserts the filter it applies is
    /// the class rule and nothing softer -- if this list and
    /// `may_leave_the_machine` ever disagree, the disagreement is the bug.
    #[test]
    fn only_classes_cleared_to_leave_reach_model_context() {
        let cleared: Vec<DataClass> = [
            DataClass::Public,
            DataClass::OwnerPrivate,
            DataClass::Sensitive,
            DataClass::Derived,
            DataClass::OrgConfidential,
            DataClass::Restricted,
            DataClass::LocalOnly,
            DataClass::Credential,
        ]
        .into_iter()
        .filter(|c| c.may_leave_the_machine())
        .collect();

        assert!(!cleared.contains(&DataClass::Restricted));
        assert!(!cleared.contains(&DataClass::LocalOnly));
        assert!(!cleared.contains(&DataClass::Credential));
        assert_eq!(cleared.len(), 5);
    }

    /// An unrecognised class must not be treated as permissive. Corrupt or
    /// future values fall to the default, which is protective.
    #[test]
    fn an_unknown_class_is_not_a_free_pass() {
        let c = DataClass::parse("something_from_a_newer_version").unwrap_or_default();
        assert_eq!(c, DataClass::OwnerPrivate);
        assert!(DataClass::parse("").is_none());
    }
}
