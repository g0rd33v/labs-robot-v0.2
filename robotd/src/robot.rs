//! RobotCore: the composition of the organs behind the `surfaces::Robot`
//! trait. One core, many cells -- every principal commands their own
//! encrypted partition (law #2); the owner commands the Robot.
//!
//! Capabilities live in `caps`; prompts in `prompts`. This module owns cell
//! lifecycle, the turn, media intake, and the dashboard view.

use crate::caps::{Instance, Policy, Registry, Services};
use anyhow::{anyhow, bail, Context};
use prism::verdict::{FallbackVerdict, VerdictProvider};
use prism::types::Rendering;
use prism::{Cell, Envelope, Evidence, Outcome, TurnDeps};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use surfaces::dash::DashData;
use tokio::sync::broadcast;
use trust::boundary::{self, Crossing, Direction};
use trust::keys::KeyChain;
use trust::schema;

pub(crate) fn ensure_cell_key(
    core: &Connection,
    keys: &KeyChain,
    cell_id: &str,
) -> anyhow::Result<[u8; 32]> {
    let existing: Option<(Vec<u8>, Vec<u8>)> = core
        .query_row(
            "SELECT wrapped_dek, nonce FROM cell_keys WHERE cell_id = ?1",
            params![cell_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((wrapped, nonce)) = existing {
        return Ok(keys.unwrap_dek(&nonce, &wrapped)?);
    }
    let dek = KeyChain::new_dek();
    let (nonce, wrapped) = keys.wrap_dek(&dek)?;
    core.execute(
        "INSERT INTO cell_keys(cell_id, wrapped_dek, nonce, created_at) VALUES (?1,?2,?3,?4)",
        params![cell_id, wrapped, nonce, trust::ids::ts_ms()],
    )?;
    Ok(dek)
}

// ------------------------------------------------------------- robot core

#[derive(Clone)]
pub struct CellHandle {
    /// Lockable in short bursts -- never held across a model call.
    pub cell: Cell,
    pub vault: Arc<mind::vault::MediaVault>,
}

pub struct RobotCore {
    pub owner_principal: i64,
    /// This installation. Shared `robot_id`, distinct instance.
    pub instance_id: String,
    /// shared with the hub gateway as its boundary-log sink
    pub core: Arc<Mutex<Connection>>,
    cells: Mutex<HashMap<i64, CellHandle>>,
    /// Held across a cell's first open so two threads cannot both build one.
    open_gate: Mutex<()>,
    keys: KeyChain,
    data_dir: PathBuf,
    pub embedder: Option<Arc<hub::Embedder>>,
    pub gateway: Option<Arc<hub::ModelGateway>>,
    pub research: Option<Arc<hub::Research>>,
    pub google: Option<Arc<hub::google::Google>>,
    pub oauth_app: Option<Arc<hub::oauth::App>>,
    /// Authorization attempts waiting for their callback. In memory on
    /// purpose: the PKCE verifier is the proof of possession for a code, it
    /// is useful for ten minutes, and writing it to disk would give it a
    /// lifetime it has no business having.
    pub pending_auth: Arc<Mutex<HashMap<String, hub::oauth::Attempt>>>,
    pub ultra_daily_cap: u32,
    /// Q26's sample rate for routine turns; acting turns are always checked.
    pub verify_percent: u32,
    /// Capabilities the owner has asked to approve by hand.
    pub approval_required: Vec<String>,
    pub public_base: String,
    pub robot_name: String,
    pub started_at: i64,
    events: broadcast::Sender<i64>,
    /// live draft text while an answer streams: (principal, accumulated).
    /// Display-only -- the canonical reply still lands through the outbox
    /// with its receipt; a draft nobody receives costs nothing.
    drafts: broadcast::Sender<(i64, String)>,
}

impl RobotCore {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner_principal: i64,
        core: Arc<Mutex<Connection>>,
        keys: KeyChain,
        data_dir: PathBuf,
        embedder: Option<Arc<hub::Embedder>>,
        gateway: Option<Arc<hub::ModelGateway>>,
        research: Option<Arc<hub::Research>>,
        ultra_daily_cap: u32,
        verify_percent: u32,
        approval_required: Vec<String>,
        public_base: String,
        robot_name: String,
        instance_id: String,
        google: Option<Arc<hub::google::Google>>,
        oauth_app: Option<Arc<hub::oauth::App>>,
    ) -> Self {
        Self {
            owner_principal,
            instance_id,
            core,
            cells: Mutex::new(HashMap::new()),
            open_gate: Mutex::new(()),
            keys,
            data_dir,
            embedder,
            gateway,
            research,
            google,
            oauth_app,
            pending_auth: Arc::new(Mutex::new(HashMap::new())),
            ultra_daily_cap,
            verify_percent,
            approval_required,
            public_base,
            robot_name,
            started_at: trust::ids::ts_ms(),
            events: broadcast::channel(64).0,
            drafts: broadcast::channel(256).0,
        }
    }

    /// Open (or fetch) a principal's cell: their own encrypted file, their
    /// own vault. Lazily opened, cached for the process lifetime.
    ///
    /// Serialized by `open_gate`: without it two threads that both miss the
    /// cache (a member joining and immediately messaging, while the 5s
    /// scheduler tick also opens their cell) each build a separate
    /// `Connection` to the same SQLCipher file behind a separate Mutex,
    /// breaking the one-writer-per-cell invariant and racing the schema
    /// batches against each other.
    /// Open one cell's database directly, for the sync lane, which works
    /// across instances rather than through the per-principal handles.
    pub fn open_cell_db(
        &self,
        cell_id: &str,
        dek: &[u8; 32],
    ) -> anyhow::Result<prism::Cell> {
        let conn = trust::cells::open_encrypted(
            &self.data_dir.join("cells").join(format!("{cell_id}.db")),
            dek,
        )?;
        prism::init_cell_schema(&conn)?;
        mind::init_cell_schema(&conn)?;
        soul::init_cell_schema(&conn)?;
        Ok(prism::Cell::new(conn))
    }

    pub fn media_dir(&self, cell_id: &str) -> std::path::PathBuf {
        self.data_dir.join("media").join(cell_id)
    }

    pub fn keychain(&self) -> KeyChain {
        self.keys.clone()
    }

    /// Check a reply against its own receipt, on a seat that did not write
    /// it (Q26). Journals the verdict; never blocks delivery, because an
    /// evaluator that can stop the robot talking is a new way for the robot
    /// to go silent.
    /// Runs OFF the turn's critical path (sec 2c #6): it verified the same
    /// way for months while BLOCKING the reply on an evaluator round trip
    /// -- measured p50 832ms, on every acting turn. The check is about the
    /// record, not the delivery; the journal row lands the same either
    /// way, and the person gets their reply an evaluator-call earlier.
    fn expression_verify(&self, cell: &Cell, out: &prism::TurnOutput) {
        let Some(gw) = self.gateway.clone() else { return };
        let acted = out
            .receipt
            .claims
            .iter()
            .any(|c| c.evidence.iter().any(|e| e.kind == "row"));
        if !hub::evaluator::should_verify(&out.intent_id, acted, self.verify_percent) {
            return;
        }
        let claims: Vec<String> = out.receipt.claims.iter().map(|c| c.claim.clone()).collect();
        let (cell, intent_id, reply) = (cell.clone(), out.intent_id.clone(), out.reply.clone());
        std::thread::spawn(move || {
        let verdict = hub::evaluator::expression_supported(&gw, &reply, &claims);
        let payload = match &verdict {
            Some(v) => serde_json::json!({
                "supported": v.supported, "why": v.why, "sampled": !acted,
            }),
            // unavailable is recorded as UNVERIFIED, never as passed: an
            // evaluator that silently approves when broken is worse than
            // none, because it leaves a record saying someone looked
            None => serde_json::json!({ "supported": null, "why": "evaluator unavailable" }),
        };
        let _ = cell.with(|c| {
            prism::journal::step(c, &intent_id, "expression.verified", &payload.to_string(), None)
        });
        if verdict.as_ref().is_some_and(|v| !v.supported) {
            tracing::warn!(
                "expression-verify: reply for {intent_id} claims more than its receipt"
            );
        }
        });
    }

    pub fn cell(&self, principal: i64) -> anyhow::Result<CellHandle> {
        if let Some(h) = self
            .cells
            .lock()
            .map_err(|_| anyhow!("cells lock poisoned"))?
            .get(&principal)
        {
            return Ok(h.clone());
        }
        let _gate = self
            .open_gate
            .lock()
            .map_err(|_| anyhow!("cell open gate poisoned"))?;
        // re-check: another thread may have opened it while we waited
        if let Some(h) = self
            .cells
            .lock()
            .map_err(|_| anyhow!("cells lock poisoned"))?
            .get(&principal)
        {
            return Ok(h.clone());
        }
        let (cell_id, dek) = {
            let core = self.core.lock().map_err(|_| anyhow!("core lock poisoned"))?;
            let cell_id: String = core
                .query_row(
                    "SELECT cell_id FROM principals WHERE id = ?1 AND status = 'active'",
                    params![principal],
                    |r| r.get(0),
                )
                .optional()?
                .with_context(|| format!("no active principal {principal}"))?;
            let dek = ensure_cell_key(&core, &self.keys, &cell_id)?;
            (cell_id, dek)
        };
        let conn = trust::cells::open_encrypted(
            &self.data_dir.join("cells").join(format!("{cell_id}.db")),
            &dek,
        )?;
        prism::init_cell_schema(&conn)?;
        mind::init_cell_schema(&conn)?;
        soul::init_cell_schema(&conn)?;
        let vault = mind::vault::MediaVault::new(
            self.data_dir.join("media").join(&cell_id),
            trust::keys::derive_key(&dek, b"media"),
        );
        let handle = CellHandle {
            cell: Cell::new(conn),
            vault: Arc::new(vault),
        };
        self.cells
            .lock()
            .map_err(|_| anyhow!("cells lock poisoned"))?
            .insert(principal, handle.clone());
        Ok(handle)
    }

    pub fn principals_active(&self) -> anyhow::Result<Vec<i64>> {
        let core = self.core.lock().map_err(|_| anyhow!("core lock poisoned"))?;
        let mut stmt = core.prepare("SELECT id FROM principals WHERE status = 'active'")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    /// Put an operational notice into the owner's chat, journaled and
    /// receipted like any other action. Used by background lanes: a lane
    /// that fails silently is worse than one that does not run.
    /// Finish an OAuth sign-in: the callback arrived with a code.
    ///
    /// The attempt is REMOVED from the pending map before anything else, so
    /// a state value is single-use whatever happens next -- a replayed
    /// callback finds nothing and is refused, which is the property that
    /// makes an interceptable loopback URL safe to use.
    pub fn complete_google_auth(&self, state: &str, code: &str) -> anyhow::Result<String> {
        let attempt = {
            let mut pending = self
                .pending_auth
                .lock()
                .map_err(|_| anyhow!("pending sign-ins unavailable"))?;
            pending.remove(state)
        };
        let attempt = attempt.ok_or_else(|| anyhow!("unknown or already-used sign-in"))?;
        hub::oauth::check_callback(&attempt, state, trust::ids::ts_ms())?;

        let (Some(google), Some(app)) = (&self.google, &self.oauth_app) else {
            return Err(anyhow!("no google client configured"));
        };
        let now = trust::ids::ts_ms();
        let tokens = google.exchange(&hub::oauth::code_exchange_form(app, &attempt, code))?;
        let account = google.whoami(&tokens.access_token)?;

        // Record the scopes GOOGLE granted, not the ones we asked for. A
        // person may untick one on the consent screen, and believing we
        // have a permission we do not is how a capability fails with an
        // opaque 403 instead of saying what is missing.
        let granted: Vec<String> = tokens
            .scope
            .as_deref()
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_else(|| attempt.scopes.clone());

        let handle = self.cell(attempt.principal)?;
        handle.cell.with(|c| {
            mind::connections::save(
                c,
                &attempt.provider,
                &account,
                &granted,
                &tokens.access_token,
                tokens.refresh_token.as_deref(),
                tokens.expires_at(now),
            )
            .map_err(|e| prism::PrismError::Capability(e.to_string()))
        })?;
        self.notify(attempt.principal);
        Ok(account)
    }

    pub fn tell_owner(&self, text: &str) -> anyhow::Result<()> {
        let handle = self.cell(self.owner_principal)?;
        let cell = &handle.cell;
        let intent_id = trust::ids::new_id("int");
        let open_json = serde_json::json!({
            "system": "ops.notice",
            "principal_id": self.owner_principal,
        })
        .to_string();
        cell.with(|c| prism::journal::intent_open(c, &intent_id, &open_json))?;
        let outcome = Outcome::attested(
            trust::ids::new_id("pstep"),
            vec![Evidence {
                kind: "deterministic".into(),
                provider: "ops".into(),
                external_id: "notice".into(),
                hash: trust::ids::sha256_hex(text.as_bytes()),
                ts: trust::ids::ts_ms(),
            }],
            format!("delivered an operational notice ({} chars)", text.chars().count()),
            prism::types::Rendering::bare("ops_notice"),
        );
        let outcome_json = serde_json::to_string(&outcome)?;
        cell.with(|c| prism::journal::step(c, &intent_id, "outcome", &outcome_json, None))?;
        let receipt = prism::lifecycle::build_receipt(&intent_id, &[outcome]);
        let receipt = cell.with(|c| prism::receipts::store(c, &receipt))?;
        cell.with(|c| Ok(mind::record_message(c, "out", "chat", text)))??;
        cell.with(|c| prism::journal::intent_close(c, &intent_id, receipt.status.as_str()))?;
        self.boundary_crossing(Direction::Out, "ops", text)?;
        self.notify(self.owner_principal);
        Ok(())
    }

    pub fn notify(&self, principal: i64) {
        let _ = self.events.send(principal);
    }

    /// The capability registry for this robot (also used by boot-time replay).
    ///
    /// Takes the acting cell's vault: it is keyed from that cell's DEK, so
    /// there is no instance-wide one to hold on `self`.
    pub fn router(&self, vault: Option<Arc<mind::vault::MediaVault>>) -> Registry {
        self.router_with_draft(vault, None)
    }

    /// As `router`, with a live draft sink for the turn's streamed tokens.
    pub fn router_with_draft(
        &self,
        vault: Option<Arc<mind::vault::MediaVault>>,
        draft: Option<crate::caps::DraftSink>,
    ) -> Registry {
        self.router_full(vault, draft, None)
    }

    pub fn router_full(
        &self,
        vault: Option<Arc<mind::vault::MediaVault>>,
        draft: Option<crate::caps::DraftSink>,
        premix_embedding: Option<Arc<std::sync::OnceLock<Option<Vec<f32>>>>>,
    ) -> Registry {
        self.router_all(vault, draft, premix_embedding, None)
    }

    #[allow(clippy::type_complexity)]
    pub fn router_all(
        &self,
        vault: Option<Arc<mind::vault::MediaVault>>,
        draft: Option<crate::caps::DraftSink>,
        premix_embedding: Option<Arc<std::sync::OnceLock<Option<Vec<f32>>>>>,
        warm_answer: Option<Arc<Mutex<Option<crate::caps::WarmAnswer>>>>,
    ) -> Registry {
        let mut reg = Registry::new(
            Services {
                embedder: self.embedder.clone(),
                gateway: self.gateway.clone(),
                research: self.research.clone(),
                draft,
                premix_embedding,
                warm_answer,
                vault,
                google: self.google.clone(),
                oauth_app: self.oauth_app.clone(),
                pending_auth: Some(self.pending_auth.clone()),
            },
            Policy {
                ultra_daily_cap: self.ultra_daily_cap,
            },
            Instance {
                core: Some(self.core.clone()),
                owner_principal: self.owner_principal,
                public_base: self.public_base.clone(),
                instance_id: self.instance_id.clone(),
            },
        );
        reg.approval_policy = self.approval_required.clone();
        reg
    }

    fn boundary_crossing(
        &self,
        direction: Direction,
        surface: &str,
        payload: &str,
    ) -> anyhow::Result<()> {
        let core = self.core.lock().map_err(|_| anyhow!("core lock poisoned"))?;
        boundary::append(
            &core,
            &Crossing {
                direction,
                channel: surface.into(),
                counterparty: if surface == "telegram" {
                    "api.telegram.org".into()
                } else {
                    "local-web".into()
                },
                purpose: "conversation".into(),
                categories: "message".into(),
                payload_hash: trust::ids::sha256_hex(payload.as_bytes()),
                size: payload.len() as i64,
                // Trust is a property of where the bytes came from, not of
                // who the session belongs to. Inbound Telegram text arrives
                // over a third-party platform from the open world; only the
                // local session on this machine is owner-trusted. Everything
                // the robot itself emits is `granted` (it left under its own
                // authority).
                trust_tag: match (direction, surface) {
                    (Direction::Out, _) => "granted".into(),
                    (Direction::In, "chat") | (Direction::In, "upload") => "owner".into(),
                    (Direction::In, _) => "untrusted".into(),
                },
            },
        )?;
        Ok(())
    }

    /// One governed turn for any principal on any surface.
    pub fn turn(&self, principal: i64, text: String, surface: &str) -> anyhow::Result<String> {
        self.boundary_crossing(Direction::In, surface, &text)?;
        let handle = self.cell(principal)?;
        let reply = {
            let cell = &handle.cell;
            let msg_id = cell.with(|c| Ok(mind::record_message(c, "in", surface, &text)))??;
            let env = Envelope {
                surface: surface.into(),
                principal_id: principal,
                modality: "text".into(),
                content: text,
                ts: trust::ids::ts_ms(),
                device_trust: "session".into(),
                source_msg_id: Some(msg_id),
            };
            let drafts = self.drafts.clone();
            let draft_sink: crate::caps::DraftSink = Arc::new(move |text: &str| {
                let _ = drafts.send((principal, text.to_string()));
            });
            // sec 2c #2: the embedding runs on its own thread while the
            // routing model call runs -- max(), not sum(). ~30-50ms of the
            // serial path, and the pattern the speculative context build
            // will extend.
            let premix = Arc::new(std::sync::OnceLock::new());
            if let Some(embedder) = self.embedder.clone() {
                let slot = premix.clone();
                let text = env.content.clone();
                std::thread::spawn(move || {
                    let _ = slot.set(embedder.embed_query(&text).ok());
                });
            }
            // sec 2c: the answer starts when the router COMMITS to "no
            // tool", not when the verdict closes -- about two seconds
            // earlier. Nothing is guessed: `tool` is written once, so a
            // commitment cannot be walked back, and the capability adopts
            // this instead of calling again.
            let warm: Arc<Mutex<Option<crate::caps::WarmAnswer>>> = Arc::new(Mutex::new(None));
            let router = self.router_all(
                Some(handle.vault.clone()),
                Some(draft_sink.clone()),
                Some(premix),
                Some(warm.clone()),
            );
            let verdicts: Box<dyn VerdictProvider> = match &self.gateway {
                Some(g) => Box::new(hub::GatewayVerdicts { gateway: g.clone() }),
                None => Box::new(FallbackVerdict),
            };
            let speak = crate::render::Speak {
                gateway: self.gateway.clone(),
                voice: cell_voice(cell),
            };
            // the person's standing rules (sec 4.6), compiled once per turn
            // and handed to the router -- prism carries the block without
            // knowing where rules live
            let standing =
                cell.with(|c| mind::instructions::context_block(c).map_err(crate::caps::mind_err))?;
            let early_gw = self.gateway.clone();
            let early_query = env.content.clone();
            let early_warm = warm.clone();
            let early_sink = draft_sink.clone();
            let on_early = move |tool: Option<&str>| {
                // only the answer path: a named tool needs arguments that
                // have not arrived, and there is nothing to start early
                if tool.is_some() {
                    return;
                }
                let (Some(gw), Ok(mut slot)) = (early_gw.clone(), early_warm.lock()) else {
                    return;
                };
                if slot.is_some() {
                    return; // already running
                }
                let q = early_query.clone();
                let sink = early_sink.clone();
                let messages = vec![hub::gateway::Msg {
                    role: "user",
                    content: q.clone(),
                }];
                let handle = std::thread::spawn(move || {
                    let mut acc = String::new();
                    let mut last = 0usize;
                    let mut on_token = |delta: &str| {
                        acc.push_str(delta);
                        // the FIRST fragment goes out immediately: waiting
                        // for a full chunk adds ~400ms to the number the
                        // person actually experiences, and time-to-first-
                        // token is the whole point. Throttle after that.
                        if last == 0 || acc.len() - last >= 48 {
                            last = acc.len();
                            sink(&acc);
                        }
                    };
                    gw.chat_stream(
                        hub::gateway::Role::Answer,
                        &messages,
                        1200,
                        0.4,
                        &mut on_token,
                    )
                });
                *slot = Some(crate::caps::WarmAnswer {
                    query: early_query.clone(),
                    handle,
                });
            };
            let deps = TurnDeps {
                router: &router,
                verdicts: verdicts.as_ref(),
                renderer: &speak,
                crash: None,
                standing,
                on_early: Some(&on_early),
            };
            // the cell is locked only in short bursts inside run_turn; the
            // model call in the middle happens with it free
            // sec 3b.2: if something is parked, this message is most
            // likely the answer to it. Checked BEFORE routing, because a
            // model asked to interpret "yes" with no idea a question is
            // open will happily interpret it as something else.
            // R4.3.1: an open time question is answered before anything
            // else looks at the message -- "2" means the second option,
            // and a router asked to interpret it with no idea a question
            // is open will read it as something else entirely.
            if let Some((about, at_ms)) = crate::caps::reminders::clarify_answer(cell, &env.content)
            {
                crate::caps::reminders::clear_clarify(cell);
                let resolved = format!("remind me at {} {about}", prism::lifecycle::rfc3339(at_ms));
                tracing::debug!("clarify answered -> {resolved}");
                let env = Envelope {
                    content: resolved,
                    ..env.clone()
                };
                let out = prism::run_turn(cell, &env, &deps)?;
                cell.with(|c| prism::outbox::mark(c, &out.reply_effect_id, "sent", None))?;
                cell.with(|c| Ok(mind::record_message(c, "out", surface, &out.reply)))??;
                cell.with(|c| prism::outbox::mark(c, &out.reply_effect_id, "confirmed", None))?;
                self.boundary_crossing(Direction::Out, surface, &out.reply)?;
                self.notify(principal);
                return Ok(out.reply);
            }
            let answered_park = parked_answer(cell, &env.content)?;
            let out = match &answered_park {
                Some((intent, yes)) => prism::approval::respond(cell, intent, *yes, &deps)?
                    .map(Ok)
                    .unwrap_or_else(|| prism::run_turn(cell, &env, &deps))?,
                None => prism::run_turn(cell, &env, &deps)?,
            };
            // the ledger (sec 4.5), kept by the orchestrator because prism
            // cannot depend on mind. An answered park closes its entry with
            // the answer; a turn that ends parked opens one -- so anything
            // waiting on a person is in the ledger, and the reason it
            // stopped waiting is recorded whichever way it went.
            let parked_now = prism::approval::waiting_for(cell, &out.intent_id)?.is_some();
            cell.with(|c| {
                if let Some((intent, yes)) = &answered_park {
                    let (status, why) = if *yes {
                        ("done", "you approved it; it ran")
                    } else {
                        ("declined", "you declined it; nothing ran")
                    };
                    mind::commitments::close(c, intent, status, why).map_err(crate::caps::mind_err)?;
                }
                if parked_now {
                    mind::commitments::open(
                        c,
                        &out.intent_id,
                        &env.content,
                        "approval",
                        "waiting",
                        env.source_msg_id.as_deref(),
                        Some(&out.intent_id),
                        None,
                    )
                    .map_err(crate::caps::mind_err)?;
                }
                Ok(())
            })?;
            // remember what language this person speaks to us in, so the
            // lanes that talk to them when they are not asking -- a
            // reminder firing at 03:00, a backup failure -- do it in their
            // language rather than in English by default
            cell.with(|c| {
                remember_lang(c, &out.lang);
                Ok(())
            })?;
            // `confirmed` must follow the delivery it claims. For the local
            // chat the message store IS the delivery channel (the surface
            // renders from history), so recording it is the confirmation.
            // Telegram confirms on the provider's message_id, which the send
            // path owns -- tracked in BUILD-LOG, not silently conflated.
            // Q26 / sec 5's evaluator-separation LAW: a turn that acted is
            // always checked, and a sample of the rest is. On a different
            // seat than the one that generated -- the whole point is that
            // generators grade their own work too generously.
            self.expression_verify(cell, &out);

            cell.with(|c| prism::outbox::mark(c, &out.reply_effect_id, "sent", None))?;
            cell.with(|c| Ok(mind::record_message(c, "out", surface, &out.reply)))??;
            cell.with(|c| prism::outbox::mark(c, &out.reply_effect_id, "confirmed", None))?;
            out.reply
        };
        self.boundary_crossing(Direction::Out, surface, &reply)?;
        self.notify(principal);
        Ok(reply)
    }
}

/// The language a cell last spoke, remembered in `cell_meta`.
///
/// Kept here rather than in the kernel: prism decides the language, robotd
/// composes and stores. A failure to record it is not a reason to fail the
/// turn -- worst case the next background notice speaks English.
pub fn remember_lang(conn: &Connection, lang: &str) {
    let _ = conn.execute(
        "INSERT INTO cell_meta(key, value) VALUES ('lang', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![lang],
    );
}

/// Is this message an answer to something parked, and which way?
///
/// Deliberately narrow and deterministic: a short, unambiguous yes or no
/// while a step is waiting. Anything longer is a real message and goes
/// through the normal path -- someone who types a paragraph has moved on,
/// and treating it as consent to a parked action would be the worst kind
/// of helpful.
fn parked_answer(cell: &Cell, text: &str) -> anyhow::Result<Option<(String, bool)>> {
    let waiting = prism::approval::waiting(cell)?;
    let Some(p) = waiting.last() else {
        return Ok(None);
    };
    let t = text.trim().to_lowercase();
    let t = t.trim_matches(|c: char| ",.!?".contains(c));
    const YES: [&str; 8] = ["yes", "y", "approve", "approved", "do it", "go ahead", "ok", "okay"];
    const NO: [&str; 6] = ["no", "n", "cancel", "decline", "don't", "stop"];
    if YES.contains(&t) {
        return Ok(Some((p.intent_id.clone(), true)));
    }
    if NO.contains(&t) {
        return Ok(Some((p.intent_id.clone(), false)));
    }
    Ok(None)
}

/// Soul's instruction for this cell, or `None` when nothing needs shaping.
///
/// One place computes it so every path agrees about the voice, and so the
/// "default dial means templates" property has a single home.
pub fn cell_voice(cell: &Cell) -> Option<String> {
    cell.with(|c| {
        let d = soul::dial::load(c).map_err(|e| prism::PrismError::Capability(e.to_string()))?;
        let st =
            soul::stance::get(c).map_err(|e| prism::PrismError::Capability(e.to_string()))?;
        Ok(soul::express::shape(&d, st.as_ref()))
    })
    .ok()
    .flatten()
}

/// The language a cell speaks, for the lanes that talk without being asked.
pub fn cell_lang(cell: &Cell) -> String {
    cell.sql(|c| {
        c.query_row("SELECT value FROM cell_meta WHERE key = 'lang'", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()
    })
    .ok()
    .flatten()
    .unwrap_or_else(|| "en".into())
}

const AUDIO_EXTS: [&str; 8] = ["ogg", "oga", "mp3", "m4a", "wav", "webm", "opus", "flac"];

impl surfaces::Robot for RobotCore {
    fn subscribe_drafts(&self) -> broadcast::Receiver<(i64, String)> {
        self.drafts.subscribe()
    }

    fn complete_google_auth(&self, state: &str, code: &str) -> anyhow::Result<String> {
        RobotCore::complete_google_auth(self, state, code)
    }

    fn tell_owner(&self, text: &str) -> anyhow::Result<()> {
        RobotCore::tell_owner(self, text)
    }

    fn handle_message(&self, principal: i64, text: String) -> anyhow::Result<String> {
        self.turn(principal, text, "chat")
    }

    fn handle_media(
        &self,
        principal: i64,
        filename: String,
        bytes: Vec<u8>,
    ) -> anyhow::Result<String> {
        let filename = urlencoding_decode(&filename);
        let hash_in = trust::ids::sha256_hex(&bytes);
        {
            let core = self.core.lock().map_err(|_| anyhow!("core lock poisoned"))?;
            boundary::append(
                &core,
                &Crossing {
                    direction: Direction::In,
                    channel: "upload".into(),
                    counterparty: "local-web".into(),
                    purpose: "media-upload".into(),
                    categories: "media".into(),
                    payload_hash: hash_in.clone(),
                    size: bytes.len() as i64,
                    trust_tag: "owner".into(),
                },
            )?;
        }
        let handle = self.cell(principal)?;
        let ext = filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_lowercase();

        // store first: the vault keeps the original either way (sec 4a)
        let media_ref = {
            let cell = &handle.cell;
            let media_ref = cell
                .with(|c| Ok(handle.vault.store(c, &bytes, Some(&ext), Some("chat-upload"))))??;
            // the storage act is a receipted system intent
            let intent_id = trust::ids::new_id("int");
            let open_json = serde_json::json!({
                "system": "media.store",
                "filename": filename,
                "hash": media_ref.hash,
                "size": media_ref.size,
            })
            .to_string();
            cell.with(|c| prism::journal::intent_open(c, &intent_id, &open_json))?;
            // a real state transition: the bytes are in the vault, and the
            // content hash is the evidence
            let outcome = Outcome::attested(
                trust::ids::new_id("pstep"),
                vec![Evidence {
                    kind: "row".into(),
                    provider: "vault".into(),
                    external_id: media_ref.hash.clone(),
                    hash: media_ref.hash.clone(),
                    ts: trust::ids::ts_ms(),
                }],
                format!("stored in the vault: {filename}"),
                Rendering::new(
                    "media_stored",
                    serde_json::json!({ "filename": filename }),
                ),
            );
            let outcome_json = serde_json::to_string(&outcome)?;
            cell.with(|c| prism::journal::step(c, &intent_id, "outcome", &outcome_json, None))?;
            let receipt = prism::lifecycle::build_receipt(&intent_id, &[outcome]);
            let receipt = cell.with(|c| prism::receipts::store(c, &receipt))?;
            cell.with(|c| prism::journal::intent_close(c, &intent_id, receipt.status.as_str()))?;
            media_ref
        };

        let is_audio = AUDIO_EXTS.contains(&ext.as_str());
        // true only when `turn()` ran and therefore already recorded + logged
        let mut transcribed = false;
        let reply = if is_audio {
            match &self.gateway {
                Some(gw) => match gw.transcribe(&bytes, &ext) {
                    Ok(out) => {
                        let transcript = out.content.trim().to_string();
                        // the voice note becomes a normal governed turn
                        let answer =
                            self.turn(principal, transcript.clone(), "chat")?;
                        transcribed = true;
                        format!("heard your voice note: \"{transcript}\"\n\n{answer}")
                    }
                    Err(e) => {
                        tracing::warn!("stt failed: {e}");
                        format!(
                            "voice note stored ({} KB, {}...), but transcription failed \
                             honestly: {e}",
                            media_ref.size / 1024,
                            &media_ref.hash[..12]
                        )
                    }
                },
                None => format!(
                    "voice note stored ({} KB, {}...); transcription needs the model \
                     gateway, which is offline.",
                    media_ref.size / 1024,
                    &media_ref.hash[..12]
                ),
            }
        } else {
            format!(
                "stored in your vault: {filename} ({} KB, content-addressed {}...) -- \
                 it stays encrypted beside your cell.",
                media_ref.size.max(1024) / 1024,
                &media_ref.hash[..12]
            )
        };

        // The reply must land in the message store so history/SSE deliver
        // it, and must be boundary-logged on the way out (law #3).
        //
        // `turn()` already recorded and logged its own reply for the
        // transcribed path, so only the wrapper text is new there; every
        // other path (non-audio, transcription failure, gateway offline)
        // produces text that has no other record at all. Previously the
        // transcription-failure branch recorded nothing, so the user saw a
        // completed upload and absolutely no output.
        if !transcribed {
            handle
                .cell
                .with(|c| Ok(mind::record_message(c, "out", "chat", &reply)))??;
        }
        self.boundary_crossing(Direction::Out, "upload", &reply)?;
        self.notify(principal);
        Ok(reply)
    }

    fn history(&self, principal: i64, after_ts: i64) -> anyhow::Result<Vec<(i64, String, String)>> {
        let handle = self.cell(principal)?;
        Ok(handle
            .cell
            .with(|c| Ok(mind::messages_after(c, after_ts, 200)))??)
    }

    fn accept_invite(&self, token: &str) -> anyhow::Result<(i64, String)> {
        let token_hash = trust::ids::sha256_hex(token.as_bytes());
        let (principal, name) = {
            let core = self.core.lock().map_err(|_| anyhow!("core lock poisoned"))?;
            let used_by: Option<Option<i64>> = core
                .query_row(
                    "SELECT used_by FROM invites WHERE token_hash = ?1",
                    params![token_hash],
                    |r| r.get(0),
                )
                .optional()?;
            match used_by {
                None => bail!("unknown invite"),
                Some(Some(_)) => bail!("invite already used"),
                Some(None) => {}
            }
            let n: i64 = core.query_row(
                "SELECT count(*) FROM principals WHERE kind = 'member'",
                [],
                |r| r.get(0),
            )?;
            let name = format!("member-{}", n + 1);
            let cell_id = format!("member{}-{}", n + 1, trust::ids::random_hex(3));
            core.execute(
                "INSERT INTO principals(kind, display_name, cell_id, created_at) \
                 VALUES ('member', ?1, ?2, ?3)",
                params![name, cell_id, trust::ids::ts_ms()],
            )?;
            let principal = core.last_insert_rowid();
            core.execute(
                "UPDATE invites SET used_by = ?1 WHERE token_hash = ?2",
                params![principal, token_hash],
            )?;
            schema::core_journal(
                &core,
                "member.join",
                &serde_json::json!({ "principal": principal, "name": name }).to_string(),
            )?;
            (principal, name)
        };
        // create their sealed cell eagerly
        self.cell(principal)?;
        Ok((principal, name))
    }

    fn subscribe(&self) -> broadcast::Receiver<i64> {
        self.events.subscribe()
    }

    fn dashboard(&self, principal: i64) -> anyhow::Result<DashData> {
        if principal != self.owner_principal {
            bail!("dashboard is owner-only");
        }
        let mut d = DashData {
            robot_name: self.robot_name.clone(),
            started_at: self.started_at,
            now: trust::ids::ts_ms(),
            gateway_online: self.gateway.is_some(),
            search_online: self.research.as_ref().map(|r| r.can_search()).unwrap_or(false),
            embedder_online: self.embedder.is_some(),
            ..Default::default()
        };
        if let Some(gw) = &self.gateway {
            d.cast_answer = gw.cast.answer.clone();
            d.cast_verdict = gw.cast.verdict.clone();
        }
        {
            let core = self.core.lock().map_err(|_| anyhow!("core lock poisoned"))?;
            d.robot_id = schema::meta_get(&core, "robot_id")?.unwrap_or_default();
            let mut stmt = core.prepare(
                "SELECT id, kind, display_name, status FROM principals ORDER BY id",
            )?;
            d.principals = stmt
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            d.boundary_count = trust::boundary::count(&core)?;
            d.boundary_chain_ok = trust::boundary::verify_chain(&core)?;
            let mut stmt = core.prepare(
                "SELECT ts, direction, channel, counterparty, purpose, size \
                 FROM boundary_log ORDER BY seq DESC LIMIT 50",
            )?;
            d.boundary = stmt
                .query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
        }
        // panel 8: the whole cast, and what each seat actually did today
        if let Some(gw) = &self.gateway {
            d.cast = vec![
                ("verdict".into(), gw.cast.verdict.clone()),
                ("route".into(), gw.cast.route.clone()),
                ("answer".into(), gw.cast.answer.clone()),
                ("super".into(), gw.cast.super_.clone()),
                ("ultra".into(), gw.cast.ultra.clone()),
                ("evaluator".into(), gw.cast.evaluator.clone()),
                ("extract".into(), gw.cast.extract.clone()),
                ("vision".into(), gw.cast.vision.clone()),
                ("stt".into(), gw.cast.stt.clone()),
            ];
        }
        {
            let core = self.core.lock().map_err(|_| anyhow!("core lock poisoned"))?;
            let day_ago = trust::ids::ts_ms() - 24 * 60 * 60 * 1000;
            let mut stmt = core.prepare(
                "SELECT role, count(*),                         100.0 * coalesce(sum(cached_tokens),0) / max(sum(prompt_tokens), 1),                         coalesce(sum(cost_usd), 0.0),                         CAST(avg(latency_ms) AS INTEGER),                         CAST(avg(first_token_ms) AS INTEGER)                  FROM model_calls WHERE ts > ?1 GROUP BY role ORDER BY role",
            )?;
            d.meter = stmt
                .query_map(params![day_ago], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            // spec 4.7.3.3: spend today vs the cap, on Overview. The meter
            // has had the number since the cost tranche; Overview simply
            // never asked it.
            let midnight = {
                use chrono::{Local, TimeZone};
                let now = Local::now();
                now.date_naive()
                    .and_hms_opt(0, 0, 0)
                    .and_then(|d| Local.from_local_datetime(&d).earliest())
                    .map(|d| d.timestamp_millis())
                    .unwrap_or(day_ago)
            };
            d.spend_today_usd = core
                .query_row(
                    "SELECT coalesce(sum(cost_usd), 0.0) FROM model_calls WHERE ts > ?1",
                    params![midnight],
                    |r| r.get(0),
                )
                .unwrap_or(0.0);
        }
        d.ultra_cap = self.ultra_daily_cap;
        d.instance_id = self.instance_id.clone();
        d.version = env!("CARGO_PKG_VERSION").into();
        // panel 7: connector states, described without a single secret
        d.hub.push((
            "openrouter".into(),
            if self.gateway.is_some() { "online (key in memory)".into() } else { "no key -- floor only".into() },
        ));
        d.hub.push((
            "serper".into(),
            if d.search_online { "online".into() } else { "no key -- web search off".into() },
        ));
        d.hub.push((
            "google".into(),
            if self.oauth_app.is_none() { "no client configured".into() } else { "client configured".into() },
        ));
        {
            let handle = self.cell(self.owner_principal)?;
            handle.cell.with(|c| {
                d.message_count = mind::message_count(c).unwrap_or(0);
                d.fact_count = mind::facts::count_active(c).unwrap_or(0);
                d.active_reminders = mind::reminders::count_active(c).unwrap_or(0);
                d.facts = mind::facts::registry_list(c, 50)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(f, src, ts)| (f.content, src.chars().take(60).collect(), ts))
                    .collect();
                // panel 3, self always (viewing others is policy, later)
                d.conversations = mind::recent_messages(c, 20)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(dir, content)| (0i64, dir, content.chars().take(120).collect()))
                    .collect();
                // panel 5: the ledger, open then why-closed
                d.commitments_open = mind::commitments::outstanding(c)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|x| (x.what, x.kind, x.due_at))
                    .collect();
                d.commitments_closed = mind::commitments::recently_closed(c, 10)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|x| (x.what, x.status, x.closed_why.unwrap_or_default()))
                    .collect();
                // panel 6: recent receipts beside the boundary log
                let mut stmt = c
                    .prepare(
                        "SELECT intent_id, status, body_json FROM receipts                          ORDER BY rowid DESC LIMIT 15",
                    )
                    .map_err(crate::caps::mind_err)?;
                d.receipts = stmt
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(crate::caps::mind_err)?
                    .filter_map(|x| x.ok())
                    .map(|(i, st, cj)| {
                        let claim = serde_json::from_str::<serde_json::Value>(&cj)
                            .ok()
                            .and_then(|v| v["claims"][0]["claim"].as_str().map(String::from))
                            .unwrap_or_default();
                        (i.chars().take(12).collect(), st, claim.chars().take(90).collect())
                    })
                    .collect();
                // panel 7 continued: the person's own connected accounts
                for acc in mind::connections::list(c).unwrap_or_default() {
                    d.hub.push((
                        format!("google: {}", acc.account),
                        format!(
                            "connected{}",
                            if acc.has_scope(hub::google::SCOPE_MAIL_SEND) {
                                ", send enabled"
                            } else {
                                ", send off"
                            }
                        ),
                    ));
                }
                // panel 9: soul, read from the same stores /soul reads
                d.soul_stance = soul::stance::get(c)
                    .ok()
                    .flatten()
                    .map(|st| st.label())
                    .unwrap_or_else(|| "its own".into());
                if let Ok(dial) = soul::dial::load(c) {
                    d.soul_evolution = dial.evolution;
                    d.soul_dial = dial
                        .settings
                        .iter()
                        .map(|v| {
                            (v.dimension.as_str().to_string(), v.value, v.floor, v.ceiling, v.pinned())
                        })
                        .collect();
                }
                let mut stmt = c
                    .prepare(
                        "SELECT created_at, reason, applied FROM soul_revisions                          ORDER BY created_at DESC LIMIT 10",
                    )
                    .map_err(crate::caps::mind_err)?;
                d.soul_revisions = stmt
                    .query_map([], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))
                    })
                    .map_err(crate::caps::mind_err)?
                    .filter_map(|x| x.ok())
                    .collect();
                // panel 10: vault usage + standing rules
                d.vault_objects = c
                    .query_row("SELECT count(*) FROM media", [], |r| r.get(0))
                    .unwrap_or(0);
                d.files_count = c
                    .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
                    .unwrap_or(0);
                d.standing_rules = mind::instructions::active(c)
                    .map(|v| v.len() as i64)
                    .unwrap_or(0);
                // the ultra counter lives per-day in cell_meta (Q18)
                let key = format!("ultra:{}", chrono::Local::now().format("%Y-%m-%d"));
                d.ultra_used_today = c
                    .query_row(
                        "SELECT value FROM cell_meta WHERE key = ?1",
                        params![key],
                        |r| r.get::<_, String>(0),
                    )
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                // Q21's third clause, surfaced: "conflicting -- pick one"
                d.contested = mind::promotion::contests(c)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(_, a, _, b)| (a, b))
                    .collect();
                Ok(())
            })?;
        }
        Ok(d)
    }

    fn owner_principal(&self) -> i64 {
        self.owner_principal
    }
}

