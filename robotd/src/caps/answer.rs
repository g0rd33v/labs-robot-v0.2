//! The model answer path: memory-grounded, escalation-aware.
//!
//! Its output is an *utterance*, never an attestation -- the receipt records
//! that a model spoke and which one, not what it said (arch sec 3).

use super::{failed, note_evidence, spoke, Capability, Ctx};
use crate::prompts::persona;
use chrono::Local;
use hub::gateway::{Msg, Role};
use prism::types::{Effect, Outcome, Tier};
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
            return spoke(note_evidence("brain-offline"), ctx.say("brain_offline", &[]));
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
                quota_note = ctx.say("ultra_quota_note", &[]);
            }
        }

        // context compiler-lite: persona + recalled facts + recent turns,
        // all read under short locks so the model call below runs with the
        // person's cell free
        let emb = ctx.services.query_embedding(query);
        let facts = ctx
            .cell
            .with(|c| Ok(mind::facts::recall(c, query, emb.as_deref(), 5)))?
            .unwrap_or_default();
        let mut system = persona();
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
                ctx.say("provider_failure", &[("error", &e.to_string())]),
            ),
        }
    }
}
