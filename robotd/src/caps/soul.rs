//! The owner's control surface for Soul (Q27: `/soul` in chat).
//!
//! Every one of these answers **from stored state, with no model call**. A
//! robot that had to ask a model why it was speaking a certain way would be
//! guessing at its own reasons, and the answer would be a plausible story
//! rather than the truth. The dial is a row; reading it is a query.

use super::{attested, typed, Capability, Ctx};
use prism::types::{Effect, Outcome, Rendering};
use prism::PrismError;
use serde::Deserialize;
use soul::dial::{self, Dimension};

fn soul_err(e: soul::SoulError) -> PrismError {
    PrismError::Capability(e.to_string())
}

fn dimension_schema(desc: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "string",
        "enum": ["directness", "warmth", "brevity", "initiative", "formality"],
        "description": desc,
    })
}

fn dial_slots_with(d: &dial::Dial, stance: Option<&soul::stance::Stance>) -> serde_json::Value {
    let mut v = dial_slots(d);
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "stance".into(),
            serde_json::json!(stance.map(|s| s.label()).unwrap_or_else(|| "its own".into())),
        );
    }
    v
}

fn dial_slots(d: &dial::Dial) -> serde_json::Value {
    let items: Vec<serde_json::Value> = d
        .settings
        .iter()
        .map(|s| {
            let (low, high) = s.dimension.poles();
            serde_json::json!({
                "dimension": s.dimension.as_str(),
                "value": s.value,
                "floor": s.floor,
                "ceiling": s.ceiling,
                "pinned": s.pinned(),
                "low": low,
                "high": high,
            })
        })
        .collect();
    serde_json::json!({ "items": items, "evolution": d.evolution })
}

pub struct Show;

impl Capability for Show {
    fn name(&self) -> &'static str {
        "soul.show"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Show how this robot is currently set to speak: the five dial \
         dimensions, their values, the bounds the owner has set, which are \
         pinned, and whether the robot is allowed to adjust itself. Use when \
         the person asks why you talk the way you do, what your personality \
         settings are, or to see the dial."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let (d, st) = ctx.cell.with(|c| {
            Ok((
                dial::load(c).map_err(soul_err)?,
                soul::stance::get(c).map_err(soul_err)?,
            ))
        })?;
        attested(
            super::note_evidence("soul.show"),
            "reported the persona dial",
            Rendering::new("soul_dial", dial_slots_with(&d, st.as_ref())),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    dimension: String,
    value: i64,
}

pub struct Set;

