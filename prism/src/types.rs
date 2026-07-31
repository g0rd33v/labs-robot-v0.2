//! The five core objects (appendix A) + the Q16 verdict. Frozen vocabulary:
//! these names are the dialect of the codebase, the dashboard, and the docs.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- effects

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Read,
    ReversibleWrite,
    Irreversible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    Auto,
    Required,
}

// ---------------------------------------------------------------- intent

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub intent_id: String,
    pub principal: i64,
    pub desired_outcome: String,
    pub constraints: Vec<String>,
    pub confidence: f32,
    pub risk_class: String,
    pub status: String,
}

// ---------------------------------------------------------------- tools

/// One capability, described the way the industry describes a tool: an
/// English name, an English sentence saying what it is for, and a JSON
/// Schema for its arguments.
///
/// This is the ENTIRE multilingual mechanism. A model maps any phrasing in
/// any language onto `reminder.create` because the description says what
/// the tool does -- not because anyone wrote a phrase table. The number of
/// supported languages appears nowhere in this codebase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
    pub effect: Effect,
}

/// A tool call as proposed by a model -- untrusted until validated against
/// the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// A tool call that has been through the registry: the tool exists, the
/// arguments typecheck, and `effect` is the registry's own classification
/// rather than anything the proposer claimed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedCall {
    pub tool: String,
    pub args: serde_json::Value,
    pub effect: Effect,
}

/// What one routing call returns: the frozen Q16 verdict, plus an optional
/// proposed tool call beside it. Q16 itself is untouched -- the envelope
/// around it gained a sibling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routing {
    pub verdict: Verdict,
    #[serde(default)]
    pub call: Option<ToolCall>,
}

// ---------------------------------------------------------------- plan

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub step_id: String,
    pub capability: String,
    pub args: serde_json::Value,
    pub effect: Effect,
    pub approval: Approval,
    pub deps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub plan_id: String,
    pub intent_id: String,
    /// The language this turn is answered in, resolved once at decision
    /// time and journaled with the plan so a replayed intent speaks the
    /// same language the live one did. Defaulted for plans journaled
    /// before packs existed.
    #[serde(default = "default_lang")]
    pub lang: String,
    pub steps: Vec<PlanStep>,
}

pub(crate) fn default_lang() -> String {
    "en".into()
}

// ---------------------------------------------------------------- grant

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    pub grant_id: String,
    pub capability: String,
    pub scope: serde_json::Value,
    pub principal: i64,
    pub expires_at: i64,
    pub issued_by: String,
}

/// Why a grant did not authorise a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantDenial {
    /// The authority lapsed before the step ran. Reachable in ordinary
    /// operation: a turn interrupted by a crash and resumed later, or a
    /// step that waited for an approval that never came.
    Expired { by_ms: i64 },
    /// The grant authorises a different capability than the step performs.
    WrongCapability { granted: String, attempted: String },
    /// The step's arguments are not the ones the grant was issued for.
    /// This is the check that makes "narrow" mean something: authority for
    /// `reminder.create{about: "call mark"}` is not authority for
    /// `reminder.create{about: anything else}`.
    OutOfScope { field: String },
}

impl std::fmt::Display for GrantDenial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrantDenial::Expired { by_ms } => {
                write!(f, "the authority for this step expired {by_ms} ms ago")
            }
            GrantDenial::WrongCapability { granted, attempted } => write!(
                f,
                "the authority covers {granted}, not {attempted}"
            ),
            GrantDenial::OutOfScope { field } => {
                write!(f, "the authority does not cover this step's '{field}'")
            }
        }
    }
}

