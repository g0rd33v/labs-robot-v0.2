//! The capability registry.
//!
//! Each capability is one type implementing `Capability`, registered by
//! name. Three things this fixes over the previous 390-line match:
//!
//! 1. **`effect()` lives next to the implementation.** Previously the
//!    effect class was declared in `prism::lifecycle::plan_from_decision`
//!    while the code that performs it lived in `robotd` -- two crates apart,
//!    with nothing tying them together, so they could silently disagree
//!    about whether something was a read or an irreversible write.
//! 2. **Adding a capability is one file**, not three edits across two crates.
//! 3. **Each capability is unit-testable** without constructing a RobotCore.

pub mod admin;
pub mod answer;
pub mod basics;
pub mod memory;
pub mod reminders;
pub mod research;
pub mod soul;

use prism::types::{Effect, Evidence, Outcome, Rendering, ToolDef};
use prism::{Cell, CapabilityRouter, PrismError};
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Outbound services a capability may use. Built once at boot.
#[derive(Default, Clone)]
pub struct Services {
    pub embedder: Option<Arc<hub::Embedder>>,
    pub gateway: Option<Arc<hub::ModelGateway>>,
    pub research: Option<Arc<hub::Research>>,
}

impl Services {
    pub fn passage_embedding(&self, text: &str) -> Option<Vec<f32>> {
        self.embedder
            .as_ref()
            .and_then(|e| e.embed_passage(text).ok())
    }

    pub fn query_embedding(&self, text: &str) -> Option<Vec<f32>> {
        self.embedder.as_ref().and_then(|e| e.embed_query(text).ok())
    }
}

/// Instance-level identity and authority.
#[derive(Default, Clone)]
pub struct Instance {
    /// core.db, for capabilities that touch instance-wide state
    pub core: Option<Arc<Mutex<Connection>>>,
    pub owner_principal: i64,
    pub public_base: String,
    /// Which INSTALLATION this is. Two instances of the same robot -- the
    /// machine and the stick -- share a `robot_id` and differ here, which
    /// is what lets a deletion be attributed and a sync watermark mean
    /// something.
    pub instance_id: String,
}

/// Owner-settable policy.
#[derive(Default, Clone, Copy)]
pub struct Policy {
    pub ultra_daily_cap: u32,
}

/// Everything a capability gets for one execution. Borrowed, so this
/// allocates nothing per turn.
pub struct Ctx<'a> {
    pub cell: &'a Cell,
    pub intent_id: &'a str,
    /// The acting principal, PASSED DOWN rather than re-parsed from the
    /// journal blob. Recovering identity by JSON-parsing a payload and
    /// defaulting to -1 on failure is not how an authorization input should
    /// reach a check.
    pub principal: i64,
    pub services: &'a Services,
    pub policy: &'a Policy,
    pub instance: &'a Instance,
    /// The turn's language (BCP 47), carried for capabilities that must
    /// pass it outward -- to a model, say. Capabilities never render with
    /// it: they emit `Rendering`, and the surface does the words.
    pub lang: &'a str,
}

impl Ctx<'_> {
    /// Provenance anchor (law #5): the source message id journaled at
    /// intent_open. Capabilities that store knowledge refuse without it.
    pub fn source_msg(&self) -> Result<String, PrismError> {
        let payload = self
            .cell
            .with(|c| prism::journal::payload_of(c, self.intent_id, "intent_open"))?
            .ok_or_else(|| PrismError::Capability("no intent_open journaled".into()))?;
        let v: serde_json::Value = serde_json::from_str(&payload)?;
        v["source_msg_id"].as_str().map(String::from).ok_or_else(|| {
            PrismError::Capability(
                "no source message journaled; refusing to store an unsourced fact (law #5)".into(),
            )
        })
    }

    /// Owner-only gate for administrative capabilities. The role check is
    /// the FIRST thing that happens -- it used to sit behind an availability
    /// check that every test tripped first, so the comparison never ran.
    pub fn require_owner(&self, what: &str) -> Result<Arc<Mutex<Connection>>, Rendering> {
        if self.principal != self.instance.owner_principal {
            return Err(Rendering::new(
                "owner_only",
                serde_json::json!({ "what": what }),
            ));
        }
        self.instance.core.clone().ok_or_else(|| {
            Rendering::new("not_available_here", serde_json::json!({ "what": what }))
        })
    }
}

/// One capability: a name, an effect class, a self-description, and an
/// implementation.
///
/// The four declarative methods together ARE the tool definition handed to
/// a model. Because they live beside the code that runs, a capability
/// cannot describe itself as something it is not, and adding a capability
/// makes it reachable in every language on the same commit.
pub trait Capability: Send + Sync {
    fn name(&self) -> &'static str;
    /// Declared beside the code that performs it.
    fn effect(&self) -> Effect;

