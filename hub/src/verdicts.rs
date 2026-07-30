//! The one verdict call (Q16) through the gateway: gemma-4-26b-a4b with
//! structured output, salvage fallback, and the deterministic fallback
//! verdict as the floor under everything (sec 6a: tolerant JSON handling --
//! strip fences, salvage the largest valid object, validate; invalid ->
//! retry is the chain's business, then fall back).

use crate::gateway::{ModelGateway, Msg, Role};
use prism::types::Verdict;
use prism::verdict::{FallbackVerdict, VerdictProvider};
use std::sync::Arc;

const VERDICT_SYSTEM: &str = "you are the router of a personal robot. \
classify the user's message into exactly one JSON verdict. output ONLY the \
JSON object, nothing else. fields: action (answer|task|search|meta|clarify|\
chitchat), domain (reminder|note|fact|calendar|email|file|none), door \
(exact|vector|web|blended|followup), tier (fast|super|ultra), lang \
(two-letter language of the user's message), mood {valence: -1..1, urgency: \
0..1}, confidence (0..1), reply (a one-liner ONLY for chitchat, else omit). \
guidance: questions about stored personal facts -> action answer, door \
vector. anything needing current/web information -> action search, door \
web. greetings/small talk -> chitchat with a short warm reply. hard \
reasoning (code, math, multi-step analysis) -> tier super.";

fn q16_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {"type": "string", "enum": ["answer","task","search","meta","clarify","chitchat"]},
            "domain": {"type": "string", "enum": ["reminder","note","fact","calendar","email","file","none"]},
            "door":   {"type": "string", "enum": ["exact","vector","web","blended","followup"]},
            "tier":   {"type": "string", "enum": ["fast","super","ultra"]},
            "lang":   {"type": "string"},
            "mood":   {"type": "object", "properties": {
                          "valence": {"type": "number"}, "urgency": {"type": "number"}},
                       "required": ["valence","urgency"]},
            "confidence": {"type": "number"},
            "reply": {"type": "string"}
        },
        "required": ["action","domain","door","tier","lang","mood","confidence"]
    })
}

/// Strip code fences and salvage the largest balanced JSON object.
pub fn salvage_json(text: &str) -> Option<serde_json::Value> {
    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(v) = serde_json::from_str(cleaned) {
        return Some(v);
    }
    // largest balanced {...}
    let bytes = cleaned.as_bytes();
    let mut best: Option<&str> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let mut depth = 0usize;
            let mut in_str = false;
            let mut esc = false;
            for (j, &b) in bytes.iter().enumerate().skip(i) {
                if esc {
                    esc = false;
                    continue;
                }
                match b {
                    b'\\' if in_str => esc = true,
                    b'"' => in_str = !in_str,
                    b'{' if !in_str => depth += 1,
                    b'}' if !in_str => {
                        depth -= 1;
                        if depth == 0 {
                            let cand = &cleaned[i..=j];
                            if best.map(|b| cand.len() > b.len()).unwrap_or(true) {
                                best = Some(cand);
                            }
                            i = j;
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        i += 1;
    }
    best.and_then(|s| serde_json::from_str(s).ok())
}

/// The gateway-backed verdict provider. Any failure anywhere degrades to
/// the deterministic fallback -- the doorman may be wrong, never absent.
pub struct GatewayVerdicts {
    pub gateway: Arc<ModelGateway>,
}

impl VerdictProvider for GatewayVerdicts {
    fn verdict(&self, text: &str) -> Verdict {
        let messages = [
            Msg {
                role: "system",
                content: VERDICT_SYSTEM.into(),
            },
            Msg {
                role: "user",
                content: text.into(),
            },
        ];
        let out = match self
            .gateway
            .chat(Role::Verdict, &messages, Some(q16_schema()), 300)
        {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!("verdict call failed, deterministic fallback: {e}");
                return FallbackVerdict.verdict(text);
            }
        };
        match salvage_json(&out.content).and_then(|v| serde_json::from_value::<Verdict>(v).ok()) {
            Some(v) => v,
            None => {
                tracing::warn!("verdict unparseable, deterministic fallback");
                FallbackVerdict.verdict(text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salvage_handles_fences_and_prose() {
        let fenced = "```json\n{\"a\": 1}\n```";
        assert_eq!(salvage_json(fenced).unwrap()["a"], 1);
        let prose = "sure! here's the verdict: {\"action\":\"answer\",\"n\":{\"x\":2}} hope it helps";
        assert_eq!(salvage_json(prose).unwrap()["n"]["x"], 2);
        let strings = r#"{"s": "curly } inside", "ok": true}"#;
        assert_eq!(salvage_json(strings).unwrap()["ok"], true);
        assert!(salvage_json("no json here").is_none());
    }

    #[test]
    fn q16_verdict_parses_from_salvaged_json() {
        let raw = r#"{"action":"search","domain":"none","door":"web","tier":"fast",
                      "lang":"en","mood":{"valence":0.1,"urgency":0.3},"confidence":0.8}"#;
        let v: Verdict = serde_json::from_value(salvage_json(raw).unwrap()).unwrap();
        assert_eq!(v.lang, "en");
    }
}
