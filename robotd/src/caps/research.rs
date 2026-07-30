//! Web research: SERP -> fetch/READ -> cited answer, with the fetched
//! material framed as untrusted data (sec 7a). Its output is an utterance;
//! the fetched pages are evidence of what was READ, not of what the model
//! concluded from them.

use super::{failed, note_evidence, spoke, Capability, Ctx};
use crate::prompts::research_system_prompt;
use hub::gateway::{Msg, Role};
use prism::types::{Effect, Evidence, Outcome};
use prism::PrismError;

pub struct WebResearch;

impl Capability for WebResearch {
    fn name(&self) -> &'static str {
        "web.research"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let query = args["query"].as_str().unwrap_or("");
        let (Some(gw), Some(rs)) = (&ctx.services.gateway, &ctx.services.research) else {
            let why = if ctx.services.gateway.is_none() {
                ctx.say("brain_offline", &[])
            } else {
                ctx.say("search_offline", &[])
            };
            return spoke(note_evidence("search-offline"), why);
        };

        let hits = match rs.search(query) {
            Ok(h) if !h.is_empty() => h,
            Ok(_) => {
                return spoke(note_evidence("no-results"), ctx.say("search_empty", &[]))
            }
            Err(e) => {
                return failed(
                    note_evidence("search-failure"),
                    ctx.say("search_failed", &[("error", &e.to_string())]),
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
                let mut sources = format!("\n\n{}", ctx.say("sources_header", &[]));
                for (i, h) in hits.iter().take(3).enumerate() {
                    sources.push_str(&format!("\n{}. {}", i + 1, h.link));
                }
                ev.push(super::model_evidence(&out.model, &out.content));
                spoke(ev, format!("{}{sources}", out.content))
            }
            Err(e) => failed(
                note_evidence("provider-failure"),
                ctx.say("research_failed", &[("error", &e.to_string())]),
            ),
        }
    }
}
