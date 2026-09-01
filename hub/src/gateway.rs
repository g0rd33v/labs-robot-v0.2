//! The Intelligence Gateway (arch sec 6): one canonical path for every
//! model call. Akita's paid-for laws are law here: hard connect+total
//! timeouts on every call (the doorman may be wrong, it may never hang),
//! a fallback chain per role, hedging for the verdict class, and every
//! crossing in the Boundary Log with the exact model named -- receipts can
//! then name what acted.

use crate::HubError;
use rusqlite::Connection;
use serde::Serialize;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use trust::boundary::{self, Crossing, Direction};

pub type BoundarySink = Arc<Mutex<Connection>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Verdict,
    /// Verification. A LAW, not a preference (arch sec 5): verification
    /// never runs on the model that generated. Generators grade their own
    /// work too generously, and a skeptical standalone evaluator is
    /// tractable where self-criticism is not.
    Evaluator,
    /// Routing: classify AND choose a tool. A different class of call from
    /// the bare verdict -- a prompt carrying the whole catalog, and an
    /// answer that has to pick correctly -- so it gets its own seat and its
    /// own budget rather than borrowing the doorman's three seconds.
    Route,
    Answer,
    Extract,
    Super,
    Ultra,
    Vision,
    Stt,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Verdict => "verdict",
            Role::Route => "route",
            Role::Evaluator => "evaluator",
            Role::Answer => "answer",
            Role::Extract => "extract",
            Role::Super => "super",
            Role::Ultra => "ultra",
            Role::Vision => "vision",
            Role::Stt => "stt",
        }
    }
}

/// The cast (arch sec 6a / Q28): roles are permanent, models rotate.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct Cast {
    pub verdict: String,
    pub route: String,
    pub evaluator: String,
    pub answer: String,
    pub extract: String,
    #[serde(rename = "super")]
    pub super_: String,
    pub ultra: String,
    pub vision: String,
    pub stt: String,
}

impl Default for Cast {
    fn default() -> Self {
        Self {
            verdict: "google/gemma-4-26b-a4b-it".into(),
            route: "google/gemma-4-31b-it".into(),
            // Q26: gemma-4-26b-a4b, deliberately NOT the 31b generator
            evaluator: "google/gemma-4-26b-a4b-it".into(),
            answer: "google/gemma-4-31b-it".into(),
            extract: "google/gemma-4-31b-it".into(),
            super_: "nvidia/nemotron-3-super-120b-a12b".into(),
            ultra: "nvidia/nemotron-3-ultra-550b-a55b".into(),
            vision: "qwen/qwen3-vl-30b-a3b-instruct".into(),
            stt: "nvidia/parakeet-tdt-0.6b-v3".into(),
        }
    }
}

impl Cast {
    pub fn model_for(&self, role: Role) -> &str {
        match role {
            Role::Verdict => &self.verdict,
            Role::Route => &self.route,
            Role::Evaluator => &self.evaluator,
            Role::Answer => &self.answer,
            Role::Extract => &self.extract,
            Role::Super => &self.super_,
            Role::Ultra => &self.ultra,
            Role::Vision => &self.vision,
            Role::Stt => &self.stt,
        }
    }

