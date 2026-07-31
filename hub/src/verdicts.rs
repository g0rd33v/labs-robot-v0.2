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

/// Strip code fences and recover a JSON object from a model's output.
///
/// Three things go wrong in practice, and this handles all three:
///
/// 1. **Fences and prose** around the object.
/// 2. **Truncation.** A model that hits its token ceiling stops mid-object,
///    sometimes after padding the tail with whitespace. The object is
///    complete enough to use -- it just never closed.
/// 3. **The subtle one.** Picking the largest *balanced* object out of a
///    truncated response returns an inner fragment: an unterminated
///    `{"call": {...}, "verdict": {...` yields the `call` object alone,
///    which parses fine and is missing everything else. That looked exactly
///    like a model refusing to route, and cost a whole live eval run before
///    the raw output was actually read.
///
/// So: always take the OUTERMOST object, and repair it if it did not close.
pub fn salvage_json(text: &str) -> Option<serde_json::Value> {
    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(cleaned) {
        return Some(v);
    }
    let start = cleaned.find('{')?;
    repair_object(&cleaned[start..])
}

/// Close an object that was cut off, discarding whatever partial value was
/// being written when the output stopped.
///
/// It has to know whether the last string it saw was a KEY or a VALUE: a
/// truncated `..., "lang"` must lose that key, while a truncated
/// `..., "lang": "ru"` must keep the pair. Guessing from punctuation alone
/// gets this backwards, so the position is tracked as it goes.
fn repair_object(src: &str) -> Option<serde_json::Value> {
    #[derive(PartialEq)]
    enum In {
        Object,
        Array,
    }
    let bytes = src.as_bytes();
    let mut stack: Vec<In> = vec![];
    let mut want_key = false;
    let mut in_str = false;
    let mut esc = false;
    // the last index at which the document was structurally sound: a
    // completed value, with nothing half-written after it
    let mut safe: Option<usize> = None;
    let mut last_solid: Option<usize> = None;

    for (i, &b) in bytes.iter().enumerate() {
        if esc {
            esc = false;
            continue;
        }
        if in_str {
            match b {
                b'\\' => esc = true,
                b'"' => {
                    in_str = false;
                    // a key is not a place we can stop; a value is
                    if !want_key {
                        safe = Some(i);
                    }
                    last_solid = Some(i);
                }
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => {
                stack.push(In::Object);
                want_key = true;
            }
            b'[' => {
                stack.push(In::Array);
                want_key = false;
            }
            b'}' | b']' => {
                stack.pop()?;
                safe = Some(i);
                last_solid = Some(i);
                want_key = stack.last().map(|c| *c == In::Object).unwrap_or(false);
                if stack.is_empty() {
                    // a complete outermost object: use exactly that
                    return serde_json::from_str(&src[..=i]).ok();
                }
            }
            b':' => want_key = false,
            b',' => {
                // whatever preceded the comma was a finished value, even a
                // bare number or literal that set no other marker
                safe = last_solid.or(safe);
                want_key = stack.last().map(|c| *c == In::Object).unwrap_or(false);
            }
            b' ' | b'\n' | b'\r' | b'\t' => {}
            _ => last_solid = Some(i),
        }
    }

    // truncated: cut back to the last sound point, drop a dangling
    // separator, and close every container still open
    let end = safe?;
    let mut out = src[..=end].trim_end().to_string();
    while out.ends_with(',') || out.ends_with(':') {
        out.pop();
        out = out.trim_end().to_string();
    }
    for c in stack.iter().rev() {
        out.push(if *c == In::Object { '}' } else { ']' });
    }
    serde_json::from_str(&out).ok()
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

/// One tool, rendered compactly for the prompt.
///
/// The full JSON Schema goes in the response format, not in the prompt --
/// pasting fourteen schemas made the prompt an order of magnitude larger
/// than the answer, which is how the first live run timed out on every
/// case. Name, purpose, and what each argument is: that is what routing
/// needs.
fn tool_line(t: &ToolDef) -> String {
    let mut args = vec![];
    if let Some(props) = t.input_schema.get("properties").and_then(|p| p.as_object()) {
        for (name, spec) in props {
            let ty = spec.get("type").and_then(|x| x.as_str()).unwrap_or("string");
            let doc = spec
                .get("description")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            args.push(format!("{name} ({ty}) -- {doc}"));
        }
    }
    let args = if args.is_empty() {
        "none".to_string()
    } else {
        format!("\n    {}", args.join("\n    "))
    };
    format!("- {}\n  {}\n  args: {}", t.name, t.description, args)
}

/// The routing prompt.
///
/// Everything language-specific about this robot now lives in ONE English
/// sentence per tool, written beside the code that runs -- and in this
/// prompt, which never names a language.
fn routing_system(tools: &[ToolDef], now: &str) -> String {
    let catalog: Vec<String> = tools.iter().map(tool_line).collect();
    format!(
        "you are the router of a personal robot. do BOTH of these, always.\n\n\
         1. CLASSIFY the message into `verdict`.\n\
         2. CHOOSE the one tool that does what the person is asking for and \
         fill `call`. if genuinely nothing in the list does it -- small talk, \
         a general knowledge question, anything the robot has no tool for -- \
         answer with tool \"none\". `call` is never omitted: \"none\" is a \
         decision and you must make it.\n\n\
         rules that matter:\n\
         - the person may write in ANY language. that changes nothing about \
         which tool fits. route on MEANING.\n\
         - never translate their words into a tool argument. arguments \
         described as verbatim must carry their own text, in their own \
         language, unchanged.\n\
         - `lang` is the BCP 47 tag of the language they wrote in (en, ru, \
         tr, ja, zh, pt-BR...).\n\
         - resolve every relative time into an absolute RFC 3339 timestamp \
         from the current time below.\n\
         - one call at most.\n\n\
         {}\n\n\
         current local time: {now}\n\n\
         tools:\n\n{}",
        output_shape(tools),
        catalog.join("\n\n")
    )
}

/// The sentinel meaning "no tool fits". A plain string rather than null:
/// "decide explicitly" was the property we wanted, and a nullable union is
/// the shape most likely to confuse a decoder.
pub const NO_TOOL: &str = "none";

/// The exact output shape, stated in the prompt rather than enforced as a
/// response schema.
///
/// A tool call's `args` is a different shape per tool, so it can only be a
/// free-form object -- and a constrained decoder asked to satisfy that pads
/// its output with whitespace until the token ceiling. That arrives as a
/// truncated response, or as a timeout, and looks exactly like a model
/// that will not route. It cost two full eval runs to see, because the
/// symptom is indistinguishable from refusal until you read the raw bytes.
///
/// So routing asks in words and verifies afterwards. Nothing is lost: the
/// response goes through salvage, repair, and registry validation before it
/// can do anything, and those were always the layers that mattered.
fn output_shape(tools: &[ToolDef]) -> String {
    let names: Vec<String> = tools
        .iter()
        .map(|t| format!("\"{}\"", t.name))
        .chain(std::iter::once(format!("\"{NO_TOOL}\"")))
        .collect();
    format!(
        "output EXACTLY this shape, and nothing else -- no prose, no fences, \
         no trailing padding:\n\
         {{\"verdict\": {{\"action\": \"answer|task|search|meta|clarify|chitchat\", \
         \"domain\": \"reminder|note|fact|calendar|email|file|none\", \
         \"door\": \"exact|vector|web|blended|followup\", \
         \"tier\": \"fast|super|ultra\", \"lang\": \"<BCP 47>\", \
         \"mood\": {{\"valence\": 0.0, \"urgency\": 0.0}}, \"confidence\": 0.0}}, \
         \"call\": {{\"tool\": <one of {}>, \"args\": {{...}}}}}}",
        names.join(" | ")
    )
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
            .chat(Role::Route, &messages, None, 400)
            .map_err(|e| tracing::warn!("routing call failed: {e}"))
            .ok()?;
        if std::env::var("BENDER_ROUTE_DEBUG").is_ok() {
            eprintln!("--- routing raw ---\n{}\n---", out.content);
        }
        let v = salvage_json(&out.content)?;
        // the verdict must parse; a proposal that does not is simply dropped,
        // since a half-understood call is worse than none
        let verdict: Verdict = match v.get("verdict") {
            Some(raw) => match serde_json::from_value(raw.clone()) {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!("routing verdict unparseable: {e}");
                    return None;
                }
            },
            None => {
                tracing::warn!("routing response had no verdict object");
                return None;
            }
        };
        let call: Option<prism::types::ToolCall> = v
            .get("call")
            .filter(|c| !c.is_null())
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .filter(|c: &prism::types::ToolCall| c.tool != NO_TOOL);
        Some(Routing { verdict, call })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug that cost a live eval run: a truncated response must not
    /// silently degrade into one of its own sub-objects.
    #[test]
    fn a_truncated_response_is_repaired_not_misread() {
        // exactly the shape the router produced: complete call, verdict cut
        // off by the token ceiling, then whitespace padding
        let truncated = "{\n \"call\": {\"tool\": \"reminder.create\", \
                         \"args\": {\"about\": \"stretch\"}},\n \
                         \"verdict\": {\"action\": \"task\", \"lang\": \"ru\"\n\n\n   ";
        let v = salvage_json(truncated).expect("should be repaired");
        assert_eq!(v["call"]["tool"], "reminder.create", "the call survived");
        assert_eq!(v["verdict"]["lang"], "ru", "and so did the verdict");

        // a dangling key with no value is dropped, not invented
        let dangling = "{\"a\": 1, \"b\": {\"c\": 2}, \"d\"";
        let v = salvage_json(dangling).expect("should be repaired");
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"]["c"], 2);
        assert!(v.get("d").is_none());
    }

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
