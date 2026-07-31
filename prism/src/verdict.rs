//! The one verdict call (Q16). The floor runs first and wins (Q17); only
//! unmatched turns reach a verdict provider.
//!
//! M2 ships the frozen schema and a deterministic fallback provider (the
//! salvage default of arch sec 6a). M4 replaces the provider with the real
//! gateway call (gemma-4-26b-a4b) behind the same trait -- the lifecycle
//! does not change.

use crate::types::{
    Door, Mood, Routing, Tier, ToolDef, Verdict, VerdictAction, VerdictDomain,
};

pub trait VerdictProvider: Send + Sync {
    fn verdict(&self, text: &str) -> Verdict;

    /// Route a turn: classify it, and -- if one of the offered tools fits --
    /// propose a call.
    ///
    /// The default proposes nothing, which is what an offline provider
    /// honestly can do: no model, no routing beyond the English floor. A
    /// provider that cannot reach a model must degrade to fewer
    /// capabilities, never to guessed ones.
    fn route(&self, text: &str, _tools: &[ToolDef], _now: &str) -> Routing {
        Routing {
            verdict: self.verdict(text),
            call: None,
        }
    }
}

/// Deterministic fallback: no model, no network. Everything is a low-
/// confidence chitchat/answer verdict; the lifecycle answers honestly about
/// its current abilities.
pub struct FallbackVerdict;

impl VerdictProvider for FallbackVerdict {
    fn verdict(&self, text: &str) -> Verdict {
        // greetings live in the packs, like every other surface phrase;
        // the one that matches also tells us which language to answer in
        let greeting = crate::lexicon::greeting_pack(text);
        Verdict {
            action: if greeting.is_some() {
                VerdictAction::Chitchat
            } else {
                VerdictAction::Answer
            },
            domain: VerdictDomain::None,
            door: Door::Followup,
            tier: Tier::Fast,
            // offline, script is all we have: it is right for a greeting we
            // recognised and honestly "en" otherwise, which is what the
            // deterministic strings will render in anyway
            lang: greeting.map(|p| p.code.clone()).unwrap_or_else(|| "en".into()),
            mood: Mood {
                valence: 0.0,
                urgency: 0.1,
            },
            confidence: 0.2,
            reply: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_is_deterministic_and_low_confidence() {
        let v = FallbackVerdict.verdict("ponder the universe");
        assert_eq!(v.action, VerdictAction::Answer);
        assert!(v.confidence < 0.5);
        let v = FallbackVerdict.verdict("hello there");
        assert_eq!(v.action, VerdictAction::Chitchat);
    }
}