    /// The fallback chain per role: same schema, next seat (13d).
    fn chain(&self, role: Role) -> Vec<String> {
        match role {
            Role::Verdict => vec![self.verdict.clone(), self.answer.clone()],
            Role::Route => vec![self.route.clone(), self.answer.clone()],
            // no fallback to the answer seat: falling back to the generator
            // would break the very law this role exists to keep
            Role::Evaluator => vec![self.evaluator.clone()],
            Role::Answer => vec![self.answer.clone(), self.super_.clone()],
            Role::Extract => vec![self.extract.clone(), self.answer.clone()],
            Role::Super => vec![self.super_.clone(), self.answer.clone()],
            Role::Ultra => vec![self.ultra.clone(), self.super_.clone()],
            Role::Vision => vec![self.vision.clone()],
            Role::Stt => vec![self.stt.clone()],
        }
    }
}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub base_url: String,
    /// verdict class: first attempt ceiling, then one retry ceiling (sec 6a)
    pub verdict_timeout_ms: u64,
    pub verdict_retry_timeout_ms: u64,
    /// routing class: the catalog makes the prompt an order of magnitude
    /// bigger than a bare verdict, and a three-second ceiling sized for the
    /// latter simply times the former out -- which looks exactly like a
    /// model that cannot route
    pub route_timeout_ms: u64,
    /// the evaluator class. It borrowed the doorman's retry budget at first
    /// and timed out on every call -- a check that never runs is worse than
    /// no check, because the journal fills with "unverified" and nobody
    /// reads why.
    pub evaluator_timeout_ms: u64,
    pub answer_timeout_ms: u64,
    /// hedge deadline for the verdict class (Q19)
    pub hedge_after_ms: u64,
    /// Which endpoints OpenRouter should try, in order of preference.
    ///
    /// This used to be `sort: "latency"`, which scattered every call. Two
    /// things went wrong with that, and the second one was the expensive
    /// one.
    ///
    /// **The sort does not deliver low latency.** Measured directly, same
    /// 6K prefix, gemma-4-31b, two runs: `sort: latency` gave p50
    /// 1487/2148 ms but p95 18443/3411 ms, picking endpoints that took
    /// 19.5 s and 37.5 s while pinned DeepInfra in the same window was
    /// steady at 792-2664 ms. It sorts on stale global statistics.
    ///
    /// **And a scattered call can never warm a prompt cache**, because
    /// caches are per-endpoint. This is what the 32.5% lifetime cache-hit
    /// on the route seat actually was: the average of occasionally landing
    /// on the same endpoint twice in a row. Pinned, the real routing
    /// prefix reaches **88.7%** after ten turns and climbs as it goes --
    /// with `ttft[route]` p50 771 ms, p95 976 ms, against a 7-day
    /// baseline of p50 2882 ms and p95 9094 ms.
    ///
    /// So the cache-stable layout the speed tranche built was never the
    /// problem; it had nothing to accumulate on. Provider choice is what
    /// lets it pay.
    ///
    /// Fallbacks stay ON, so this is a preference and not a dependency:
    /// if DeepInfra is down or rate-limited the call still goes through,
    /// at the cost of a cold cache for as long as it is away. Empty sends
    /// no preference at all.
    pub providers: Vec<String>,
    /// hedge deadline for the ROUTING class, which is a different animal:
    /// the doorman's 2.5s was sized for a one-line classification, and a
    /// routing call carrying the whole catalog normally takes longer than
    /// that. Left at the doorman's figure it fired on essentially every
    /// non-English turn -- doubling the cost of the most expensive call in
    /// the turn to protect against a tail that is far further out.
    pub route_hedge_after_ms: u64,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            base_url: "https://openrouter.ai/api/v1".into(),
            verdict_timeout_ms: 3000,
            verdict_retry_timeout_ms: 5000,
            route_timeout_ms: 30_000,
            evaluator_timeout_ms: 20_000,
            answer_timeout_ms: 45_000,
            hedge_after_ms: 2500,
            route_hedge_after_ms: 8000,
            providers: vec!["DeepInfra".into(), "Together".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Msg {
    pub role: &'static str,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ChatOut {
    pub content: String,
    /// the exact model that produced the content -- for the receipt
    pub model: String,
    /// what the call cost, when the provider said (sec 2b: measured, not
    /// estimated). None on transports that report nothing.
    pub usage: Option<CallUsage>,
}

/// One call's arithmetic, as the provider reported it.
#[derive(Debug, Clone, Default)]
pub struct CallUsage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// tokens the provider served from its prompt cache -- the number that
    /// makes sec 6's cache-stable layout a measurement instead of a hope
    pub cached_tokens: i64,
    /// the provider's own charge in USD, when it reports one
    pub cost_usd: Option<f64>,
    pub latency_ms: i64,
}

