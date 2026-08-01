//! The person's standing rules (§4.6, Registry category 2).
//!
//! What a rule IS here: words the models read, verbatim, in a fenced block
//! — never code that executes and never authority over the robot's own
//! governance. "Always ask before sending" does not rewire the approval
//! gate (which cannot be narrowed by anything); it tells the models what
//! this person wants, which is what it is.
//!
//! Adding is a `ReversibleWrite` even though it changes future behavior,
//! because retiring undoes it completely and the history of having had the
//! rule is itself kept.

use super::{attested, mind_err, note_evidence, typed, Capability, Ctx};
use prism::types::{Effect, Outcome, Rendering};
use prism::PrismError;
use serde::Deserialize;

const MAX_RULE_CHARS: usize = 500;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddArgs {
    rule: String,
}

pub struct Add;

impl Capability for Add {
    fn name(&self) -> &'static str {
        "instruction.add"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Keep a standing rule the person gives about how you should behave \
         from now on -- 'always answer in bullet points', 'never schedule \
         anything on sundays', 'ask before spending over 50 euros'. Any \
         message that sets durable future behavior belongs here, whatever \
         language it is in: 'from now on', 'starting today', 'always', \
         'never again' are the signals. Acknowledging such a rule without \
         calling this tool means the rule is NOT saved. Not for facts about \
         the person (memory.remember) or one-off requests."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "rule": {
                    "type": "string",
                    "description": "The rule in the person's own words. Keep \
                                    their phrasing; do not translate or summarise."
                }
            },
            "required": ["rule"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: AddArgs = typed(args)?;
        if a.rule.trim().is_empty() {
            return Err("an empty rule rules nothing".into());
        }
        if a.rule.chars().count() > MAX_RULE_CHARS {
            return Err("that is an essay, not a rule -- keep it under 500 characters".into());
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: AddArgs = typed(args).map_err(PrismError::Capability)?;
        let source = ctx.source_msg()?;
        let it = ctx.cell.with(|c| {
            mind::instructions::add(c, a.rule.trim(), &source, "owner_private").map_err(mind_err)
        })?;
        attested(
            super::row_evidence(&it.id, &it.id),
            format!("stored standing rule {}", it.id),
            Rendering::new("instruction_added", serde_json::json!({ "rule": it.body })),
        )
    }
}

pub struct List;

impl Capability for List {
    fn name(&self) -> &'static str {
        "instruction.list"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Show the standing rules currently in force, numbered. Use when the \
         person asks what rules they have given you, or before changing one."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let rules = ctx
            .cell
            .with(|c| mind::instructions::active(c).map_err(mind_err))?;
        let say = if rules.is_empty() {
            Rendering::bare("instruction_list_empty")
        } else {
            let items: Vec<serde_json::Value> = rules
                .iter()
                .map(|i| serde_json::json!({ "rule": i.body }))
                .collect();
            Rendering::new("instruction_list", serde_json::json!({ "items": items }))
        };
        attested(
            note_evidence("instruction.list"),
            format!("listed {} standing rules", rules.len()),
            say,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviseArgs {
    index: usize,
    rule: String,
}

pub struct Revise;

impl Capability for Revise {
    fn name(&self) -> &'static str {
        "instruction.revise"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Replace standing rule number N with a new wording. The old version \
         is kept as history, never overwritten. Use when the person changes \
         or refines a rule they gave earlier."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "index": {
                    "type": "integer", "minimum": 1,
                    "description": "The rule's number as instruction.list shows it."
                },
                "rule": {
                    "type": "string",
                    "description": "The new wording, in the person's own words."
                }
            },
            "required": ["index", "rule"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: ReviseArgs = typed(args)?;
        if a.index == 0 {
            return Err("rules are numbered from 1".into());
        }
        if a.rule.trim().is_empty() {
            return Err("an empty rule rules nothing".into());
        }
        if a.rule.chars().count() > MAX_RULE_CHARS {
            return Err("keep it under 500 characters".into());
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: ReviseArgs = typed(args).map_err(PrismError::Capability)?;
        let source = ctx.source_msg()?;
        let revised = ctx.cell.with(|c| {
            mind::instructions::revise(c, a.index, a.rule.trim(), &source).map_err(mind_err)
        })?;
        match revised {
            Some((old, new)) => attested(
                super::row_evidence(&new.id, &new.id),
                format!("revised rule {} -> {}", old.id, new.id),
                Rendering::new(
                    "instruction_revised",
                    serde_json::json!({ "old": old.body, "new": new.body }),
                ),
            ),
            None => attested(
                note_evidence("instruction.revise"),
                format!("no rule #{}", a.index),
                Rendering::new("instruction_missing", serde_json::json!({ "n": a.index })),
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetireArgs {
    index: usize,
}

pub struct Retire;

impl Capability for Retire {
    fn name(&self) -> &'static str {
        "instruction.retire"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Stop following standing rule number N. The rule is kept in history \
         and can be brought back -- use when the person says to drop, stop \
         or ignore a rule they gave."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "index": {
                    "type": "integer", "minimum": 1,
                    "description": "The rule's number as instruction.list shows it."
                }
            },
            "required": ["index"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: RetireArgs = typed(args)?;
        if a.index == 0 {
            return Err("rules are numbered from 1".into());
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: RetireArgs = typed(args).map_err(PrismError::Capability)?;
        let gone = ctx
            .cell
            .with(|c| mind::instructions::retire(c, a.index).map_err(mind_err))?;
        match gone {
            Some(it) => attested(
                super::row_evidence(&it.id, &it.id),
                format!("retired rule {}", it.id),
                Rendering::new("instruction_retired", serde_json::json!({ "rule": it.body })),
            ),
            None => attested(
                note_evidence("instruction.retire"),
                format!("no rule #{}", a.index),
                Rendering::new("instruction_missing", serde_json::json!({ "n": a.index })),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_checked_before_anything_changes() {
        assert!(Add.validate(&serde_json::json!({"rule": "answer briefly"})).is_ok());
        assert!(Add.validate(&serde_json::json!({"rule": "  "})).is_err());
        assert!(
            Add.validate(&serde_json::json!({"rule": "x".repeat(600)})).is_err(),
            "an essay is not a rule"
        );
        assert!(Revise
            .validate(&serde_json::json!({"index": 1, "rule": "shorter"}))
            .is_ok());
        assert!(Revise
            .validate(&serde_json::json!({"index": 0, "rule": "shorter"}))
            .is_err());
        assert!(Retire.validate(&serde_json::json!({"index": 2})).is_ok());
        assert!(Retire.validate(&serde_json::json!({"index": 0})).is_err());
    }

    /// Everything here is undoable, and declares itself so.
    #[test]
    fn every_instruction_operation_is_reversible() {
        for e in [Add.effect(), Revise.effect(), Retire.effect()] {
            assert_eq!(e, Effect::ReversibleWrite);
        }
        assert_eq!(List.effect(), Effect::Read);
    }
}
