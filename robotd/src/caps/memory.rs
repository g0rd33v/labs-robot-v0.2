//! Memory capabilities: the Q28 `memory.*` set plus the Registry (sec 4b).
//! Every write carries a source pointer -- law #5 is a schema constraint,
//! and `Ctx::source_msg` refuses rather than store an unsourced fact.

use super::{attested, mind_err, note_evidence, row_evidence, typed, Capability, Ctx};
use prism::types::{Effect, Outcome, Rendering};
use prism::PrismError;
use serde::Deserialize;

/// The sentence every content argument in this file carries. Stored
/// knowledge must be what the person actually said: a translated fact makes
/// provenance point at words they never wrote (law #5).
const VERBATIM: &str = "Copied VERBATIM from the person's message, in their own \
language. Never translate, correct, or paraphrase it -- this text is stored and \
shown back to them as their own words.";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentArgs {
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryArgs {
    query: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexArgs {
    index: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorrectArgs {
    index: u32,
    content: String,
}

fn nonempty(s: &str, field: &str) -> Result<(), String> {
    if s.trim().is_empty() {
        return Err(format!("{field} is empty"));
    }
    Ok(())
}

fn one_based(index: u32) -> Result<(), String> {
    if index == 0 {
        return Err("index is 1-based; 0 is not a fact".into());
    }
    Ok(())
}

pub struct Remember;

impl Capability for Remember {
    fn name(&self) -> &'static str {
        "memory.remember"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Store a durable fact the person has told you about themselves, their \
         preferences, their people, or their circumstances -- something worth \
         recalling weeks from now. Use when they say to remember something, or \
         state a lasting fact about themselves. Do not use for passing chatter."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": VERBATIM }
            },
            "required": ["content"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: ContentArgs = typed(args)?;
        nonempty(&a.content, "content")
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: ContentArgs = typed(args).map_err(PrismError::Capability)?;
        let source = ctx.source_msg()?;
        let emb = ctx.services.passage_embedding(&a.content);
        let fact = ctx.cell.with(|c| {
            mind::facts::remember(c, &a.content, &source, ctx.intent_id, emb.as_deref())
                .map_err(mind_err)
        })?;
        attested(
            row_evidence(&fact.id, &trust::ids::sha256_hex(a.content.as_bytes())),
            "stored a fact with its source",
            Rendering::new("remembered", serde_json::json!({ "content": a.content })),
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
    fn description(&self) -> &'static str {
        "Search stored facts about the person and return the ones that match. \
         Use when they ask what you know or remember about a subject."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The subject to search for, taken from the \
                                    person's own words. Not translated."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let _: QueryArgs = typed(args)?;
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: QueryArgs = typed(args).map_err(PrismError::Capability)?;
        let emb = if a.query.trim().is_empty() {
            None
        } else {
            ctx.services.query_embedding(&a.query)
        };
        let found = ctx
            .cell
            .with(|c| mind::facts::recall(c, &a.query, emb.as_deref(), 5).map_err(mind_err))?;
        let say = if found.is_empty() {
            Rendering::bare("recall_empty")
        } else {
            let items: Vec<serde_json::Value> = found
                .iter()
                .map(|f| serde_json::json!({ "fact": f.content, "when_ms": f.created_at }))
                .collect();
            Rendering::new("recall", serde_json::json!({ "items": items }))
        };
        attested(
            note_evidence("memory.recall"),
            format!("recalled {} facts", found.len()),
            say,
        )
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
    fn description(&self) -> &'static str {
        "Show every stored fact together with the exact message it came from \
         and when it was learned, each numbered. Use when the person wants to \
         audit what is held about them, or needs the numbers before correcting \
         or forgetting something."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let listed = ctx
            .cell
            .with(|c| mind::facts::registry_list(c, 50).map_err(mind_err))?;
        let say = if listed.is_empty() {
            Rendering::bare("registry_empty")
        } else {
            let items: Vec<serde_json::Value> = listed
                .iter()
                .map(|(f, src, ts)| {
                    let snippet: String = src.chars().take(48).collect();
                    serde_json::json!({
                        "fact": f.content, "source": snippet, "when_ms": ts
                    })
                })
                .collect();
            Rendering::new("registry", serde_json::json!({ "items": items }))
        };
        attested(
            note_evidence("registry.list"),
            format!("listed {} facts with their sources", listed.len()),
            say,
        )
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
    fn description(&self) -> &'static str {
        "Permanently delete a stored fact by its number in the registry. The \
         row is destroyed, not hidden, and it cannot be recovered. The person \
         must have said which numbered fact to forget -- if they have not seen \
         the registry, show it first instead of guessing."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "index": {
                    "type": "integer", "minimum": 1,
                    "description": "The fact's number as shown in the registry."
                }
            },
            "required": ["index"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: IndexArgs = typed(args)?;
        one_based(a.index)
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: IndexArgs = typed(args).map_err(PrismError::Capability)?;
        let index = a.index as usize;
        match ctx
            .cell
            .with(|c| mind::facts::forget_by_index(c, index, ctx.intent_id).map_err(mind_err))?
        {
            Some(content) => attested(
                note_evidence("memory.forget"),
                "permanently deleted a fact and its supersession chain",
                Rendering::new("forgotten", serde_json::json!({ "content": content })),
            ),
            None => attested(
                note_evidence("memory.forget"),
                format!("no fact #{index} exists; nothing was deleted"),
                Rendering::new("forget_missing", serde_json::json!({ "n": index })),
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
    fn description(&self) -> &'static str {
        "Replace a stored fact with a corrected version, by its number in the \
         registry. The old fact is kept as superseded so the history stays \
         inspectable. Use when the person says something you stored is wrong."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "index": {
                    "type": "integer", "minimum": 1,
                    "description": "The fact's number as shown in the registry."
                },
                "content": { "type": "string", "description": VERBATIM }
            },
            "required": ["index", "content"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: CorrectArgs = typed(args)?;
        one_based(a.index)?;
        nonempty(&a.content, "content")
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: CorrectArgs = typed(args).map_err(PrismError::Capability)?;
        let index = a.index as usize;
        let source = ctx.source_msg()?;
        let emb = ctx.services.passage_embedding(&a.content);
        match ctx.cell.with(|c| {
            mind::facts::correct_by_index(
                c,
                index,
                &a.content,
                &source,
                ctx.intent_id,
                emb.as_deref(),
            )
            .map_err(mind_err)
        })? {
            Some((old_content, new)) => attested(
                row_evidence(&new.id, ""),
                "superseded a fact with a corrected version",
                Rendering::new(
                    "corrected",
                    serde_json::json!({ "old": old_content, "new": new.content }),
                ),
            ),
            None => attested(
                note_evidence("memory.correct"),
                format!("no fact #{index} exists; nothing was corrected"),
                Rendering::new("correct_missing", serde_json::json!({ "n": index })),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_typechecked_before_anything_runs() {
        assert!(Remember
            .validate(&serde_json::json!({"content": "я пью зелёный чай"}))
            .is_ok());
        assert!(Remember.validate(&serde_json::json!({"content": ""})).is_err());
        assert!(Remember
            .validate(&serde_json::json!({"contents": "typo"}))
            .is_err());

        assert!(Forget.validate(&serde_json::json!({"index": 2})).is_ok());
        assert!(Forget.validate(&serde_json::json!({"index": 0})).is_err());
        assert!(Forget.validate(&serde_json::json!({"index": -1})).is_err());
        assert!(Forget.validate(&serde_json::json!({"index": "two"})).is_err());
    }
}
