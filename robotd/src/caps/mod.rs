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
pub mod memory;
pub mod reminders;
pub mod research;

use prism::types::{Effect, Evidence, Outcome};
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
    pub fn require_owner(&self, what: &str) -> Result<Arc<Mutex<Connection>>, String> {
        if self.principal != self.instance.owner_principal {
            return Err(format!("only the owner can {what}."));
        }
        self.instance
            .core
            .clone()
            .ok_or_else(|| format!("{what} isn't available in this context."))
    }
}

/// One capability: a name, an effect class, and an implementation.
pub trait Capability: Send + Sync {
    fn name(&self) -> &'static str;
    /// Declared beside the code that performs it.
    fn effect(&self) -> Effect;
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value)
        -> Result<Outcome, PrismError>;
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

pub fn attested(evidence: Vec<Evidence>, detail: String) -> Result<Outcome, PrismError> {
    Ok(Outcome::attested(String::new(), evidence, detail))
}

pub fn spoke(evidence: Vec<Evidence>, detail: String) -> Result<Outcome, PrismError> {
    Ok(Outcome::utterance(String::new(), evidence, detail))
}

pub fn failed(evidence: Vec<Evidence>, detail: String) -> Result<Outcome, PrismError> {
    Ok(Outcome::failed(String::new(), evidence, detail))
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

    pub fn names(&self) -> Vec<&'static str> {
        let mut n: Vec<_> = self.caps.keys().copied().collect();
        n.sort();
        n
    }
}

fn all_capabilities() -> Vec<Box<dyn Capability>> {
    vec![
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
    ]
}

impl CapabilityRouter for Registry {
    fn execute(
        &self,
        cell: &Cell,
        capability: &str,
        args: &serde_json::Value,
        intent_id: &str,
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

    #[test]
    fn unknown_capability_is_an_error() {
        let reg = Registry::offline();
        assert!(reg.effect_of("memory.explode").is_none());
    }
}
