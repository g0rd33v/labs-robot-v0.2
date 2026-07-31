//! The one verdict call (Q16) through the gateway: gemma-4-26b-a4b with
//! structured output, salvage fallback, and the deterministic fallback
//! verdict as the floor under everything (sec 6a: tolerant JSON handling --
//! strip fences, salvage the largest valid object, validate; invalid ->
//! retry is the chain's business, then fall back).

use crate::gateway::{ModelGateway, Msg, Role};
use prism::types::{Routing, ToolDef, Verdict};
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

/// As `salvage_json`, for a JSON array -- the rendering call returns a list
/// of strings, and models like to wrap lists in prose just as much.
pub fn salvage_array(text: &str) -> Option<serde_json::Value> {
    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(v @ serde_json::Value::Array(_)) = serde_json::from_str(cleaned) {
        return Some(v);
    }
    let start = cleaned.find('[')?;
    let end = cleaned.rfind(']')?;
    (start < end)
        .then(|| serde_json::from_str(&cleaned[start..=end]).ok())
        .flatten()
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

    fn route(&self, text: &str, tools: &[ToolDef], now: &str) -> Routing {
        match self.route_call(text, tools, now) {
            Some(r) => r,
            None => {
                tracing::warn!("routing unparseable, deterministic fallback");
                Routing {
                    verdict: FallbackVerdict.verdict(text),
                    call: None,
                }
            }
        }
    }
}

// ---------------------------------------------------------------- routing

/// The routing prompt. Everything language-specific about this robot now
/// lives in ONE English sentence per tool, written beside the code that
/// runs -- and this prompt, which never names a language.
fn routing_system(tools: &[ToolDef], now: &str) -> String {
    let mut s = String::from(
        "you are the router of a personal robot. you do two things at once.\n\n\
         1. CLASSIFY the message into a verdict object.\n\
         2. If one of the tools below does what the person is asking for, \
         propose a call to it. If none fits, omit `call` entirely -- do not \
         force a tool.\n\n\
         rules that matter:\n\
         - the person may write in ANY language. never translate their words \
         when copying them into a tool argument. arguments described as \
         verbatim must contain their own text, in their own language, \
         unchanged.\n\
         - `lang` in the verdict is the BCP 47 tag of the language they wrote \
         in (e.g. en, ru, tr, ja, pt-BR).\n\
         - resolve every relative time into an absolute RFC 3339 timestamp \
         using the current time given below.\n\
         - propose at most one call.\n\
         - if you are unsure which tool is meant, omit `call` and let the \
         robot ask. a wrong action is worse than a question.\n\
         - output ONLY the JSON object.\n\n",
    );
    s.push_str(&format!("current local time: {now}\n\ntools:\n"));
    for t in tools {
        s.push_str(&format!(
            "\n- {} ({:?})\n  {}\n  args: {}\n",
            t.name,
            t.effect,
            t.description,
            t.input_schema
        ));
    }
    s
}

fn routing_schema(tools: &[ToolDef]) -> serde_json::Value {
    let names: Vec<&str> = tools.iter().map(|t| t.name).collect();
    serde_json::json!({
        "type": "object",
        "properties": {
            "verdict": q16_schema(),
            "call": {
                "type": "object",
                "properties": {
                    "tool": {"type": "string", "enum": names},
                    "args": {"type": "object"}
                },
                "required": ["tool", "args"]
            }
        },
        "required": ["verdict"]
    })
}

impl GatewayVerdicts {
    /// One model call, returning the frozen verdict and an optional proposal.
    ///
    /// Any failure degrades to a verdict with no call: the robot loses the
    /// action, never invents one.
    fn route_call(&self, text: &str, tools: &[ToolDef], now: &str) -> Option<Routing> {
        let messages = [
            Msg {
                role: "system",
                content: routing_system(tools, now),
            },
            Msg {
                role: "user",
                content: text.into(),
            },
        ];
        let out = self
            .gateway
            .chat(Role::Verdict, &messages, Some(routing_schema(tools)), 600)
            .map_err(|e| tracing::warn!("routing call failed: {e}"))
            .ok()?;
        let v = salvage_json(&out.content)?;
        // the verdict must parse; a proposal that does not is simply dropped,
        // since a half-understood call is worse than none
        let verdict: Verdict = serde_json::from_value(v.get("verdict")?.clone()).ok()?;
        let call = v
            .get("call")
            .filter(|c| !c.is_null())
            .and_then(|c| serde_json::from_value(c.clone()).ok());
        Some(Routing { verdict, call })
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