/// Minimal percent-decode for the x-filename header (the client encodes it).
fn urlencoding_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::Registry;
    use prism::{CapabilityRouter, PrismError, CRASH_POINTS};

    fn file_cell(name: &str) -> (Cell, std::path::PathBuf) {
        mind::install_vec();
        let path = std::env::temp_dir().join(format!(
            "killtest-{}-{name}.db",
            trust::ids::random_hex(6)
        ));
        let key = trust::keys::KeyChain::new_dek();
        let conn = trust::cells::open_encrypted(&path, &key).unwrap();
        prism::init_cell_schema(&conn).unwrap();
        mind::init_cell_schema(&conn).unwrap();
        soul::init_cell_schema(&conn).unwrap();
        (Cell::new(conn), path)
    }

    fn envelope(cell: &Cell, content: &str) -> Envelope {
        let msg_id = cell
            .with(|c| Ok(mind::record_message(c, "in", "chat", content).unwrap()))
            .unwrap();
        Envelope {
            surface: "chat".into(),
            principal_id: 1,
            modality: "text".into(),
            content: content.into(),
            ts: trust::ids::ts_ms(),
            device_trust: "owner-session".into(),
            source_msg_id: Some(msg_id),
        }
    }

    /// The renderer the tests speak through: English templates, no model.
    static SPEAK: crate::render::Speak = crate::render::Speak {
        gateway: None,
        voice: None,
    };

    fn live_deps(router: &Registry) -> TurnDeps<'_> {
        TurnDeps {
            router,
            verdicts: &FallbackVerdict,
            renderer: &SPEAK,
            crash: None,
            standing: None,
        on_early: None,
        }
    }

    #[test]
    fn every_turn_ends_with_a_terminal_receipt() {
        let (cell, path) = file_cell("receipts");
        let router = Registry::offline();
        for text in [
            "what time is it?",
            "who are you",
            "help",
            "remind me in 10 minutes to stretch",
            "my reminders",
            "cancel reminder",
            "remember that i drink green tea",
            "what do you remember about tea",
            "my facts",
            "correct fact 1: i drink black tea",
            "forget fact 1",
            "invite",        // owner-only refusal path (owner 0 != principal 1)
            "telegram code", // owner-only refusal path
            "tell me a joke",
        ] {
            let out = prism::run_turn(&cell, &envelope(&cell, text), &live_deps(&router)).unwrap();
            assert!(out.receipt.status.is_terminal(), "{text}");
            assert!(!out.reply.is_empty(), "{text}");
            let kinds = cell
                .with(|c| prism::journal::kinds_for_intent(c, &out.intent_id))
                .unwrap();
            assert_eq!(kinds.first().map(String::as_str), Some("intent_open"), "{text}");
            assert_eq!(kinds.last().map(String::as_str), Some("intent_close"), "{text}");
            assert!(kinds.iter().any(|k| k == "receipt"), "{text}");
        }
        let _ = std::fs::remove_file(path);
    }

    /// A stand-in for the routing model: returns whatever proposal the test
    /// wants, so the validation gate can be exercised without a network.
    struct Proposes(Option<prism::types::ToolCall>);

    impl VerdictProvider for Proposes {
        fn verdict(&self, text: &str) -> prism::types::Verdict {
            FallbackVerdict.verdict(text)
        }
        fn route(
            &self,
            text: &str,
            _tools: &[prism::types::ToolDef],
            _now: &str,
            _standing: Option<&str>,
        ) -> prism::types::Routing {
            let mut v = FallbackVerdict.verdict(text);
            v.lang = "ru".into();
            prism::types::Routing {
                verdict: v,
                call: self.0.clone(),
            }
        }
    }

    fn call(tool: &str, args: serde_json::Value) -> Option<prism::types::ToolCall> {
        Some(prism::types::ToolCall {
            tool: tool.into(),
            args,
        })
    }

    /// The capability a language could not reach before: a Russian sentence
    /// the English floor does not match now creates a real reminder, through
    /// the same governed path, with the person's own words stored verbatim.
    #[test]
    fn a_validated_proposal_reaches_a_real_capability() {
        let (cell, path) = file_cell("proposal_ok");
        let router = Registry::offline();
        let fire_at = (chrono::Local::now() + chrono::Duration::minutes(10)).to_rfc3339();
        let verdicts = Proposes(call(
            "reminder.create",
            serde_json::json!({ "fire_at": fire_at, "about": "размяться" }),
        ));
        let deps = TurnDeps {
            router: &router,
            verdicts: &verdicts,
            renderer: &SPEAK,
            crash: None,
            standing: None,
        on_early: None,
        };
        let out = prism::run_turn(
            &cell,
            &envelope(&cell, "мне бы размяться через десять минут"),
            &deps,
        )
        .unwrap();

        assert_eq!(out.receipt.status, prism::types::ReceiptStatus::Verified);
        // the reminder is really there, and it is their words, not a translation
        let all = cell
            .with(|c| Ok(mind::reminders::list_active(c).unwrap()))
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].about, "размяться");

        let kinds = cell
            .with(|c| prism::journal::kinds_for_intent(c, &out.intent_id))
            .unwrap();
        assert!(kinds.iter().any(|k| k == "call.accepted"), "{kinds:?}");
        let _ = std::fs::remove_file(path);
    }

    /// Sec 6b, end to end: an inference that would destroy something asks
    /// first, and only a yes releases it. The English floor still deletes on
    /// an explicit instruction, because that is an instruction, not a guess.
    #[test]
    fn an_inferred_deletion_asks_before_it_deletes() {
        let (cell, path) = file_cell("confirm");
        let router = Registry::offline();
        let run = |verdicts: &Proposes, text: &str| {
            let deps = TurnDeps {
                router: &router,
                verdicts,
                renderer: &SPEAK,
                crash: None,
            standing: None,
        on_early: None,
            };
            prism::run_turn(&cell, &envelope(&cell, text), &deps).unwrap()
        };

        // a fact to aim at
        run(&Proposes(None), "remember that the demo is on friday");
        let before = cell
            .with(|c| Ok(mind::facts::count_active(c)))
            .unwrap()
            .unwrap();
        assert_eq!(before, 1);

        // the model infers a deletion: parked, asked, nothing done
        let ask = run(
            &Proposes(call("memory.forget", serde_json::json!({"index": 1}))),
            "забудь про демо",
        );
        assert!(ask.reply.contains("say yes"), "{}", ask.reply);
        assert!(!ask.reply.contains("✓"), "nothing ran: {}", ask.reply);
        assert_eq!(cell.with(|c| Ok(mind::facts::count_active(c))).unwrap().unwrap(), 1);

        // "no" leaves it alone
        let no = run(
            &Proposes(call(
                prism::lifecycle::CONFIRM_TOOL,
                serde_json::json!({"confirmed": false}),
            )),
            "нет, не надо",
        );
        // the wording is effect-neutral now that irreversible covers sends
        // and calendar events as well as deletions
        assert!(no.reply.contains("nothing happened"), "{}", no.reply);
        assert_eq!(cell.with(|c| Ok(mind::facts::count_active(c))).unwrap().unwrap(), 1);

        // ask again, then say yes -- and this time it really goes
        run(
            &Proposes(call("memory.forget", serde_json::json!({"index": 1}))),
            "забудь про демо",
        );
        let yes = run(
            &Proposes(call(
                prism::lifecycle::CONFIRM_TOOL,
                serde_json::json!({"confirmed": true}),
            )),
            "да, удаляй",
        );
        assert!(yes.reply.contains("✓ memory.forget"), "{}", yes.reply);
        assert_eq!(cell.with(|c| Ok(mind::facts::count_active(c))).unwrap().unwrap(), 0);

        // and a second yes has nothing left to spend
        let again = run(
            &Proposes(call(
                prism::lifecycle::CONFIRM_TOOL,
                serde_json::json!({"confirmed": true}),
            )),
            "да",
        );
        assert!(!again.reply.contains("✓ memory.forget"), "{}", again.reply);

        let _ = std::fs::remove_file(path);
    }

    /// Saying it in their language costs a trip to a model carrying their
    /// own data, and the journal has to admit that. English costs nothing.
    #[test]
    fn rendering_in_another_language_is_recorded_as_a_disclosure() {
        use prism::lifecycle::Renderer;
        use prism::types::{Rendering, ReplyPart};

        // english: local templates, nothing leaves
        let en = SPEAK.render(
            "en",
            &[ReplyPart::Say(Rendering::bare("registry_empty"))],
            &[],
        );
        assert!(en.disclosed.is_empty());

        // another language with no model: falls back to english, and a
        // fallback is not a disclosure either
        let ru = SPEAK.render(
            "ru",
            &[ReplyPart::Say(Rendering::bare("registry_empty"))],
            &[],
        );
        assert!(
            ru.disclosed.is_empty(),
            "an english fallback sent nothing anywhere"
        );
    }

    /// A yes that cannot be spent must SAY so.
    ///
    /// The confirmation is deliberately spent before the call is planned:
    /// between a double-submitted yes and a double deletion, the double
    /// deletion is far worse. The cost is a window in which the yes is gone
    /// and the work has not happened -- a crash between the two, or a
    /// second yes arriving. Both leave the data untouched, which is the
    /// right direction, but the person must be told, or they walk away
    /// believing a thing was deleted that is still there.
    #[test]
    fn a_yes_that_cannot_be_spent_is_not_silently_swallowed() {
        let (cell, path) = file_cell("confirm_stale");
        let router = Registry::offline();
        let yes = Proposes(call(
            prism::lifecycle::CONFIRM_TOOL,
            serde_json::json!({"confirmed": true}),
        ));
        let run = |verdicts: &Proposes, text: &str| {
            let deps = TurnDeps {
                router: &router,
                verdicts,
                renderer: &SPEAK,
                crash: None,
            standing: None,
        on_early: None,
            };
            prism::run_turn(&cell, &envelope(&cell, text), &deps).unwrap()
        };

        run(&Proposes(None), "remember that the demo is on friday");
        run(
            &Proposes(call("memory.forget", serde_json::json!({"index": 1}))),
            "забудь про демо",
        );

        // the first yes spends it and really deletes
        let first = run(&yes, "да");
        assert!(first.reply.contains("✓ memory.forget"), "{}", first.reply);
        assert_eq!(
            cell.with(|c| Ok(mind::facts::count_active(c)))
                .unwrap()
                .unwrap(),
            0
        );

        // the second finds nothing to spend and says so, rather than
        // quietly becoming small talk
        let second = run(&yes, "да");
        assert!(!second.reply.contains("✓"), "{}", second.reply);
        assert!(
            second.reply.contains("too late") || second.reply.contains("nothing was deleted"),
            "a spent yes must be acknowledged: {}",
            second.reply
        );
        let _ = std::fs::remove_file(path);
    }

    /// The answering tool is offered only while a question is open, so a
    /// model cannot conjure a confirmation for something nobody asked.
    #[test]
    fn the_answering_tool_exists_only_while_a_question_does() {
        let (cell, path) = file_cell("confirm_catalog");
        let router = Registry::offline();
        let names = |c: &prism::Cell| -> Vec<&'static str> {
            prism::CapabilityRouter::describe(&router, c)
                .into_iter()
                .map(|t| t.name)
                .collect()
        };
        assert!(!names(&cell).contains(&prism::lifecycle::CONFIRM_TOOL));

        cell.with(|c| {
            prism::pending::park(c, "int_x", "memory.forget", &serde_json::json!({"index": 1}))?;
            Ok(())
        })
        .unwrap();
        assert!(names(&cell).contains(&prism::lifecycle::CONFIRM_TOOL));
        let _ = std::fs::remove_file(path);
    }

    /// Everything a model can get wrong, and none of it reaches a capability.
    /// Each case must still produce a terminal receipt -- refusing is a
    /// normal turn, not an error.
    #[test]
    fn bad_proposals_are_refused_and_journaled() {
        let (cell, path) = file_cell("proposal_bad");
        let router = Registry::offline();
        let past = (chrono::Local::now() - chrono::Duration::hours(1)).to_rfc3339();

        for (why, proposed) in [
            ("invented tool", call("memory.obliterate", serde_json::json!({}))),
            ("hidden tool", call("answer.model", serde_json::json!({"query": "x"}))),
            ("missing argument", call("memory.remember", serde_json::json!({}))),
            ("wrong type", call("memory.forget", serde_json::json!({"index": "two"}))),
            (
                "stray field",
                call("memory.remember", serde_json::json!({"content": "x", "note": 1})),
            ),
            (
                "time in the past",
                call("reminder.create", serde_json::json!({"fire_at": past, "about": "x"})),
            ),
        ] {
            let verdicts = Proposes(proposed);
            let deps = TurnDeps {
                router: &router,
                verdicts: &verdicts,
                renderer: &SPEAK,
                crash: None,
            standing: None,
        on_early: None,
            };
            let out =
                prism::run_turn(&cell, &envelope(&cell, "что-нибудь сделай"), &deps).unwrap();
            assert!(out.receipt.status.is_terminal(), "{why}");
            let kinds = cell
                .with(|c| prism::journal::kinds_for_intent(c, &out.intent_id))
                .unwrap();
            assert!(
                kinds.iter().any(|k| k == "call.rejected"),
                "{why}: should have been refused, journal was {kinds:?}"
            );
            assert!(
                !kinds.iter().any(|k| k == "call.accepted"),
                "{why}: was accepted"
            );
        }

        // nothing was created or destroyed by any of that
        let all = cell
            .with(|c| Ok(mind::reminders::list_active(c).unwrap()))
            .unwrap();
        assert!(all.is_empty());
        let _ = std::fs::remove_file(path);
    }

    /// The English floor is English, and that is the whole of it. A turn it
    /// matches is answered from templates -- instant, free, offline -- and a
    /// turn in any other language falls through to the routing call, which
    /// is where every other language is understood.
    #[test]
    fn the_english_floor_answers_english_and_declines_the_rest() {
        let (cell, path) = file_cell("floor_english");
        let router = Registry::offline();
        let run = |text: &str| {
            prism::run_turn(&cell, &envelope(&cell, text), &live_deps(&router)).unwrap()
        };

        let en = run("remind me in 10 minutes to stretch");
        assert_eq!(en.lang, "en");
        assert!(en.reply.contains("i'll remind you"), "{}", en.reply);
        // the effect really happened, and the record says so
        assert!(en.reply.contains("✓ reminder.create"), "{}", en.reply);

        // no model here, so a russian sentence gets an honest degradation
        // rather than a guess -- and never an error
        let ru = run("напомни через 10 минут размяться");
        assert!(ru.receipt.status.is_terminal());
        assert!(!ru.reply.is_empty());
        // and it created nothing: the floor did not guess at it
        let all = cell
            .with(|c| Ok(mind::reminders::list_active(c).unwrap()))
            .unwrap();
        assert_eq!(all.len(), 1, "only the english turn created a reminder");

        let _ = std::fs::remove_file(path);
    }

    /// A language nobody has written a pack for must be an ordinary turn,
    /// not an error: the floor declines, the verdict path takes it, and the
    /// receipt is terminal like any other.
    #[test]
    fn an_unpacked_language_still_completes_a_governed_turn() {
        let (cell, path) = file_cell("lang_unpacked");
        let router = Registry::offline();
        for text in [
            "今何時ですか",           // Japanese
            "كم الساعة",              // Arabic
            "¿me recuerdas algo?",    // Spanish
            "wie spät ist es",        // German
            "지금 몇 시야",            // Korean
        ] {
            let out =
                prism::run_turn(&cell, &envelope(&cell, text), &live_deps(&router)).unwrap();
            assert!(out.receipt.status.is_terminal(), "{text}");
            assert!(!out.reply.is_empty(), "{text}");
            // no pack, so the deterministic strings render in English --
            // degraded, never broken
            assert_eq!(out.lang, "en", "{text}");
        }
        let _ = std::fs::remove_file(path);
    }

    /// Sec 6a, structurally: the code path that reads untrusted material
    /// has no ability to act.
    ///
    /// Tool calling raises the stakes of prompt injection from "the model
    /// says something wrong" to "the model DOES something wrong". The only
    /// robust answer is that a tool catalog never reaches a prompt that
    /// contains fetched web pages -- there is nothing there to induce. The
    /// routing call sees the person's message and nothing else; the
    /// research and answer calls see everything and are offered nothing.
    #[test]
    fn untrusted_content_never_meets_a_tool_catalog() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for file in ["src/caps/research.rs", "src/caps/answer.rs"] {
            let src = std::fs::read_to_string(root.join(file)).unwrap();
            let code = match src.find("#[cfg(test)]") {
                Some(i) => &src[..i],
                None => &src[..],
            };
            for forbidden in ["describe(", "ToolDef", "catalog(", "routing_schema"] {
                assert!(
                    !code.contains(forbidden),
                    "{file} mentions {forbidden}: the path that reads untrusted \
                     pages must not be able to offer or accept tools"
                );
            }
        }
    }

    /// Law #4 with teeth. The kernel and the crates beneath it hold no
    /// human-language surface vocabulary at all: `prism` emits `Rendering`
    /// structures, and the only file that contains sentences a person reads
    /// is `robotd::render`, in English, because English is the kernel's own
    /// language. Non-Latin script anywhere in non-test code means someone
    /// started a phrase table again.
    #[test]
    fn no_surface_vocabulary_lives_in_code() {
        fn offending(path: &std::path::Path) -> Vec<char> {
            let src = std::fs::read_to_string(path).unwrap_or_default();
            // tests may quote any language; only non-test code is scanned
            let code = match src.find("#[cfg(test)]") {
                Some(i) => &src[..i],
                None => &src[..],
            };
            code.chars()
                .filter(|c| {
                    let u = *c as u32;
                    (0x0400..=0x04FF).contains(&u)      // Cyrillic
                        || (0x0590..=0x08FF).contains(&u) // Hebrew, Arabic
                        || (0x3040..=0x30FF).contains(&u) // kana
                        || (0x4E00..=0x9FFF).contains(&u) // han
                        || (0xAC00..=0xD7AF).contains(&u) // hangul
                })
                .collect()
        }
        let mut checked = 0;
        for dir in ["../prism/src", "../robotd/src", "../mind/src", "../hub/src"] {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
            let mut stack = vec![root];
            while let Some(p) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&p) else { continue };
                for e in entries.flatten() {
                    let path = e.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.extension().is_some_and(|x| x == "rs") {
                        checked += 1;
                        let bad = offending(&path);
                        assert!(
                            bad.is_empty(),
                            "surface vocabulary hard-coded in {}: {bad:?} -- \
                             the kernel emits Rendering, and other languages \
                             are the renderer's job",
                            path.display()
                        );
                    }
                }
            }
        }
        assert!(checked > 10, "the scan found almost nothing to check");
    }

    #[test]
    fn memory_walk_remember_recall_registry_forget() {
        let (cell, path) = file_cell("memory");
        let router = Registry::offline();
        let run = |text: &str| {
            prism::run_turn(&cell, &envelope(&cell, text), &live_deps(&router))
                .unwrap()
                .reply
        };
        let r = run("remember that the demo is on friday");
        assert!(r.contains("remembered: the demo is on friday"), "{r}");
        let r = run("what do you remember about the demo");
        assert!(r.contains("the demo is on friday"), "{r}");
        let r = run("my facts");
        assert!(r.contains("from your words"), "{r}");
        let r = run("correct fact 1: the demo moved to monday");
        assert!(r.contains("superseded"), "{r}");
        let r = run("forget fact 1");
        assert!(r.contains("forgotten for real"), "{r}");
        assert_eq!(
            cell.with(|c| Ok(mind::facts::count_active(c).unwrap())).unwrap(),
            0
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn remember_without_provenance_fails_honestly() {
        let (cell, path) = file_cell("noprov");
        let router = Registry::offline();
        let env = Envelope {
            surface: "chat".into(),
            principal_id: 1,
            modality: "text".into(),
            content: "remember that x is y".into(),
            ts: trust::ids::ts_ms(),
            device_trust: "owner-session".into(),
            source_msg_id: None,
        };
        assert!(prism::run_turn(&cell, &env, &live_deps(&router)).is_err());
        assert_eq!(
            cell.with(|c| Ok(mind::facts::count_active(c).unwrap())).unwrap(),
            0
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn kill_test_crash_at_every_boundary_replays_exactly_once() {
        type EffectCount = fn(&Connection) -> i64;
        let cases: [(&str, EffectCount); 2] = [
            ("remind me in 10 minutes to call mark", |c| {
                mind::reminders::count_active(c).unwrap()
            }),
            ("remember that mark prefers mornings", |c| {
                mind::facts::count_active(c).unwrap()
            }),
        ];
        for (text, check) in cases {
            for point in CRASH_POINTS {
                let (cell, path) = file_cell(point);
                let router = Registry::offline();
                let crash = |p: &str| p == point;
                let deps = TurnDeps {
                    router: &router,
                    verdicts: &FallbackVerdict,
                    renderer: &SPEAK,
                    crash: Some(&crash),
            standing: None,
        on_early: None,
                };
                let err = prism::run_turn(&cell, &envelope(&cell, text), &deps).unwrap_err();
                assert!(matches!(err, PrismError::SimulatedCrash(_)), "{text}@{point}");

                let s1 = prism::replay::resume_incomplete(&cell, &router, &SPEAK).unwrap();
                assert_eq!(s1.resumed + s1.closed_failed, 1, "{text}@{point}");
                let s2 = prism::replay::resume_incomplete(&cell, &router, &SPEAK).unwrap();
                assert_eq!(s2.resumed + s2.closed_failed, 0, "{text}@{point}");

                let expected = if point == "after_open" { 0 } else { 1 };
                assert_eq!(
                    cell.with(|c| Ok(check(c))).unwrap(),
                    expected,
                    "{text}@{point}"
                );
                assert!(cell
                    .with(prism::journal::open_intents)
                    .unwrap()
                    .is_empty());
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// A capability that spends seconds in a network call must NOT hold the
    /// person's cell while it does. Before Phase C the whole turn ran under
    /// one guard, so a `web.research` turn could hold it for ~2 minutes:
    /// their history, dashboard, SSE and reminders all blocked, and the
    /// watchdog could not even observe the hang because it needed the same
    /// lock to look. No test caught it -- tests were single-threaded and the
    /// gateway was mocked.
    #[test]
    fn a_slow_capability_does_not_block_the_cell() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        /// Stands in for a model call: sleeps with the cell NOT held.
        struct SlowRouter {
            entered: Arc<AtomicBool>,
        }
        impl CapabilityRouter for SlowRouter {
            fn execute(
                &self,
                _cell: &Cell,
                _capability: &str,
                _args: &serde_json::Value,
                _intent_id: &str,
                _lang: &str,
            ) -> Result<Outcome, PrismError> {
                self.entered.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(600));
                Ok(Outcome::utterance(String::new(), vec![], "slow answer".into()))
            }
            fn describe(&self, _cell: &Cell) -> Vec<prism::types::ToolDef> {
                vec![]
            }
            fn validate(
                &self,
                _tool: &str,
                _args: &serde_json::Value,
            ) -> Result<prism::types::Effect, String> {
                Err("this router offers no tools".into())
            }
        }

        let (cell, path) = file_cell("slowlock");
        let entered = Arc::new(AtomicBool::new(false));
        let router = SlowRouter { entered: entered.clone() };
        let env = envelope(&cell, "tell me a joke"); // verdict path -> answer.model
        let probe = cell.clone();

        let turn = std::thread::spawn(move || {
            let deps = TurnDeps {
                router: &router,
                verdicts: &FallbackVerdict,
                renderer: &SPEAK,
                crash: None,
            standing: None,
        on_early: None,
            };
            prism::run_turn(&cell, &env, &deps).unwrap()
        });

        // wait until the slow capability is definitely running
        let start = Instant::now();
        while !entered.load(Ordering::SeqCst) {
            assert!(start.elapsed() < Duration::from_secs(5), "capability never ran");
            std::thread::sleep(Duration::from_millis(5));
        }

        // ...and now the cell must still be readable, promptly
        let probe_start = Instant::now();
        let n = probe
            .with(|c| Ok(mind::message_count(c).unwrap()))
            .expect("cell must be readable while a turn is in a model call");
        let waited = probe_start.elapsed();
        assert!(n >= 1);
        assert!(
            waited < Duration::from_millis(250),
            "cell was blocked for {waited:?} by a slow capability -- the lock is \
             being held across the call again"
        );

        turn.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reply_effect_is_unique_per_intent() {
        let (cell, path) = file_cell("outbox");
        let router = Registry::offline();
        let out =
            prism::run_turn(&cell, &envelope(&cell, "what time is it"), &live_deps(&router))
                .unwrap();
        let (again_id, fresh) = cell
            .with(|c| prism::outbox::enqueue(c, &out.intent_id, "surface:chat", &out.reply))
            .unwrap();
        assert!(!fresh);
        assert_eq!(again_id, out.reply_effect_id);
        let _ = std::fs::remove_file(path);
    }
}
