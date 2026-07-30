//! Telegram surface transport (long-polling, arch sec 6a) -- lives in hub
//! because hub is the sole external gate: every getUpdates/sendMessage
//! crossing is boundary-logged. The bot token stays in memory only.

use crate::gateway::BoundarySink;
use crate::HubError;
use std::time::Duration;
use trust::boundary::{self, Crossing, Direction};

pub struct Telegram {
    agent: ureq::Agent,
    token: String,
    boundary: Option<BoundarySink>,
}

#[derive(Debug, Clone)]
pub struct TgUpdate {
    pub update_id: i64,
    pub chat_id: i64,
    pub text: String,
}

impl Telegram {
    pub fn new(token: String, boundary: Option<BoundarySink>) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_millis(5000))
                .build(),
            token,
            boundary,
        }
    }

    /// Law #3, enforced: a failed log write aborts the crossing.
    fn log(&self, direction: Direction, purpose: &str, payload: &[u8]) -> Result<(), HubError> {
        let Some(sink) = &self.boundary else {
            return Ok(());
        };
        let conn = sink
            .lock()
            .map_err(|_| HubError::Gateway("boundary log unavailable (poisoned)".into()))?;
        boundary::append(
            &conn,
            &Crossing {
                direction,
                channel: "telegram".into(),
                counterparty: "api.telegram.org".into(),
                purpose: purpose.into(),
                categories: "message".into(),
                payload_hash: trust::ids::sha256_hex(payload),
                size: payload.len() as i64,
                trust_tag: if direction == Direction::Out {
                    "granted".into()
                } else {
                    "untrusted".into()
                },
            },
        )
        .map_err(|e| HubError::Gateway(format!("boundary log write failed: {e}")))?;
        Ok(())
    }

    /// Long-poll for updates (25s server-side wait).
    pub fn get_updates(&self, offset: i64) -> Result<Vec<TgUpdate>, HubError> {
        self.log(Direction::Out, "telegram-poll", format!("offset={offset}").as_bytes())?;
        let resp: serde_json::Value = self
            .agent
            .get(&format!(
                "https://api.telegram.org/bot{}/getUpdates",
                self.token
            ))
            .query("timeout", "25")
            .query("offset", &offset.to_string())
            .timeout(Duration::from_secs(35))
            .call()
            .map_err(|e| HubError::Gateway(format!("telegram getUpdates: {e}")))?
            .into_json()
            .map_err(|e| HubError::Gateway(format!("telegram json: {e}")))?;
        let resp_str = resp.to_string();
        self.log(Direction::In, "telegram-poll", resp_str.as_bytes())?;
        let updates = resp["result"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|u| {
                        Some(TgUpdate {
                            update_id: u["update_id"].as_i64()?,
                            chat_id: u["message"]["chat"]["id"].as_i64()?,
                            text: u["message"]["text"].as_str().unwrap_or("").to_string(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(updates)
    }

    pub fn send_message(&self, chat_id: i64, text: &str) -> Result<(), HubError> {
        let body = serde_json::json!({ "chat_id": chat_id, "text": text });
        self.log(Direction::Out, "telegram-send", body.to_string().as_bytes())?;
        let resp = self
            .agent
            .post(&format!(
                "https://api.telegram.org/bot{}/sendMessage",
                self.token
            ))
            .timeout(Duration::from_secs(15))
            .send_json(body)
            .map_err(|e| HubError::Gateway(format!("telegram send: {e}")))?;
        let text_resp = resp.into_string().unwrap_or_default();
        self.log(Direction::In, "telegram-send", text_resp.as_bytes())?;
        Ok(())
    }
}