impl CallUsage {
    /// Parse the OpenAI-compatible `usage` object; OpenRouter adds `cost`
    /// when the request asks for accounting.
    pub fn from_response(resp: &serde_json::Value, latency_ms: i64) -> Option<CallUsage> {
        let u = resp.get("usage")?;
        Some(CallUsage {
            prompt_tokens: u["prompt_tokens"].as_i64().unwrap_or(0),
            completion_tokens: u["completion_tokens"].as_i64().unwrap_or(0),
            cached_tokens: u["prompt_tokens_details"]["cached_tokens"]
                .as_i64()
                .unwrap_or(0),
            cost_usd: u["cost"].as_f64(),
            latency_ms,
        })
    }
}

/// The raw transport, mockable for tests.
pub trait ChatApi: Send + Sync {
    fn post_chat(
        &self,
        model: &str,
        body: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, HubError>;

    /// Streaming chat (sec 2c #1). Calls `on_chunk` per content delta and
    /// returns the ASSEMBLED response in the same shape `post_chat`
    /// returns, so everything downstream -- usage parsing, logging,
    /// metering -- is one code path. The default degrades to the plain
    /// call and emits the whole content as one chunk: a transport that
    /// cannot stream still works, it just has nothing to say early.
    fn post_chat_stream(
        &self,
        model: &str,
        body: &serde_json::Value,
        timeout_ms: u64,
        on_chunk: &mut dyn FnMut(&str),
    ) -> Result<serde_json::Value, HubError> {
        let resp = self.post_chat(model, body, timeout_ms)?;
        if let Some(c) = resp["choices"][0]["message"]["content"].as_str() {
            on_chunk(c);
        }
        Ok(resp)
    }

    /// The router's audio endpoint (multipart). Default: unsupported.
    fn post_transcription(
        &self,
        _model: &str,
        _bytes: &[u8],
        _format: &str,
        _timeout_ms: u64,
    ) -> Result<serde_json::Value, HubError> {
        Err(HubError::Gateway("transcription transport unavailable".into()))
    }
}

/// OpenRouter over ureq. The API key lives in memory only, injected into
/// the Authorization header per call -- never on disk, never in a prompt.
pub struct UreqApi {
    agent: ureq::Agent,
    api_key: String,
    base_url: String,
}

impl UreqApi {
    pub fn new(api_key: String, base_url: String) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_millis(3000))
                .build(),
            api_key,
            base_url,
        }
    }
}

impl ChatApi for UreqApi {
    fn post_chat_stream(
        &self,
        model: &str,
        body: &serde_json::Value,
        timeout_ms: u64,
        on_chunk: &mut dyn FnMut(&str),
    ) -> Result<serde_json::Value, HubError> {
        use std::io::{BufRead, BufReader};
        let mut body = body.clone();
        body["model"] = serde_json::Value::String(model.to_string());
        body["stream"] = serde_json::Value::Bool(true);
        let resp = self
            .agent
            .post(&format!("{}/chat/completions", self.base_url))
            .set("authorization", &format!("Bearer {}", self.api_key))
            .set("content-type", "application/json")
            .timeout(Duration::from_millis(timeout_ms))
            .send_json(body)
            .map_err(|e| HubError::Gateway(format!("{model}: {e}")))?;

        let mut content = String::new();
        let mut model_used = model.to_string();
        let mut usage = serde_json::Value::Null;
        let reader = BufReader::new(resp.into_reader());
        for line in reader.lines() {
            let line = line.map_err(|e| HubError::Gateway(format!("{model}: stream: {e}")))?;
            let Some(data) = line.strip_prefix("data: ") else { continue };
            if data.trim() == "[DONE]" {
                break;
            }
            let Ok(chunk) = serde_json::from_str::<serde_json::Value>(data) else {
                continue; // a malformed keep-alive is not a reason to lose the stream
            };
            if let Some(m) = chunk["model"].as_str() {
                model_used = m.to_string();
            }
            // usage rides the final chunk when accounting was requested
            if !chunk["usage"].is_null() {
                usage = chunk["usage"].clone();
            }
            if let Some(delta) = chunk["choices"][0]["delta"]["content"].as_str() {
                if !delta.is_empty() {
                    content.push_str(delta);
                    on_chunk(delta);
                }
            }
        }
        if content.is_empty() {
            return Err(HubError::Gateway(format!("{model}: stream produced no content")));
        }
        // the same shape post_chat returns, so downstream stays one path
        Ok(serde_json::json!({
            "model": model_used,
            "usage": usage,
            "choices": [{ "message": { "content": content } }],
        }))
    }

