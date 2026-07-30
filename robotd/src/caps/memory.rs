//! Memory capabilities: the Q28 `memory.*` set plus the Registry (sec 4b).
//! Every write carries a source pointer -- law #5 is a schema constraint,
//! and `Ctx::source_msg` refuses rather than store an unsourced fact.

use super::{attested, mind_err, note_evidence, row_evidence, Capability, Ctx};
use prism::types::{Effect, Outcome};
use prism::PrismError;

pub struct Remember;

impl Capability for Remember {
    fn name(&self) -> &'static str {
        "memory.remember"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let content = args["content"]
            .as_str()
            .ok_or_else(|| PrismError::Capability("memory.remember: content missing".into()))?;
        let source = ctx.source_msg()?;
        let emb = ctx.services.passage_embedding(content);
        let fact = ctx.cell.with(|c| {
            mind::facts::remember(c, content, &source, ctx.intent_id, emb.as_deref())
                .map_err(mind_err)
        })?;
        attested(
            row_evidence(&fact.id, &trust::ids::sha256_hex(content.as_bytes())),
            format!(
                "{}\n{}",
                ctx.say("remembered", &[("content", content)]),
                ctx.say("remembered_note", &[])
            ),
        )
    }
}

pub struct Recall;

impl Capability for Recall {
    fn name(&self) -> &'static str {
        "memory.recall"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let query = args["query"].as_str().unwrap_or("");
        let emb = if query.trim().is_empty() {
            None
        } else {
            ctx.services.query_embedding(query)
        };
        let found = ctx
            .cell
            .with(|c| mind::facts::recall(c, query, emb.as_deref(), 5).map_err(mind_err))?;
        let detail = if found.is_empty() {
            ctx.say("recall_empty", &[])
        } else {
            let lines: Vec<String> = found
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    ctx.say(
                        "recall_line",
                        &[
                            ("n", &(i + 1).to_string()),
                            ("fact", &f.content),
                            ("when", &ctx.pack().datetime_ms("learned_at", f.created_at)),
                        ],
                    )
                })
                .collect();
            format!("{}\n{}", ctx.say("recall_header", &[]), lines.join("\n"))
        };
        attested(note_evidence("memory.recall"), detail)
    }
}

pub struct RegistryList;

impl Capability for RegistryList {
    fn name(&self) -> &'static str {
        "registry.list"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let listed = ctx
            .cell
            .with(|c| mind::facts::registry_list(c, 50).map_err(mind_err))?;
        let detail = if listed.is_empty() {
            ctx.say("registry_empty", &[])
        } else {
            let lines: Vec<String> = listed
                .iter()
                .enumerate()
                .map(|(i, (f, src, ts))| {
                    let snippet: String = src.chars().take(48).collect();
                    ctx.say(
                        "registry_line",
                        &[
                            ("n", &(i + 1).to_string()),
                            ("fact", &f.content),
                            ("source", &snippet),
                            ("when", &ctx.pack().datetime_ms("learned_at", *ts)),
                        ],
                    )
                })
                .collect();
            format!(
                "{}\n{}\n{}",
                ctx.say("registry_header", &[]),
                lines.join("\n"),
                ctx.say("registry_note", &[])
            )
        };
        attested(note_evidence("registry.list"), detail)
    }
}

pub struct Forget;

impl Capability for Forget {
    fn name(&self) -> &'static str {
        "memory.forget"
    }
    /// Deletion is real: the row and its whole supersession chain go. That
    /// is irreversible, and the plan must say so.
    fn effect(&self) -> Effect {
        Effect::Irreversible
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let index = args["index"].as_u64().unwrap_or(0) as usize;
        match ctx.cell.with(|c| {
            mind::facts::forget_by_index(c, index, ctx.intent_id).map_err(mind_err)
        })? {
            Some(content) => attested(
                note_evidence("memory.forget"),
                ctx.say("forgotten", &[("content", &content)]),
            ),
            None => attested(
                note_evidence("memory.forget"),
                ctx.say("forget_missing", &[("n", &index.to_string())]),
            ),
        }
    }
}

pub struct Correct;

impl Capability for Correct {
    fn name(&self) -> &'static str {
        "memory.correct"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let index = args["index"].as_u64().unwrap_or(0) as usize;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| PrismError::Capability("memory.correct: content missing".into()))?;
        let source = ctx.source_msg()?;
        let emb = ctx.services.passage_embedding(content);
        match ctx.cell.with(|c| {
            mind::facts::correct_by_index(c, index, content, &source, ctx.intent_id, emb.as_deref())
                .map_err(mind_err)
        })? {
            Some((old_content, new)) => attested(
                row_evidence(&new.id, ""),
                ctx.say("corrected", &[("old", &old_content), ("new", &new.content)]),
            ),
            None => attested(
                note_evidence("memory.correct"),
                ctx.say("correct_missing", &[("n", &index.to_string())]),
            ),
        }
    }
}