impl Grant {
    /// Does this grant authorise this exact step, right now?
    ///
    /// Minting a grant and never reading it is the shape of an authority
    /// model that only looks like one. The three checks below are what make
    /// arch sec 3's sentence true -- *"narrow, time-boxed authority: may use
    /// `calendar.create`, on this calendar, for this event, until Friday"*
    /// -- rather than aspirational.
    ///
    /// Scope is checked field by field against the step's arguments, so a
    /// grant issued for one reminder cannot execute a different one. That
    /// matters most on replay: the plan is read back from the journal, and
    /// the grant is the thing that says it is still the plan that was
    /// authorised.
    pub fn authorises(
        &self,
        capability: &str,
        args: &serde_json::Value,
        now_ms: i64,
    ) -> Result<(), GrantDenial> {
        if self.capability != capability {
            return Err(GrantDenial::WrongCapability {
                granted: self.capability.clone(),
                attempted: capability.into(),
            });
        }
        if now_ms > self.expires_at {
            return Err(GrantDenial::Expired {
                by_ms: now_ms - self.expires_at,
            });
        }
        // every argument the step carries must be the one the grant was
        // issued for. A grant with a wider scope than the step is fine --
        // authority may exceed use; use may never exceed authority.
        if let Some(want) = args.as_object() {
            for (k, v) in want {
                match self.scope.get(k) {
                    Some(granted) if granted == v => {}
                    _ => {
                        return Err(GrantDenial::OutOfScope { field: k.clone() });
                    }
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- receipt

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Proposed,
    Submitted,
    Accepted,
    Verified,
    Failed,
    Partial,
    Uncertain,
}

impl ReceiptStatus {
    /// Terminal = the intent may close on it.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ReceiptStatus::Verified
                | ReceiptStatus::Failed
                | ReceiptStatus::Partial
                | ReceiptStatus::Uncertain
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ReceiptStatus::Proposed => "proposed",
            ReceiptStatus::Submitted => "submitted",
            ReceiptStatus::Accepted => "accepted",
            ReceiptStatus::Verified => "verified",
            ReceiptStatus::Failed => "failed",
            ReceiptStatus::Partial => "partial",
            ReceiptStatus::Uncertain => "uncertain",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    /// e.g. "row", "provider_response", "deterministic"
    #[serde(rename = "type")]
    pub kind: String,
    pub provider: String,
    pub external_id: String,
    pub hash: String,
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub claim: String,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub receipt_id: String,
    pub intent_id: String,
    pub status: ReceiptStatus,
    pub claims: Vec<Claim>,
    pub models_used: Vec<String>,
    pub data_disclosures: Vec<String>,
}

// ---------------------------------------------------------------- rendering

/// What to say, as DATA. An English identifier plus typed slots -- never a
/// sentence.
///
/// This is the boundary law 4 asks for, made structural: the kernel emits
/// `{id: "reminder_created", slots: {when: <ts>, about: "..."}}` and has no
/// opinion about words. The surface turns it into a sentence, in English
/// from templates or in any other language from a model. Nothing in the
/// kernel can be in the wrong language, because nothing in the kernel is
/// in a language.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rendering {
    pub id: String,
    #[serde(default)]
    pub slots: serde_json::Value,
}

impl Rendering {
    pub fn new(id: &str, slots: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            slots,
        }
    }

    pub fn bare(id: &str) -> Self {
        Self::new(id, serde_json::json!({}))
    }
}

/// One piece of the reply on its way out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReplyPart {
    /// Kernel-authored: structure the surface renders.
    Say(Rendering),
    /// A model spoke. Already in the person's language, passed through
    /// untouched -- and never the vehicle for an effect claim, because
    /// effects are shown as action records built from the receipt.
    Prose(String),
}

/// A machine-generated record of something that actually happened, built
/// from the receipt rather than from anyone's prose.
///
/// Shown beside the reply. If a sentence claims an effect and no record
/// appears next to it, the discrepancy is visible without reading a word --
/// which is how the receipts law survives a model writing the sentence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRecord {
    pub tool: String,
    pub status: String,
    pub detail: String,
}