impl Capability for Set {
    fn name(&self) -> &'static str {
        "soul.set"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Move one dial dimension to a value from 0 to 100, changing how this \
         robot speaks. Use when the person asks it to be blunter, warmer, \
         shorter, more or less talkative, more or less formal. directness: 0 \
         hedged, 100 blunt. warmth: 0 clinical, 100 affectionate. brevity: 0 \
         expansive, 100 terse. initiative: 0 answers only what was asked, 100 \
         offers and follows up. formality: 0 casual, 100 formal."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "dimension": dimension_schema("Which dimension to move."),
                "value": {
                    "type": "integer", "minimum": 0, "maximum": 100,
                    "description": "Where to put it. Prefer a modest step from \
                                    where it is now over a jump to an extreme."
                }
            },
            "required": ["dimension", "value"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: SetArgs = typed(args)?;
        Dimension::parse(&a.dimension).ok_or_else(|| format!("no dimension {}", a.dimension))?;
        (0..=100)
            .contains(&a.value)
            .then_some(())
            .ok_or_else(|| "a dial runs from 0 to 100".to_string())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: SetArgs = typed(args).map_err(PrismError::Capability)?;
        let d = Dimension::parse(&a.dimension)
            .ok_or_else(|| PrismError::Capability("unknown dimension".into()))?;
        match ctx.cell.with(|c| Ok(dial::set_value(c, d, a.value))) ? {
            Ok(s) => attested(
                super::row_evidence(d.as_str(), ""),
                format!("set {} to {}", d.as_str(), s.value),
                Rendering::new(
                    "soul_set",
                    serde_json::json!({ "dimension": d.as_str(), "value": s.value }),
                ),
            ),
            // a refusal is an honest answer, not a failure of the machine
            Err(why) => super::declined(
                "soul.set",
                Rendering::new(
                    "soul_refused",
                    serde_json::json!({ "why": why.to_string() }),
                ),
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundsArgs {
    dimension: String,
    #[serde(default)]
    floor: Option<i64>,
    #[serde(default)]
    ceiling: Option<i64>,
    #[serde(default)]
    pin: bool,
}

pub struct Bounds;

impl Capability for Bounds {
    fn name(&self) -> &'static str {
        "soul.bounds"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Set the limits the robot may move a dimension within, or pin it so \
         it cannot move at all. Owner only. Use when the person wants to stop \
         the robot changing some aspect of how it speaks, or to fence it into \
         a range. Pinning is how you say 'stop changing this'."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "dimension": dimension_schema("Which dimension to bound."),
                "floor": {"type": "integer", "minimum": 0, "maximum": 100},
                "ceiling": {"type": "integer", "minimum": 0, "maximum": 100},
                "pin": {
                    "type": "boolean",
                    "description": "True to freeze the dimension where it is now, \
                                    ignoring floor and ceiling."
                }
            },
            "required": ["dimension"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: BoundsArgs = typed(args)?;
        Dimension::parse(&a.dimension).ok_or_else(|| format!("no dimension {}", a.dimension))?;
        if !a.pin && a.floor.is_none() && a.ceiling.is_none() {
            return Err("give a floor, a ceiling, or pin".into());
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        // bounds are the owner's alone: adaptation moves values, never the
        // fence around them
        if ctx.principal != ctx.instance.owner_principal {
            return super::declined(
                "soul.bounds",
                Rendering::new(
                    "owner_only",
                    serde_json::json!({ "what": "change how far the dial may move" }),
                ),
            );
        }
        let a: BoundsArgs = typed(args).map_err(PrismError::Capability)?;
        let d = Dimension::parse(&a.dimension)
            .ok_or_else(|| PrismError::Capability("unknown dimension".into()))?;
        let cur = ctx.cell.with(|c| dial::load(c).map_err(soul_err))?.get(d);
        let result = ctx.cell.with(|c| {
            Ok(if a.pin {
                dial::pin(c, d)
            } else {
                dial::set_bounds(
                    c,
                    d,
                    a.floor.unwrap_or(cur.floor),
                    a.ceiling.unwrap_or(cur.ceiling),
                )
            })
        })?;
        match result {
            Ok(s) => attested(
                super::row_evidence(d.as_str(), ""),
                format!("bounded {} to {}..{}", d.as_str(), s.floor, s.ceiling),
                Rendering::new(
                    if s.pinned() { "soul_pinned" } else { "soul_bounds" },
                    serde_json::json!({
                        "dimension": d.as_str(), "value": s.value,
                        "floor": s.floor, "ceiling": s.ceiling,
                    }),
                ),
            ),
            Err(why) => super::declined(
                "soul.bounds",
                Rendering::new(
                    "soul_refused",
                    serde_json::json!({ "why": why.to_string() }),
                ),
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvolutionArgs {
    on: bool,
}

pub struct Evolution;

impl Capability for Evolution {
    fn name(&self) -> &'static str {
        "soul.evolution"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Turn on or off whether this robot may adjust how it speaks over \
         time. Off leaves the dial exactly where it is. Use when the person \
         wants it to stop adapting, or to start again."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "on": {"type": "boolean"} },
            "required": ["on"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let _: EvolutionArgs = typed(args)?;
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: EvolutionArgs = typed(args).map_err(PrismError::Capability)?;
        ctx.cell
            .with(|c| dial::set_evolution(c, a.on).map_err(soul_err))?;
        attested(
            super::note_evidence("soul.evolution"),
            format!("set self-adjustment to {}", if a.on { "on" } else { "off" }),
            Rendering::new("soul_evolution", serde_json::json!({ "on": a.on })),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StanceArgs {
    stance: String,
    #[serde(default)]
    character: Option<String>,
}

pub struct SetStance;

impl Capability for SetStance {
    fn name(&self) -> &'static str {
        "soul.stance"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Set who this robot is to the person. Choosing a stance also moves \
         the whole style dial to match, so reach for this before nudging \
         individual dimensions.\n\
         THREE NAMED STANCES, and if the person names one of these you MUST \
         pass that exact value rather than 'character': 'twin' (speaks the \
         way they do, terse, no ceremony), 'friend' (warm, easy, offers \
         things), 'mentor' (explains, takes initiative).\n\
         'character' is ONLY for a role that is not one of those three -- a \
         profession, a fictional person, a described manner -- and needs the \
         character field in their own words.\n\
         'none' returns the robot to its own voice."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "stance": {
                    "type": "string",
                    "enum": ["twin", "friend", "mentor", "character", "none"],
                    "description": "Use twin, friend or mentor whenever the person \
                                    names one of them -- never 'character' with \
                                    the word 'twin' in it. 'character' is for \
                                    anything else and needs the character field."
                },
                "character": {
                    "type": "string",
                    "description": "Who to be, in the PERSON'S OWN WORDS, copied \
                                    from their message. Only with stance \
                                    'character'."
                }
            },
            "required": ["stance"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: StanceArgs = typed(args)?;
        match a.stance.as_str() {
            "twin" | "friend" | "mentor" | "none" => Ok(()),
            "character" => a
                .character
                .as_ref()
                .filter(|c| !c.trim().is_empty())
                .map(|_| ())
                .ok_or_else(|| "a character needs describing".to_string()),
            other => Err(format!("no stance {other}")),
        }
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: StanceArgs = typed(args).map_err(PrismError::Capability)?;
        let want = match a.stance.as_str() {
            "none" => None,
            "character" => Some(soul::stance::Stance::Character(
                a.character.clone().unwrap_or_default(),
            )),
            s => soul::stance::Stance::parse(s),
        };
        ctx.cell
            .with(|c| soul::stance::set(c, want.as_ref()).map_err(soul_err))?;
        let label = want
            .as_ref()
            .map(|s| s.label())
            .unwrap_or_else(|| "its own".into());
        attested(
            super::row_evidence("soul.stance", ""),
            format!("took the stance: {label}"),
            Rendering::new("soul_stance", serde_json::json!({ "stance": label })),
        )
    }
}
