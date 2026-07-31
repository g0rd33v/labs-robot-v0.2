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
}

impl Outcome {
    /// A step that performed a verified state transition: its text is both
    /// what the person reads and what the receipt asserts.
    pub fn attested(step_id: String, evidence: Vec<Evidence>, detail: String) -> Self {
        Self {
            step_id,
            ok: true,
            evidence,
            claim: Some(detail.clone()),
            detail,
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
        }
    }

    /// A step that failed. `ok: false` so the receipt cannot come out
    /// `Verified` -- an external call that failed is not a verified success.
    pub fn failed(step_id: String, evidence: Vec<Evidence>, detail: String) -> Self {
        Self {
            step_id,
            ok: false,
            evidence,
            claim: Some(detail.clone()),
            detail,
        }
    }

    /// True when this step actually changed something in the world.
    pub fn is_effect(&self) -> bool {
        self.ok && self.claim.is_some()
    }
}

// ---------------------------------------------------------------- verdict (Q16, frozen)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictAction {
    Answer,
    Task,
    Search,
    Meta,
    Clarify,
    Chitchat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictDomain {
    Reminder,
    Note,
    Fact,
    Calendar,
    Email,
    File,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Door {
    Exact,
    Vector,
    Web,
    Blended,
    Followup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Fast,
    Super,
    Ultra,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mood {
    /// -1..1
    pub valence: f32,
    /// 0..1
    pub urgency: f32,
}

/// The one verdict call's schema (decisions Q16, frozen for M3/M4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub action: VerdictAction,
    pub domain: VerdictDomain,
    pub door: Door,
    pub tier: Tier,
    pub lang: String,
    pub mood: Mood,
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