    /// One English sentence: what this is for, and when to reach for it.
    ///
    /// This sentence is the whole multilingual mechanism. It is what lets a
    /// model map any phrasing in any language onto this tool, and it
    /// replaces every phrase table we used to maintain. Write it as if
    /// explaining to a capable colleague who cannot see the code.
    fn description(&self) -> &'static str;

    /// JSON Schema for `args`. Two classes of argument, and the split
    /// carries law 5: STRUCTURAL fields (times, indices) are typed and
    /// language-free; CONTENT fields hold the person's own words verbatim
    /// and must never be translated, or provenance would point at words
    /// they never wrote.
    fn schema(&self) -> serde_json::Value;

    /// Reject arguments that do not typecheck, BEFORE anything executes.
    /// Implemented by deserializing into the same struct `execute` reads,
    /// so the check cannot drift from the code it guards.
    fn validate(&self, args: &serde_json::Value) -> Result<(), String>;

    /// Whether this capability appears in the catalog offered to a model.
    /// The fallback answer path is a capability but not a tool: it is where
    /// a turn goes when NO tool fits, so offering it would let the model
    /// choose the escape hatch over doing the work.
    fn exposed(&self) -> bool {
        true
    }

    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value)
        -> Result<Outcome, PrismError>;
}

/// The schema for a capability that takes nothing.
pub fn no_args() -> serde_json::Value {
    serde_json::json!({
        "type": "object", "properties": {}, "additionalProperties": false
    })
}

/// Deserialize `args` into a capability's own argument struct, turning a
/// serde error into a sentence a person could act on.
pub fn typed<T: serde::de::DeserializeOwned>(
    args: &serde_json::Value,
) -> Result<T, String> {
    serde_json::from_value(args.clone()).map_err(|e| e.to_string())
}

// ---- shared outcome constructors -------------------------------------
// `attested` = a verified state transition; its text is the receipt claim.
// `spoke`    = a model produced text; the receipt records that a model
//              spoke, never what it said (arch sec 3).
// `failed`   = the step did not do what it set out to; the receipt must not
//              come out Verified.

pub fn row_evidence(id: &str, hash: &str) -> Vec<Evidence> {
    vec![Evidence {
        kind: "row".into(),
        provider: "cell".into(),
        external_id: id.into(),
        hash: hash.into(),
        ts: trust::ids::ts_ms(),
    }]
}

pub fn note_evidence(id: &str) -> Vec<Evidence> {
    vec![Evidence {
        kind: "deterministic".into(),
        provider: "robot".into(),
        external_id: id.into(),
        hash: String::new(),
        ts: trust::ids::ts_ms(),
    }]
}

pub fn model_evidence(model: &str, content: &str) -> Evidence {
    Evidence {
        kind: "provider_response".into(),
        provider: "openrouter".into(),
        external_id: model.into(),
        hash: trust::ids::sha256_hex(content.as_bytes()),
        ts: trust::ids::ts_ms(),
    }
}

/// `claim` is the ENGLISH sentence the receipt asserts -- audit text.
/// `say` is what the person is told, as structure.
pub fn attested(
    evidence: Vec<Evidence>,
    claim: impl Into<String>,
    say: Rendering,
) -> Result<Outcome, PrismError> {
    Ok(Outcome::attested(String::new(), evidence, claim.into(), say))
}

pub fn spoke(evidence: Vec<Evidence>, detail: String) -> Result<Outcome, PrismError> {
    Ok(Outcome::utterance(String::new(), evidence, detail))
}

pub fn failed(
    evidence: Vec<Evidence>,
    claim: impl Into<String>,
    say: Rendering,
) -> Result<Outcome, PrismError> {
    Ok(Outcome::failed(String::new(), evidence, claim.into(), say))
}

/// A capability declining. Not a failure of the machine -- an honest no.
pub fn declined(what: &'static str, say: Rendering) -> Result<Outcome, PrismError> {
    attested(note_evidence(what), format!("declined: {what}"), say)
}

/// Map a mind error into a capability error.
pub fn mind_err(e: impl std::fmt::Display) -> PrismError {
    PrismError::Capability(e.to_string())
}

// ---- the registry -----------------------------------------------------

pub struct Registry {
    caps: HashMap<&'static str, Box<dyn Capability>>,
    pub services: Services,
    pub policy: Policy,
    pub instance: Instance,
}

