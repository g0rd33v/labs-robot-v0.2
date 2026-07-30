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
    pub steps: Vec<PlanStep>,
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
    /// Human-usable English fragment describing what happened; feeds the
    /// reply composer. Never a model narration of success (receipts law).
    pub detail: String,
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
