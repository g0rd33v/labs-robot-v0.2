//! Web research: SERP -> fetch/READ -> cited answer, with the fetched
//! material framed as untrusted data (sec 7a). Its output is an utterance;
//! the fetched pages are evidence of what was READ, not of what the model
//! concluded from them.

use super::{failed, note_evidence, spoke, Capability, Ctx};
use crate::prompts::research_system_prompt;
use hub::gateway::{Msg, Role};
use prism::types::{Effect, Evidence, Outcome, Rendering};
use prism::PrismError;

pub struct WebResearch;

impl Capability for WebResearch {
    fn name(&self) -> &'static str {
        "web.research"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Search the open web, read the best sources, and answer from them with \
         citations. Use for anything current, local, or outside what you \
         already know -- news, prices, weather, opening hours, recent events."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to look up. Write it as a good search \
                                    query; keep proper nouns and place names \
                                    exactly as the person wrote them."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if q.trim().is_empty() {
            return Err("query is empty".into());
        }
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let query = args["query"].as_str().unwrap_or("");
        let (Some(gw), Some(rs)) = (&ctx.services.gateway, &ctx.services.research) else {
            let say = if ctx.services.gateway.is_none() {
                Rendering::bare("brain_offline")
            } else {
                Rendering::bare("search_offline")
            };
            return super::declined("web.research", say);
        };

        let hits = match rs.search(query) {
            Ok(h) if !h.is_empty() => h,
            Ok(_) => {
                return super::declined("web.research", Rendering::bare("search_empty"))
            }
            Err(e) => {
                return failed(
                    note_evidence("search-failure"),
                    format!("web search failed: {e}"),
                    Rendering::new(
                        "search_failed",
                        serde_json::json!({ "error": e.to_string() }),
                    ),
                )
            }
        };

        let mut ev: Vec<Evidence> = vec![];
        let mut context = String::new();
        for (i, h) in hits.iter().take(3).enumerate() {
            context.push_str(&format!(
                "SOURCE {}: {} ({})\nsnippet: {}\n\n",
                i + 1,
                h.title,
                h.link,
                h.snippet
            ));
        }
        // fetch->READ the top pages (capped, allowlisted, boundary-logged)
        for (i, h) in hits.iter().take(2).enumerate() {
            match rs.fetch_text(&h.link, 4000) {
                Ok(text) => {
                    context.push_str(&format!("PAGE {} ({}):\n{}\n\n", i + 1, h.link, text));
                    ev.push(Evidence {
                        kind: "web".into(),
                        provider: "fetch".into(),
                        external_id: h.link.clone(),
                        hash: trust::ids::sha256_hex(text.as_bytes()),
                        ts: trust::ids::ts_ms(),
                    });
                }
                Err(e) => tracing::warn!("fetch skipped for {}: {e}", h.link),
            }
        }

        let messages = [
            Msg {
                role: "system",
                content: research_system_prompt(&context),
            },
            Msg {
                role: "user",
                content: query.into(),
            },
        ];
        // temperature 0.0: variance over untrusted input is not creativity,
        // it is a security property left to chance
        match gw.chat_at(Role::Answer, &messages, None, 1200, 0.0) {
            Ok(out) => {
                // the citations are data, so they are rendered, not written
                let mut sources = String::from("\n\nsources:");
                for (i, h) in hits.iter().take(3).enumerate() {
                    sources.push_str(&format!("\n{}. {}", i + 1, h.link));
                }
                ev.push(super::model_evidence(&out.model, &out.content));
                spoke(ev, format!("{}{sources}", out.content))
            }
            Err(e) => failed(
                note_evidence("provider-failure"),
                format!("read sources but the model call failed: {e}"),
                Rendering::new(
                    "research_failed",
                    serde_json::json!({ "error": e.to_string() }),
                ),
            ),
        }
    }
}