impl Registry {
    pub fn new(services: Services, policy: Policy, instance: Instance) -> Self {
        let mut caps: HashMap<&'static str, Box<dyn Capability>> = HashMap::new();
        for cap in all_capabilities() {
            caps.insert(cap.name(), cap);
        }
        Self {
            caps,
            services,
            policy,
            instance,
        }
    }

    /// Floor-only registry for tests and the offline kill-suite: no
    /// services, no core, and therefore no owner. Deliberately NOT a
    /// `Default` impl -- a default-constructed router that silently refuses
    /// everything is exactly how the owner-only checks went untested.
    pub fn offline() -> Self {
        Self::new(Services::default(), Policy::default(), Instance::default())
    }

    pub fn effect_of(&self, name: &str) -> Option<Effect> {
        self.caps.get(name).map(|c| c.effect())
    }

    /// The tool catalog offered to a model: generated from the registry,
    /// never authored. Adding a capability makes it reachable in every
    /// language on the same commit; there is no second list to update.
    pub fn catalog(&self) -> Vec<ToolDef> {
        let mut tools: Vec<ToolDef> = self
            .caps
            .values()
            .filter(|c| c.exposed())
            .map(|c| ToolDef {
                name: c.name(),
                description: c.description(),
                input_schema: c.schema(),
                effect: c.effect(),
            })
            .collect();
        tools.sort_by_key(|t| t.name);
        tools
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut n: Vec<_> = self.caps.keys().copied().collect();
        n.sort();
        n
    }
}

fn all_capabilities() -> Vec<Box<dyn Capability>> {
    vec![
        Box::new(basics::TimeNow),
        Box::new(basics::About),
        Box::new(basics::Help),
        Box::new(reminders::Create),
        Box::new(reminders::List),
        Box::new(reminders::CancelLast),
        Box::new(memory::Remember),
        Box::new(memory::Recall),
        Box::new(memory::RegistryList),
        Box::new(memory::Forget),
        Box::new(memory::Correct),
        Box::new(admin::Invite),
        Box::new(admin::TelegramBindCode),
        Box::new(answer::ModelAnswer),
        Box::new(research::WebResearch),
        Box::new(soul::Show),
        Box::new(soul::Set),
        Box::new(soul::Bounds),
        Box::new(soul::Evolution),
    ]
}

impl CapabilityRouter for Registry {
    fn describe(&self, cell: &Cell) -> Vec<ToolDef> {
        let mut tools = self.catalog();
        // the answering tool exists only around a question -- while one is
        // open, and briefly after it closes so a late yes can be told it is
        // late. Outside that window a model cannot invent a confirmation
        // for something nobody asked.
        if prism::pending::answerable(cell) {
            tools.push(ToolDef {
                name: prism::lifecycle::CONFIRM_TOOL,
                description: "Answer the yes-or-no question the robot asked \
                              about an irreversible action. Use this, and only \
                              this, when their message is an answer to that \
                              question -- agreement of any kind is confirmed: \
                              true, refusal or hesitation is false. Use it even \
                              if you believe the question was already settled: \
                              the robot will tell them if their answer arrived \
                              too late, and that is better than guessing.",
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "confirmed": {
                            "type": "boolean",
                            "description": "True only for a clear yes."
                        }
                    },
                    "required": ["confirmed"],
                    "additionalProperties": false
                }),
                effect: Effect::Read,
            });
            tools.sort_by_key(|t| t.name);
        }
        tools
    }

    fn validate(&self, tool: &str, args: &serde_json::Value) -> Result<Effect, String> {
        if tool == prism::lifecycle::CONFIRM_TOOL {
            return args["confirmed"]
                .as_bool()
                .map(|_| Effect::Read)
                .ok_or_else(|| "confirmed must be a boolean".to_string());
        }
        let cap = self
            .caps
            .get(tool)
            .ok_or_else(|| format!("no such tool: {tool}"))?;
        if !cap.exposed() {
            return Err(format!("{tool} is not callable from outside"));
        }
        cap.validate(args)?;
        Ok(cap.effect())
    }

    fn execute(
        &self,
        cell: &Cell,
        capability: &str,
        args: &serde_json::Value,
        intent_id: &str,
        lang: &str,
    ) -> Result<Outcome, PrismError> {
        let cap = self
            .caps
            .get(capability)
            .ok_or_else(|| PrismError::Capability(format!("unknown capability: {capability}")))?;
        // the acting principal comes from the journaled intent, read once
        // here rather than re-derived inside every capability
        let principal = principal_of(cell, intent_id);
        let ctx = Ctx {
            cell,
            intent_id,
            principal,
            services: &self.services,
            policy: &self.policy,
            instance: &self.instance,
            lang,
        };
        cap.execute(&ctx, args)
    }
}

