//! Escalation rules (Q18): declarative and deterministic, evaluated over
//! content signals -- never model vibes. Tier upgrades only; the verdict's
//! own tier is respected as a floor.

use prism::types::Tier;

/// Deterministic content signals -> tier. Code fences, math markers, and
/// explicit effort requests upgrade; everything else stays where the
/// verdict put it.
pub fn classify(text: &str) -> Tier {
    // English cues only. Other languages do not need a table here: the
    // routing call reads the message and returns `tier` in the verdict,
    // which `merge` respects as a floor.
    let lower = text.to_lowercase();
    if ["think ultra", "maximum effort"].iter().any(|s| lower.contains(s)) {
        return Tier::Ultra;
    }
    let mathy = text.chars().filter(|c| "∫∑√≈≠≤≥±".contains(*c)).count() >= 2;
    if mathy
        || ["```", "think hard", "step by step", "prove", "theorem"]
            .iter()
            .any(|s| lower.contains(s))
    {
        return Tier::Super;
    }
    Tier::Fast
}

/// Merge the verdict tier with the rules tier: the higher wins.
pub fn merge(verdict_tier: Tier, rules_tier: Tier) -> Tier {
    fn rank(t: Tier) -> u8 {
        match t {
            Tier::Fast => 0,
            Tier::Super => 1,
            Tier::Ultra => 2,
        }
    }
    if rank(rules_tier) > rank(verdict_tier) {
        rules_tier
    } else {
        verdict_tier
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_deterministic() {
        assert_eq!(classify("what's the capital of peru"), Tier::Fast);
        assert_eq!(classify("```rust\nfn main(){}\n``` review this"), Tier::Super);
        assert_eq!(classify("prove the theorem step by step"), Tier::Super);
        assert_eq!(classify("think ultra hard about this"), Tier::Ultra);
        assert_eq!(merge(Tier::Super, Tier::Fast), Tier::Super);
        assert_eq!(merge(Tier::Fast, Tier::Ultra), Tier::Ultra);
    }
}
