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
    pub ultra_daily_cap: u32,
    pub public_base: String,
    pub robot_name: String,
    pub started_at: i64,
    events: broadcast::Sender<i64>,
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
        public_base: String,
        robot_name: String,
    ) -> Self {
        Self {
            owner_principal,
            core,
            cells: Mutex::new(HashMap::new()),
            open_gate: Mutex::new(()),
            keys,
            data_dir,
            embedder,
            gateway,
            research,
            ultra_daily_cap,
            public_base,
            robot_name,
            started_at: trust::ids::ts_ms(),
            events: broadcast::channel(64).0,
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
    pub fn router(&self) -> Registry {
        Registry::new(
            Services {
                embedder: self.embedder.clone(),
                gateway: self.gateway.clone(),
                research: self.research.clone(),
            },
            Policy {
                ultra_daily_cap: self.ultra_daily_cap,
            },
            Instance {
                core: Some(self.core.clone()),
                owner_principal: self.owner_principal,
                public_base: self.public_base.clone(),
            },
        )
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
            let router = self.router();
            let verdicts: Box<dyn VerdictProvider> = match &self.gateway {
                Some(g) => Box::new(hub::GatewayVerdicts { gateway: g.clone() }),
                None => Box::new(FallbackVerdict),
            };
            let speak = crate::render::Speak {
                gateway: self.gateway.clone(),
            };
            let deps = TurnDeps {
                router: &router,
                verdicts: verdicts.as_ref(),
                renderer: &speak,
                crash: None,
            };
            // the cell is locked only in short bursts inside run_turn; the
            // model call in the middle happens with it free
            let out = prism::run_turn(cell, &env, &deps)?;
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
    static SPEAK: crate::render::Speak = crate::render::Speak { gateway: None };

    fn live_deps(router: &Registry) -> TurnDeps<'_> {
        TurnDeps {
            router,
            verdicts: &FallbackVerdict,
            renderer: &SPEAK,
            crash: None,
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
            // sec 6b: an inference may not delete
            ("irreversible on inference", call("memory.forget", serde_json::json!({"index": 1}))),
        ] {
            let verdicts = Proposes(proposed);
            let deps = TurnDeps {
                router: &router,
                verdicts: &verdicts,
                renderer: &SPEAK,
                crash: None,
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
            fn describe(&self) -> Vec<prism::types::ToolDef> {
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
