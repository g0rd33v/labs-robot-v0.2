//! Serper search + the fetch->READ loop (sec 6a): SERP via Serper, page
//! fetch with a size cap and naive text extraction. Every crossing is
//! boundary-logged; everything inbound is untrusted-by-origin -- fetched
//! text is data, never instructions (sec 7a injection defense).

use crate::gateway::BoundarySink;
use crate::HubError;
use std::time::Duration;
use trust::boundary::{self, Crossing, Direction};

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub title: String,
    pub link: String,
    pub snippet: String,
}

pub struct Research {
    agent: ureq::Agent,
    serper_key: Option<String>,
    boundary: Option<BoundarySink>,
}

impl Research {
    pub fn new(serper_key: Option<String>, boundary: Option<BoundarySink>) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_millis(3000))
                .timeout(Duration::from_millis(12_000))
                .build(),
            serper_key,
            boundary,
        }
    }

    pub fn can_search(&self) -> bool {
        self.serper_key.is_some()
    }

    fn log(&self, direction: Direction, counterparty: &str, purpose: &str, payload: &[u8]) {
        if let Some(sink) = &self.boundary {
            if let Ok(conn) = sink.lock() {
                let _ = boundary::append(
                    &conn,
                    &Crossing {
                        direction,
                        channel: "web".into(),
                        counterparty: counterparty.into(),
                        purpose: purpose.into(),
                        categories: "web-content".into(),
                        payload_hash: trust::ids::sha256_hex(payload),
                        size: payload.len() as i64,
                        trust_tag: if direction == Direction::Out {
                            "granted".into()
                        } else {
                            "untrusted".into()
                        },
                    },
                );
            }
        }
    }

    /// Top organic results from Serper.
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>, HubError> {
        let key = self
            .serper_key
            .as_ref()
            .ok_or_else(|| HubError::Gateway("serper key not configured".into()))?;
        let body = serde_json::json!({ "q": query, "num": 5 });
        self.log(
            Direction::Out,
            "google.serper.dev",
            "web-search",
            body.to_string().as_bytes(),
        );
        let resp: serde_json::Value = self
            .agent
            .post("https://google.serper.dev/search")
            .set("x-api-key", key)
            .set("content-type", "application/json")
            .send_json(body)
            .map_err(|e| HubError::Gateway(format!("serper: {e}")))?
            .into_json()
            .map_err(|e| HubError::Gateway(format!("serper json: {e}")))?;
        let resp_str = resp.to_string();
        self.log(
            Direction::In,
            "google.serper.dev",
            "web-search",
            resp_str.as_bytes(),
        );
        let hits = resp["organic"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|h| {
                        Some(SearchHit {
                            title: h["title"].as_str()?.to_string(),
                            link: h["link"].as_str()?.to_string(),
                            snippet: h["snippet"].as_str().unwrap_or("").to_string(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(hits)
    }

    /// Fetch a page and extract readable text (capped).
    pub fn fetch_text(&self, url: &str, cap: usize) -> Result<String, HubError> {
        self.log(Direction::Out, url, "web-fetch", url.as_bytes());
        let resp = self
            .agent
            .get(url)
            .set("user-agent", "bender-robot/0.2 (personal assistant)")
            .call()
            .map_err(|e| HubError::Gateway(format!("fetch {url}: {e}")))?;
        let mut html = String::new();
        // cap the read: 800KB of html is plenty for READ
        resp.into_reader()
            .take(800 * 1024)
            .read_to_string(&mut html)
            .map_err(|e| HubError::Gateway(format!("read {url}: {e}")))?;
        self.log(Direction::In, url, "web-fetch", html.as_bytes());
        let mut text = extract_text(&html);
        text.truncate(cap);
        Ok(text)
    }
}

use std::io::Read;

/// Naive but dependable text extraction: drop script/style/head blocks,
/// strip tags, collapse whitespace. No new dependencies for the MVP.
pub fn extract_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 4);
    let lower = html.to_lowercase();
    let mut pos = 0;
    let bytes = html.as_bytes();
    let mut in_tag = false;
    let mut skip_until: Option<&'static str> = None;

    while pos < bytes.len() {
        if let Some(end_tag) = skip_until {
            match lower[pos..].find(end_tag) {
                Some(rel) => {
                    pos += rel + end_tag.len();
                    skip_until = None;
                }
                None => break,
            }
            continue;
        }
        let c = bytes[pos] as char;
        if in_tag {
            if c == '>' {
                in_tag = false;
            }
            pos += 1;
            continue;
        }
        if c == '<' {
            for (open, close) in [
                ("<script", "</script>"),
                ("<style", "</style>"),
                ("<head", "</head>"),
                ("<noscript", "</noscript>"),
            ] {
                if lower[pos..].starts_with(open) {
                    skip_until = Some(close);
                    break;
                }
            }
            if skip_until.is_none() {
                in_tag = true;
                pos += 1;
            }
            continue;
        }
        out.push(c);
        pos += 1;
    }
    // collapse whitespace
    let mut collapsed = String::with_capacity(out.len());
    let mut last_ws = false;
    for c in out.chars() {
        if c.is_whitespace() {
            if !last_ws {
                collapsed.push(' ');
            }
            last_ws = true;
        } else {
            collapsed.push(c);
            last_ws = false;
        }
    }
    collapsed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_strips_scripts_and_tags() {
        let html = "<html><head><title>x</title><style>body{}</style></head>\
                    <body><script>evil()</script><h1>Hello</h1>\
                    <p>World &amp; friends</p></body></html>";
        let text = extract_text(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("evil"));
        assert!(!text.contains("body{}"));
    }
}