fn principal_of(cell: &Cell, intent_id: &str) -> i64 {
    cell.with(|c| prism::journal::payload_of(c, intent_id, "intent_open"))
        .ok()
        .flatten()
        .and_then(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
        .and_then(|v| v["principal_id"].as_i64())
        .unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_planned_capability_is_registered() {
        let reg = Registry::offline();
        // the names prism's planner can emit
        for name in [
            "reminder.create",
            "reminder.list",
            "reminder.cancel_last",
            "memory.remember",
            "memory.recall",
            "registry.list",
            "memory.forget",
            "memory.correct",
            "member.invite",
            "telegram.bind_code",
            "answer.model",
            "web.research",
        ] {
            assert!(
                reg.effect_of(name).is_some(),
                "planner can emit {name} but nothing implements it"
            );
        }
    }

    /// The effect class now lives beside the implementation; assert the
    /// ones that matter, so a write can never be reclassified as a read
    /// without this failing.
    #[test]
    fn effect_classes_are_declared_correctly() {
        let reg = Registry::offline();
        assert_eq!(reg.effect_of("memory.recall"), Some(Effect::Read));
        assert_eq!(reg.effect_of("registry.list"), Some(Effect::Read));
        assert_eq!(reg.effect_of("web.research"), Some(Effect::Read));
        assert_eq!(
            reg.effect_of("reminder.create"),
            Some(Effect::ReversibleWrite)
        );
        assert_eq!(
            reg.effect_of("memory.remember"),
            Some(Effect::ReversibleWrite)
        );
        // deletion is real and irreversible -- it must be classified as such
        assert_eq!(reg.effect_of("memory.forget"), Some(Effect::Irreversible));
    }

    /// The catalog is what a model sees. Every entry must be usable: a
    /// missing description is a tool nobody can find, and a schema that is
    /// not an object is a tool nobody can call.
    #[test]
    fn every_tool_describes_itself_usefully() {
        let reg = Registry::offline();
        let catalog = reg.catalog();
        // the invariant is a relationship, not a magic number: the catalog
        // is exactly the exposed capabilities. A hard-coded count fails
        // every time a capability is added, which trains people to bump it
        // without reading what changed.
        let exposed: Vec<&str> = reg
            .caps
            .values()
            .filter(|c| c.exposed())
            .map(|c| c.name())
            .collect();
        assert_eq!(catalog.len(), exposed.len());
        for name in &exposed {
            assert!(
                catalog.iter().any(|t| &t.name == name),
                "{name} is exposed but missing from the catalog"
            );
        }
        assert!(catalog.len() > 10, "the catalog looks empty: {catalog:?}");

        for t in &catalog {
            assert!(
                t.description.len() > 40,
                "{} has a description too thin to route on: {:?}",
                t.name,
                t.description
            );
            let schema = &t.input_schema;
            assert_eq!(schema["type"], "object", "{}", t.name);
            assert_eq!(
                schema["additionalProperties"], false,
                "{} accepts unknown fields; a model's stray key would slip through",
                t.name
            );
            // the declared effect must be the registry's, not a gentler one
            assert_eq!(reg.effect_of(t.name), Some(t.effect), "{}", t.name);
        }
    }

    /// The escape hatch is not on the menu.
    #[test]
    fn the_fallback_answer_is_not_offered_as_a_tool() {
        let reg = Registry::offline();
        assert!(reg.effect_of("answer.model").is_some());
        assert!(!reg.catalog().iter().any(|t| t.name == "answer.model"));
    }

    /// Validation is the safety story for anything a model proposes.
    #[test]
    fn a_proposed_call_is_checked_against_the_registry() {
        let reg = Registry::offline();

        assert!(reg.validate("registry.list", &serde_json::json!({})).is_ok());
        assert_eq!(
            reg.validate("memory.forget", &serde_json::json!({"index": 3})),
            Ok(Effect::Irreversible)
        );

        // an invented tool, a hidden one, bad types, missing and extra fields
        assert!(reg.validate("memory.explode", &serde_json::json!({})).is_err());
        assert!(reg.validate("answer.model", &serde_json::json!({})).is_err());
        assert!(reg.validate("memory.forget", &serde_json::json!({"index": "3"})).is_err());
        assert!(reg.validate("memory.remember", &serde_json::json!({})).is_err());
        assert!(reg
            .validate("memory.remember", &serde_json::json!({"content": "x", "extra": 1}))
            .is_err());
    }

    #[test]
    fn unknown_capability_is_an_error() {
        let reg = Registry::offline();
        assert!(reg.effect_of("memory.explode").is_none());
    }
}