// ---------------------------------------------------------------- outcome

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub step_id: String,
    pub ok: bool,
    pub evidence: Vec<Evidence>,
    /// What the person sees. For capability steps this is system-generated;
    /// for model steps it is the model's own prose.
    pub detail: String,
    /// What the RECEIPT asserts about the world.
    ///
    /// `Some(_)` only when the text describes a state transition this code
    /// actually performed and can point at evidence for. `None` means the
    /// step produced an utterance and asserts nothing -- model prose is
    /// never promoted to a receipt claim (arch sec 3: receipts are compiled
    /// from evidence, never narrated by the model that acted). Without this
    /// split, a model replying "I've set that reminder" produced a
    /// `Verified` receipt whose only evidence was that a model spoke.
    #[serde(default)]
    pub claim: Option<String>,
    /// What the person should be told, as structure. `None` means this
    /// step's `detail` is model prose and goes out as it stands.
    #[serde(default)]
    pub rendering: Option<Rendering>,
}

impl Outcome {
    /// A step that performed a verified state transition: its text is both
    /// what the person reads and what the receipt asserts.
    /// A step that performed a verified state transition.
    ///
    /// `claim` is the English sentence the RECEIPT asserts -- audit text,
    /// read by the owner, never shown as the reply. `rendering` is what the
    /// person is told, and it is data.
    pub fn attested(
        step_id: String,
        evidence: Vec<Evidence>,
        claim: String,
        rendering: Rendering,
    ) -> Self {
        Self {
            step_id,
            ok: true,
            evidence,
            claim: Some(claim.clone()),
            detail: claim,
            rendering: Some(rendering),
        }
    }

    /// A step that produced an utterance only (model answers, chitchat).
    /// The receipt records that a model spoke, not what it said.
    pub fn utterance(step_id: String, evidence: Vec<Evidence>, detail: String) -> Self {
        Self {
            step_id,
            ok: true,
            evidence,
            detail,
            claim: None,
            rendering: None,
        }
    }

    /// A step that failed. `ok: false` so the receipt cannot come out
    /// `Verified` -- an external call that failed is not a verified success.
    pub fn failed(
        step_id: String,
        evidence: Vec<Evidence>,
        claim: String,
        rendering: Rendering,
    ) -> Self {
        Self {
            step_id,
            ok: false,
            evidence,
            claim: Some(claim.clone()),
            detail: claim,
            rendering: Some(rendering),
        }
    }

    /// The part of the reply this step contributes.
    pub fn reply_part(&self) -> ReplyPart {
        match &self.rendering {
            Some(r) => ReplyPart::Say(r.clone()),
            None => ReplyPart::Prose(self.detail.clone()),
        }
    }

    /// True when this step actually changed something in the world.
    pub fn is_effect(&self) -> bool {
        self.ok && self.claim.is_some()
    }
}