    fn post_chat(
        &self,
        model: &str,
        body: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, HubError> {
        let mut body = body.clone();
        body["model"] = serde_json::Value::String(model.to_string());
        let resp = self
            .agent
            .post(&format!("{}/chat/completions", self.base_url))
            .set("authorization", &format!("Bearer {}", self.api_key))
            .set("content-type", "application/json")
            .timeout(Duration::from_millis(timeout_ms))
            .send_json(body)
            .map_err(|e| HubError::Gateway(format!("{model}: {e}")))?;
        resp.into_json()
            .map_err(|e| HubError::Gateway(format!("{model}: bad json: {e}")))
    }

    fn post_transcription(
        &self,
        model: &str,
        bytes: &[u8],
        format: &str,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, HubError> {
        // hand-rolled multipart: no new dependency for one endpoint
        let boundary = format!("----bender{}", trust::ids::random_hex(12));
        let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 512);
        let part = |name: &str, value: &str| {
            format!(
                "--{boundary}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
        };
        body.extend_from_slice(part("model", model).as_bytes());
        body.extend_from_slice(
            format!(
                "--{boundary}\r\ncontent-disposition: form-data; name=\"file\"; \
                 filename=\"audio.{format}\"\r\ncontent-type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let resp = self
            .agent
            .post(&format!("{}/audio/transcriptions", self.base_url))
            .set("authorization", &format!("Bearer {}", self.api_key))
            .set(
                "content-type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .timeout(Duration::from_millis(timeout_ms))
            .send_bytes(&body)
            .map_err(|e| HubError::Gateway(format!("{model} (audio): {e}")))?;
        resp.into_json()
            .map_err(|e| HubError::Gateway(format!("{model} (audio): bad json: {e}")))
    }
}

pub struct ModelGateway {
    api: Arc<dyn ChatApi>,
    pub cast: Cast,
    cfg: GatewayConfig,
    boundary: Option<BoundarySink>,
}

impl ModelGateway {
    pub fn new(
        api: Arc<dyn ChatApi>,
        cast: Cast,
        cfg: GatewayConfig,
        boundary: Option<BoundarySink>,
    ) -> Self {
        Self {
            api,
            cast,
            cfg,
            boundary,
        }
    }

    /// Record a crossing. Law #3 is enforced, not attempted: if the log
    /// write fails, the caller must abort rather than let bytes move
    /// unrecorded. Previously this swallowed both a poisoned lock and a
    /// failed INSERT, so the Boundary Log silently degraded to best-effort.
    fn log(
        &self,
        direction: Direction,
        model: &str,
        purpose: &str,
        payload: &str,
    ) -> Result<(), HubError> {
        let Some(sink) = &self.boundary else {
            return Ok(()); // no sink wired: this gateway is not instance traffic
        };
        let conn = sink
            .lock()
            .map_err(|_| HubError::Gateway("boundary log unavailable (poisoned)".into()))?;
        boundary::append(
            &conn,
            &Crossing {
                direction,
                channel: "model-api".into(),
                counterparty: format!("openrouter.ai/{model}"),
                purpose: purpose.into(),
                categories: "prompt-context".into(),
                payload_hash: trust::ids::sha256_hex(payload.as_bytes()),
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

    fn attempt(
        &self,
        model: &str,
        role: Role,
        body: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<ChatOut, HubError> {
        let body_str = body.to_string();
        // no crossing record, no crossing: the log write gates the call
        self.log(Direction::Out, model, role.as_str(), &body_str)?;
        let started = std::time::Instant::now();
        let resp = match self.api.post_chat(model, body, timeout_ms) {
            Ok(r) => r,
            Err(e) => {
                // the failure itself is an inbound crossing worth recording
                let note = format!("error: {e}");
                self.log(Direction::In, model, role.as_str(), &note)?;
                return Err(e);
            }
        };
        let latency_ms = started.elapsed().as_millis() as i64;
        let resp_str = resp.to_string();
        self.log(Direction::In, model, role.as_str(), &resp_str)?;
        let content = resp["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| HubError::Gateway(format!("{model}: no content in response")))?
            .to_string();
        let model_used = resp["model"].as_str().unwrap_or(model).to_string();
        let usage = CallUsage::from_response(&resp, latency_ms);
        self.meter(role, &model_used, usage.as_ref(), latency_ms);
        Ok(ChatOut {
            content,
            model: model_used,
            usage,
        })
    }

    /// The meter (sec 2b). Best-effort by design, unlike the boundary log:
    /// a lost arithmetic row costs a cent of accounting, and failing the
    /// person's turn over it would price bookkeeping above service.
    fn meter(&self, role: Role, model: &str, usage: Option<&CallUsage>, latency_ms: i64) {
        self.meter_full(role, model, usage, latency_ms, None);
    }

    fn meter_full(
        &self,
        role: Role,
        model: &str,
        usage: Option<&CallUsage>,
        latency_ms: i64,
        first_token_ms: Option<i64>,
    ) {
        let Some(sink) = &self.boundary else { return };
        let Ok(conn) = sink.lock() else { return };
        let (p, c, cached, cost) = match usage {
            Some(u) => (u.prompt_tokens, u.completion_tokens, u.cached_tokens, u.cost_usd),
            None => (0, 0, 0, None),
        };
        let _ = conn.execute(
            "INSERT INTO model_calls(ts, role, model, prompt_tokens, completion_tokens, \
             cached_tokens, cost_usd, latency_ms, first_token_ms) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![
                trust::ids::ts_ms(),
                role.as_str(),
                model,
                p,
                c,
                cached,
                cost,
                latency_ms,
                first_token_ms
            ],
        );
    }

    /// As `chat_stream`, but the callback sees the ACCUMULATED text and may
    /// stop caring early. Used by routing, where the only field the answer
    /// path needs is the first one emitted -- see `hub::early`.
    ///
    /// The call still runs to completion: the full verdict is wanted for
    /// the journal and for tool arguments. What changes is that someone
    /// downstream can act on the decision ~2 s before the object closes.
    pub fn chat_stream_watched(
        &self,
        role: Role,
        messages: &[Msg],
        max_tokens: u32,
        temperature: f32,
        on_partial: &mut dyn FnMut(&str),
    ) -> Result<ChatOut, HubError> {
        let mut acc = String::new();
        let mut relay = |delta: &str| {
            acc.push_str(delta);
            on_partial(&acc);
        };
        self.chat_stream(role, messages, max_tokens, temperature, &mut relay)
    }

    /// Streaming chat (sec 2c #1): tokens reach `on_token` as they arrive,
    /// and time-to-first-token is finally a number rather than a promise.
    ///
    /// Same chain walk, same law-3 discipline as `chat`: the request gates
    /// on its outbound crossing, and the INBOUND crossing is written from
    /// the assembled response after the stream ends -- the log records
    /// what crossed, and what crossed is the whole reply, not each packet.
    /// No hedging: a stream that has produced tokens cannot be raced
    /// without discarding words already shown to a person.
    pub fn chat_stream(
        &self,
        role: Role,
        messages: &[Msg],
        max_tokens: u32,
        temperature: f32,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ChatOut, HubError> {
        let mut body = serde_json::json!({
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": temperature,
            "usage": { "include": true },
        });
        if !self.cfg.providers.is_empty() {
            body["provider"] = serde_json::json!({
                "order": self.cfg.providers,
                "allow_fallbacks": true,
            });
        }
        let chain = self.cast.chain(role);
        let mut last_err = HubError::Gateway("empty chain".into());
        for model in chain.iter() {
            let body_str = body.to_string();
            self.log(Direction::Out, model, role.as_str(), &body_str)?;
            let started = std::time::Instant::now();
            let mut first_token: Option<i64> = None;
            let mut relay = |delta: &str| {
                if first_token.is_none() {
                    first_token = Some(started.elapsed().as_millis() as i64);
                }
                on_token(delta);
            };
            match self
                .api
                .post_chat_stream(model, &body, self.cfg.answer_timeout_ms, &mut relay)
            {
                Ok(resp) => {
                    let latency_ms = started.elapsed().as_millis() as i64;
                    let resp_str = resp.to_string();
                    self.log(Direction::In, model, role.as_str(), &resp_str)?;
                    let content = resp["choices"][0]["message"]["content"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let model_used = resp["model"].as_str().unwrap_or(model).to_string();
                    let usage = CallUsage::from_response(&resp, latency_ms);
                    self.meter_full(role, &model_used, usage.as_ref(), latency_ms, first_token);
                    return Ok(ChatOut {
                        content,
                        model: model_used,
                        usage,
                    });
                }
                Err(e) => {
                    let note = format!("error: {e}");
                    self.log(Direction::In, model, role.as_str(), &note)?;
                    tracing::warn!("gateway stream {role:?} ({model}) failed: {e}");
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    /// One chat completion through the chain. `schema` requests structured
    /// output (used by the verdict call, Q16).
    pub fn chat(
        &self,
        role: Role,
        messages: &[Msg],
        schema: Option<serde_json::Value>,
        max_tokens: u32,
    ) -> Result<ChatOut, HubError> {
        self.chat_at(role, messages, schema, max_tokens, 0.4)
    }

    /// As `chat`, with an explicit temperature. Anything reasoning over
    /// untrusted input runs at 0.0: sampling variance there is not
    /// creativity, it is a security property left to chance.
    pub fn chat_at(
        &self,
        role: Role,
        messages: &[Msg],
        schema: Option<serde_json::Value>,
        max_tokens: u32,
        temperature: f32,
    ) -> Result<ChatOut, HubError> {
        let mut body = serde_json::json!({
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": temperature,
            // sec 2b: ask the router for its own accounting, so cost is the
            // provider's number rather than our price-table estimate
            "usage": { "include": true },
        });
        if !self.cfg.providers.is_empty() {
            body["provider"] = serde_json::json!({
                "order": self.cfg.providers,
                "allow_fallbacks": true,
            });
        }
        if let Some(s) = schema {
            // Strict constrained decoding cannot express a tool call's `args`,
            // which is a different shape per tool and so has to stay a
            // free-form object. Asked to satisfy it strictly, providers pad
            // the output with whitespace until the token ceiling -- which
            // arrives as a truncated response, or as a timeout, and looks
            // exactly like a model that cannot route. Routing therefore sends
            // the schema as guidance and relies on the layers that were going
            // to check it anyway: salvage, repair, and registry validation.
            let strict = role != Role::Route;
            body["response_format"] = serde_json::json!({
                "type": "json_schema",
                "json_schema": { "name": "verdict", "strict": strict, "schema": s }
            });
        }

        let chain = self.cast.chain(role);
        let mut last_err = HubError::Gateway("empty chain".into());
        for (i, model) in chain.iter().enumerate() {
            let timeout = match (role, i) {
                (Role::Verdict, 0) => self.cfg.verdict_timeout_ms,
                (Role::Verdict, _) => self.cfg.verdict_retry_timeout_ms,
                (Role::Route, _) => self.cfg.route_timeout_ms,
                (Role::Evaluator, _) => self.cfg.evaluator_timeout_ms,
                _ => self.cfg.answer_timeout_ms,
            };
            let result = if matches!(role, Role::Verdict | Role::Route) && i == 0 {
                self.hedged_attempt(model, role, &body, timeout)
            } else {
                self.attempt(model, role, &body, timeout)
            };
            match result {
                Ok(out) => return Ok(out),
                Err(e) => {
                    tracing::warn!("gateway {role:?} attempt {i} ({model}) failed: {e}");
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    /// The STT seat (sec 6a: parakeet via the router's audio endpoint):
    /// multipart to /audio/transcriptions first, the input_audio chat shape
    /// as the fallback. Boundary-logged like every other call.
    pub fn transcribe(&self, bytes: &[u8], format: &str) -> Result<ChatOut, HubError> {
        let model = self.cast.stt.clone();
        self.log(
            Direction::Out,
            &model,
            "stt",
            &format!("audio:{format}:{}bytes", bytes.len()),
        )?;
        match self
            .api
            .post_transcription(&model, bytes, format, self.cfg.answer_timeout_ms)
        {
            Ok(resp) => {
                let resp_str = resp.to_string();
                self.log(Direction::In, &model, "stt", &resp_str)?;
                let text = resp["text"]
                    .as_str()
                    .ok_or_else(|| HubError::Gateway(format!("{model}: no transcript")))?
                    .to_string();
                return Ok(ChatOut {
                    content: text,
                    model,
                    usage: None,
                });
            }
            Err(e) => tracing::warn!("audio endpoint failed, trying chat shape: {e}"),
        }
        // fallback: input_audio content part through chat completions
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let body = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text",
                     "text": "transcribe this audio verbatim in its original language. output only the transcript."},
                    {"type": "input_audio", "input_audio": {"data": b64, "format": format}}
                ]
            }],
            "max_tokens": 800,
            "temperature": 0.0,
        });
        self.attempt(&model, Role::Stt, &body, self.cfg.answer_timeout_ms)
    }

    /// Q19 hedging for the verdict class: if the primary hasn't answered by
    /// the deadline, fire a schema-identical second request; first responder
    /// wins. Both calls appear in the Boundary Log.
    fn hedged_attempt(
        &self,
        model: &str,
        role: Role,
        body: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<ChatOut, HubError> {
        let hedge_after = match role {
            Role::Route => self.cfg.route_hedge_after_ms,
            _ => self.cfg.hedge_after_ms,
        };
        let (tx, rx) = mpsc::channel::<Result<ChatOut, HubError>>();
        let fire = |tx: mpsc::Sender<Result<ChatOut, HubError>>| -> Result<(), HubError> {
            let api = self.api.clone();
            let model_owned = model.to_string();
            let body = body.clone();
            let body_str = body.to_string();
            // gate the request on its own crossing record (law #3)
            self.log(Direction::Out, model, role.as_str(), &body_str)?;
            let boundary = self.boundary.clone();
            let role_name = role.as_str();
            std::thread::spawn(move || {
                let model = model_owned;
                let started = std::time::Instant::now();
                let result = api.post_chat(&model, &body, timeout_ms).and_then(|resp| {
                    // the response is unusable unless its crossing is
                    // recorded: no record, no bytes
                    if let Some(sink) = &boundary {
                        let s = resp.to_string();
                        let conn = sink.lock().map_err(|_| {
                            HubError::Gateway("boundary log unavailable (poisoned)".into())
                        })?;
                        boundary::append(
                            &conn,
                            &Crossing {
                                direction: Direction::In,
                                channel: "model-api".into(),
                                counterparty: format!("openrouter.ai/{model}"),
                                purpose: role_name.into(),
                                categories: "prompt-context".into(),
                                payload_hash: trust::ids::sha256_hex(s.as_bytes()),
                                size: s.len() as i64,
                                trust_tag: "untrusted".into(),
                            },
                        )
                        .map_err(|e| {
                            HubError::Gateway(format!("boundary log write failed: {e}"))
                        })?;
                    }
                    let content = resp["choices"][0]["message"]["content"]
                        .as_str()
                        .ok_or_else(|| HubError::Gateway(format!("{model}: no content")))?
                        .to_string();
                    let model_used = resp["model"].as_str().unwrap_or(&model).to_string();
                    // the meter, same as the unhedged path -- routing goes
                    // THROUGH here, and an unmetered router hides the most
                    // frequent seat on the bill
                    let latency_ms = started.elapsed().as_millis() as i64;
                    let usage = CallUsage::from_response(&resp, latency_ms);
                    if let Some(sink) = &boundary {
                        if let Ok(conn) = sink.lock() {
                            let (pt, ct, cached, cost) = match &usage {
                                Some(u) => {
                                    (u.prompt_tokens, u.completion_tokens, u.cached_tokens, u.cost_usd)
                                }
                                None => (0, 0, 0, None),
                            };
                            let _ = conn.execute(
                                "INSERT INTO model_calls(ts, role, model, prompt_tokens, \
                                 completion_tokens, cached_tokens, cost_usd, latency_ms) \
                                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                                rusqlite::params![
                                    trust::ids::ts_ms(),
                                    role_name,
                                    model_used,
                                    pt,
                                    ct,
                                    cached,
                                    cost,
                                    latency_ms
                                ],
                            );
                        }
                    }
                    Ok(ChatOut {
                        content,
                        model: model_used,
                        usage,
                    })
                });
                let _ = tx.send(result);
            });
            Ok(())
        };

        // Race semantics: the FIRST SUCCESS wins, not the first responder.
        // Previously an error from the primary (typically its own timeout,
        // 500ms after the hedge was fired) was returned immediately and
        // discarded an in-flight hedge -- losing in exactly the case
        // hedging exists for. A fast failure now hedges at once instead of
        // waiting out the deadline.
        fire(tx.clone())?;
        let mut hedged = false;
        let mut heard = 0usize;
        let mut last_err: Option<HubError> = None;
        let mut wait = Duration::from_millis(hedge_after);

        loop {
            match rx.recv_timeout(wait) {
                Ok(Ok(out)) => return Ok(out),
                Ok(Err(e)) => {
                    heard += 1;
                    last_err = Some(e);
                    if !hedged {
                        fire(tx.clone())?;
                        hedged = true;
                        wait = Duration::from_millis(timeout_ms);
                        continue;
                    }
                    if heard >= 2 {
                        break;
                    }
                    wait = Duration::from_millis(timeout_ms);
                }
                Err(_) => {
                    if !hedged {
                        fire(tx.clone())?;
                        hedged = true;
                        wait = Duration::from_millis(timeout_ms);
                        continue;
                    }
                    break; // both attempts exhausted their time
                }
            }
        }
        Err(last_err
            .unwrap_or_else(|| HubError::Gateway(format!("{model}: hedged attempts timed out"))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ScriptedApi {
        calls: AtomicUsize,
        /// per-call results: Err = fail this call, Ok(text) = respond
        script: Vec<Result<String, ()>>,
    }

    impl ChatApi for ScriptedApi {
        fn post_chat(
            &self,
            model: &str,
            _body: &serde_json::Value,
            _timeout_ms: u64,
        ) -> Result<serde_json::Value, HubError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            match self.script.get(n) {
                Some(Ok(text)) => Ok(serde_json::json!({
                    "model": model,
                    "choices": [{"message": {"content": text}}]
                })),
                _ => Err(HubError::Gateway("scripted failure".into())),
            }
        }
    }

    fn gw(script: Vec<Result<String, ()>>) -> ModelGateway {
        ModelGateway::new(
            Arc::new(ScriptedApi {
                calls: AtomicUsize::new(0),
                script,
            }),
            Cast::default(),
            GatewayConfig {
                hedge_after_ms: 50,
                route_hedge_after_ms: 50,
                evaluator_timeout_ms: 50,
                ..Default::default()
            },
            None,
        )
    }

    #[test]
    fn primary_success_names_the_model() {
        let g = gw(vec![Ok("hello".into())]);
        let out = g
            .chat(Role::Answer, &[Msg { role: "user", content: "hi".into() }], None, 100)
            .unwrap();
        assert_eq!(out.content, "hello");
        assert_eq!(out.model, "google/gemma-4-31b-it");
    }

    #[test]
    fn fallback_chain_kicks_in() {
        // answer chain: gemma-31b fails -> nemotron super answers
        let g = gw(vec![Err(()), Ok("super answer".into())]);
        let out = g
            .chat(Role::Answer, &[Msg { role: "user", content: "hi".into() }], None, 100)
            .unwrap();
        assert_eq!(out.content, "super answer");
        assert_eq!(out.model, "nvidia/nemotron-3-super-120b-a12b");
    }

    #[test]
    fn exhausted_chain_is_an_error_not_a_hang() {
        let g = gw(vec![Err(()), Err(()), Err(())]);
        assert!(g
            .chat(Role::Answer, &[Msg { role: "user", content: "hi".into() }], None, 100)
            .is_err());
    }
}