// ---------------------------------------------------------------- verdict (Q16, frozen)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerdictAction {
    #[default]
    Answer,
    Task,
    Search,
    Meta,
    Clarify,
    Chitchat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerdictDomain {
    #[default]
    None,
    Reminder,
    Note,
    Fact,
    Calendar,
    Email,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Door {
    Exact,
    Vector,
    Web,
    Blended,
    #[default]
    Followup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    #[default]
    Fast,
    Super,
    Ultra,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Mood {
    /// -1..1
    pub valence: f32,
    /// 0..1
    pub urgency: f32,
}

/// The one verdict call's schema (decisions Q16, frozen for M3/M4).
///
/// Serialization is exactly the frozen shape. DEserialization is tolerant,
/// which is arch sec 6a's instruction in the small: a response that arrives
/// truncated -- a model padding its output and hitting the ceiling -- used
/// to fail the whole parse and discard a perfectly good tool call along
/// with it. A missing `tier` should cost us the tier, not the turn.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Verdict {
    #[serde(default)]
    pub action: VerdictAction,
    #[serde(default)]
    pub domain: VerdictDomain,
    #[serde(default)]
    pub door: Door,
    #[serde(default)]
    pub tier: Tier,
    #[serde(default = "default_lang")]
    pub lang: String,
    #[serde(default)]
    pub mood: Mood,
    #[serde(default)]
    pub confidence: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_serializes_to_the_q16_shape() {
        let v = Verdict {
            action: VerdictAction::Task,
            domain: VerdictDomain::Reminder,
            door: Door::Exact,
            tier: Tier::Fast,
            lang: "ru".into(),
            mood: Mood { valence: 0.0, urgency: 0.4 },
            confidence: 0.9,
            reply: None,
        };
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(j["action"], "task");
        assert_eq!(j["domain"], "reminder");
        assert_eq!(j["door"], "exact");
        assert_eq!(j["tier"], "fast");
        assert!(j.get("reply").is_none());
        // effects serialize snake_case
        assert_eq!(
            serde_json::to_value(Effect::ReversibleWrite).unwrap(),
            "reversible_write"
        );
    }

    #[test]
    fn receipt_statuses_terminal_set() {
        assert!(ReceiptStatus::Verified.is_terminal());
        assert!(ReceiptStatus::Failed.is_terminal());
        assert!(!ReceiptStatus::Proposed.is_terminal());
        assert!(!ReceiptStatus::Submitted.is_terminal());
    }
}

#[cfg(test)]
mod grant_tests {
    use super::*;

    fn grant(cap: &str, scope: serde_json::Value, expires_at: i64) -> Grant {
        Grant {
            grant_id: "g1".into(),
            capability: cap.into(),
            scope,
            principal: 7,
            expires_at,
            issued_by: "test".into(),
        }
    }

    /// The gate for item 1: a grant that does not cover the step refuses
    /// it. Before this, grants were minted, journaled, given an expiry --
    /// and never read. An authority model nothing consults is decoration.
    #[test]
    fn a_grant_authorises_exactly_what_it_says() {
        let args = serde_json::json!({ "index": 2 });
        let g = grant("memory.forget", args.clone(), 1_000);

        assert!(g.authorises("memory.forget", &args, 999).is_ok());

        // a different capability
        assert_eq!(
            g.authorises("memory.correct", &args, 999),
            Err(GrantDenial::WrongCapability {
                granted: "memory.forget".into(),
                attempted: "memory.correct".into()
            })
        );

        // the same capability on a DIFFERENT object -- the check that makes
        // "narrow" mean something
        assert_eq!(
            g.authorises("memory.forget", &serde_json::json!({ "index": 3 }), 999),
            Err(GrantDenial::OutOfScope {
                field: "index".into()
            })
        );

        // and an argument the grant never mentioned
        assert!(matches!(
            g.authorises(
                "memory.forget",
                &serde_json::json!({ "index": 2, "force": true }),
                999
            ),
            Err(GrantDenial::OutOfScope { .. })
        ));
    }

    /// Authority lapses. This is the case that actually happens: a turn
    /// interrupted by a crash and resumed hours later, or a step that
    /// waited for an approval nobody gave.
    #[test]
    fn authority_expires_and_a_late_resume_is_refused() {
        let args = serde_json::json!({ "about": "call mark" });
        let g = grant("reminder.create", args.clone(), 1_000);
        assert!(g.authorises("reminder.create", &args, 1_000).is_ok());
        assert_eq!(
            g.authorises("reminder.create", &args, 1_500),
            Err(GrantDenial::Expired { by_ms: 500 })
        );
    }

    /// Authority may exceed use; use may never exceed authority. A grant
    /// scoped more widely than the step is fine -- the reverse is not.
    #[test]
    fn a_wider_grant_still_covers_a_narrower_step() {
        let g = grant(
            "reminder.create",
            serde_json::json!({ "about": "call mark", "fire_at": "t", "extra": 1 }),
            1_000,
        );
        assert!(g
            .authorises(
                "reminder.create",
                &serde_json::json!({ "about": "call mark" }),
                999
            )
            .is_ok());
    }
}
