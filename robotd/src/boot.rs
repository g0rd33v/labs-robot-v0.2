//! First boot and every boot: keys, core, owner cell, gateway, slug. One
//! directory -- `core.db + cells/ + media/ + models/` -- is the whole Robot
//! (decisions Q9).

use crate::config::RobotConfig;
use crate::robot::RobotCore;
use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use trust::keys::KeyChain;
use trust::schema;

pub struct BootResult {
    pub robot: Arc<RobotCore>,
    pub state: Arc<surfaces::WebState>,
    pub slug_url: String,
    pub addr: SocketAddr,
}

pub fn bootstrap(cfg: &RobotConfig) -> anyhow::Result<BootResult> {
    let data_dir = Path::new(&cfg.robot.data_dir);
    std::fs::create_dir_all(data_dir.join("cells"))?;
    std::fs::create_dir_all(data_dir.join("media"))?;

    // the vector door: register sqlite-vec before any cell opens
    mind::install_vec();

    // keys and core
    let keys = KeyChain::load_or_create(&data_dir.join("kek.key"))?;
    let core = trust::cells::open_encrypted(&data_dir.join("core.db"), &keys.core_db_key())
        .context("opening core.db")?;
    schema::init_core(&core)?;
    if schema::meta_get(&core, "robot_id")?.is_none() {
        schema::meta_set(&core, "robot_id", &trust::ids::new_id("robot"))?;
        schema::meta_set(&core, "schema_version", "1")?;
    }

    // owner principal (the cell itself opens lazily via RobotCore)
    let owner_principal = ensure_owner(&core)?;

    // tier-3 slug (Q32): stored inside the encrypted core so the URL can be
    // re-printed at every boot; rotation = replacing this row.
    let slug = match schema::meta_get(&core, "slug_token")? {
        Some(s) => s,
        None => {
            let t = trust::ids::random_hex(16);
            schema::meta_set(&core, "slug_token", &t)?;
            t
        }
    };
    let slug_hash = trust::ids::sha256_hex(slug.as_bytes());

    // Which INSTALLATION this is. `robot_id` says which robot; this says
    // which copy of it. Restore deliberately does not carry it, so the
    // stick mints its own and two instances can attribute a deletion and
    // hold a sync watermark that means something.
    let instance_id = match schema::meta_get(&core, "instance_id")? {
        Some(id) => id,
        None => {
            let id = format!("inst_{}", trust::ids::random_hex(8));
            schema::meta_set(&core, "instance_id", &id)?;
            tracing::info!("minted instance id {id}");
            id
        }
    };

    // the local embedding seat (Q24): weights fetched through the hub
    // gateway on first run, boundary-logged; offline or disabled -> the
    // robot still boots, recall degrades to FTS + recency
    let embedder = if cfg.mind.embeddings {
        match hub::Embedder::init(Path::new(&cfg.mind.model_cache), Some(&core)) {
            Ok(e) => Some(Arc::new(e)),
            Err(e) => {
                tracing::warn!("embedder unavailable, vector door closed: {e}");
                None
            }
        }
    } else {
        None
    };

    // Law #3 is only credible if the chain is actually checked. Verify at
    // every boot, journal the verdict, and surface it on the dashboard --
    // tamper-evidence nobody evaluates is not evidence.
    let chain_ok = trust::boundary::verify_chain(&core)?;
    if chain_ok {
        tracing::info!(
            "boundary log verified: {} crossings, chain intact",
            trust::boundary::count(&core)?
        );
    } else {
        tracing::error!(
            "BOUNDARY LOG CHAIN BROKEN -- the I/O record for this robot can no \
             longer be trusted end to end. this is reported in the dashboard."
        );
    }

    // Anchors: heads this robot already published with its backups. A chain
    // that verifies internally can still have been rewritten wholesale by
    // someone holding the KEK -- recomputing every hash from a new genesis
    // is easy. What they cannot do is reach back into a manifest that left
    // for two off-site destinations last week. Any anchor that no longer
    // matches means history changed behind a point we had already committed
    // to in public.
    let broken = trust::boundary::broken_anchors(&core)?;
    if !broken.is_empty() {
        tracing::error!(
            "BOUNDARY LOG REWRITTEN behind {} published anchor(s) -- the earliest \
             is seq {} ({}). the chain may verify against itself and still be a \
             different history than the one this robot published. compare \
             chain_head in your off-site backup manifests.",
            broken.len(),
            broken[0].seq,
            broken[0].published_to
        );
    }
    schema::core_journal(
        &core,
        "boot",
        &serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "name": cfg.robot.name,
            "boundary_chain_ok": chain_ok,
        })
        .to_string(),
    )?;

    // from here core is shared: the boundary sink for every gateway call
    let core = Arc::new(Mutex::new(core));

    // the model gateway (sec 6): key from the environment (pulled from the
    // OS keychain at launch), held in memory only. no key = honest floor.
    let gateway = match std::env::var("OPENROUTER_API_KEY") {
        Ok(key) if !key.trim().is_empty() => {
            let gw_cfg = hub::GatewayConfig {
                base_url: cfg.hub.base_url.clone(),
                hedge_after_ms: cfg.hub.hedge_after_ms,
                ..Default::default()
            };
            let api = Arc::new(hub::UreqApi::new(
                key.trim().to_string(),
                cfg.hub.base_url.clone(),
            ));
            tracing::info!("model gateway online (openrouter; cast per sec 6a)");
            Some(Arc::new(hub::ModelGateway::new(
                api,
                cfg.hub.cast.clone(),
                gw_cfg,
                Some(core.clone()),
            )))
        }
        _ => {
            tracing::warn!("OPENROUTER_API_KEY not set -- model brain offline, floor only");
            None
        }
    };
    let research = match std::env::var("SERPER_API_KEY") {
        Ok(key) if !key.trim().is_empty() => Some(Arc::new(hub::Research::new(
            Some(key.trim().to_string()),
            Some(core.clone()),
        ))),
        _ => {
            tracing::warn!("SERPER_API_KEY not set -- web search off");
            None
        }
    };

    // The Google connector. The client id is not a secret (it ships in
    // every installed app); the secret is read from the environment like
    // every other credential and never written to disk.
    let (google, oauth_app) = match std::env::var("GOOGLE_OAUTH_CLIENT_ID") {
        Ok(id) if !id.trim().is_empty() => {
            let secret = std::env::var("GOOGLE_OAUTH_CLIENT_SECRET")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            (
                Some(Arc::new(hub::google::Google::new(Some(core.clone())))),
                Some(Arc::new(hub::oauth::App::loopback(
                    id.trim().to_string(),
                    secret,
                    cfg.server.port,
                ))),
            )
        }
        _ => {
            tracing::warn!("GOOGLE_OAUTH_CLIENT_ID not set -- calendar and email off");
            (None, None)
        }
    };

    let addr = SocketAddr::new(
        cfg.server.host.parse().context("server.host")?,
        cfg.server.port,
    );
    let public_base = if cfg.server.public_base.is_empty() {
        format!("http://{addr}")
    } else {
        cfg.server.public_base.clone()
    };

    let robot = Arc::new(RobotCore::new(
        owner_principal,
        core,
        keys,
        data_dir.to_path_buf(),
        embedder,
        gateway,
        research,
        cfg.hub.ultra_daily_cap,
        cfg.hub.verify_percent,
        cfg.policy.approval_required.clone(),
        public_base,
        cfg.robot.name.clone(),
        instance_id,
        google,
        oauth_app,
    ));

    if !broken.is_empty() {
        let _ = robot.tell_owner(&format!(
            "the boundary log no longer matches {} head(s) i published with earlier \
             backups -- the earliest is entry {}. that means history changed behind \
             a point already recorded off-site. compare `chain_head` in the backup \
             manifests on your storage box and in spaces.",
            broken.len(),
            broken[0].seq
        ));
    }

    // crash replay (arch sec 3): resume every intent the last run left open
    // in every principal's cell; an intent without a terminal receipt is a
    // bug, never a silent drop
    for principal in robot.principals_active()? {
        let handle = robot.cell(principal)?;
        let router = robot.router(Some(handle.vault.clone()));
        let replayed =
            prism::replay::resume_incomplete(&handle.cell, &router, &crate::render::Speak::offline())?;
        if replayed.resumed + replayed.closed_failed > 0 {
            tracing::info!(
                "crash replay (principal {principal}): {} resumed, {} closed failed",
                replayed.resumed,
                replayed.closed_failed
            );
        }
    }

    let state = Arc::new(surfaces::WebState::mounted(
        robot.clone(),
        slug_hash,
        cfg.server.path_prefix.clone(),
    ));
    Ok(BootResult {
        robot,
        state,
        // the URL a person actually opens: the public base when the robot
        // sits behind a proxy, plus the path it is mounted at
        slug_url: format!(
            "{}{}/a/{slug}",
            if cfg.server.public_base.is_empty() {
                format!("http://{addr}")
            } else {
                cfg.server.public_base.trim_end_matches('/').to_string()
            },
            cfg.server.path_prefix.trim_end_matches('/')
        ),
        addr,
    })
}

fn ensure_owner(core: &Connection) -> anyhow::Result<i64> {
    let existing: Option<i64> = core
        .query_row(
            "SELECT id FROM principals WHERE kind = 'owner' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    core.execute(
        "INSERT INTO principals(kind, display_name, cell_id, created_at) \
         VALUES ('owner', 'owner', 'owner', ?1)",
        params![trust::ids::ts_ms()],
    )?;
    Ok(core.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MindSection, RobotConfig, RobotSection, ServerSection};

    fn test_cfg() -> (RobotConfig, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("robotd-test-{}", trust::ids::random_hex(6)));
        let cfg = RobotConfig {
            robot: RobotSection {
                name: "bender-test".into(),
                data_dir: dir.to_string_lossy().into_owned(),
            },
            server: ServerSection {
                host: "127.0.0.1".into(),
                port: 0,
                public_base: String::new(),
                path_prefix: String::new(),
            },
            mind: MindSection {
                embeddings: false, // hermetic tests: no downloads
                model_cache: dir.join("models").to_string_lossy().into_owned(),
            },
            hub: Default::default(),
            backup: crate::config::BackupSection {
                every_hours: 0, // tests never shell out to a real backup
                script: String::new(),
            },
            sync: Default::default(),
            policy: Default::default(),
            update: Default::default(),
        };
        (cfg, dir)
    }

    #[test]
    fn gate_boot_encrypt_journal_boundary() {
        // hermetic: even if the developer shell has keys, tests must not
        // call the network -- the gateway is constructed but never used by
        // the floor-only turns below when the env is clean in CI; the floor
        // path itself never touches the gateway by construction (Q17).
        let (cfg, dir) = test_cfg();
        let boot = bootstrap(&cfg).unwrap();
        assert!(boot.slug_url.starts_with("http://127.0.0.1:0/a/"));

        // one full floor turn through the robot (deterministic, no network)
        let owner = boot.robot.owner_principal;
        let reply = boot
            .state
            .robot
            .handle_message(owner, "what time is it?".into())
            .unwrap()
            .text()
            .to_string();
        assert!(reply.contains("it's"), "floor time answer expected: {reply}");

        // cells are opaque on disk
        assert!(trust::cells::file_looks_encrypted(&dir.join("core.db")).unwrap());
        assert!(
            trust::cells::file_looks_encrypted(&dir.join("cells").join("owner.db")).unwrap()
        );

        // wrong key cannot open the owner cell
        let wrong = trust::keys::KeyChain::new_dek();
        assert!(
            trust::cells::open_encrypted(&dir.join("cells").join("owner.db"), &wrong).is_err()
        );

        // boundary log holds the in+out pair and the chain verifies
        let keys = KeyChain::load_or_create(&dir.join("kek.key")).unwrap();
        let core =
            trust::cells::open_encrypted(&dir.join("core.db"), &keys.core_db_key()).unwrap();
        assert_eq!(trust::boundary::count(&core).unwrap(), 2);
        assert!(trust::boundary::verify_chain(&core).unwrap());

        // the turn is journaled with a receipt, and history serves it
        let handle = boot.robot.cell(owner).unwrap();
        handle
            .cell
            .with(|c| {
                let kinds = prism::journal::kinds_for_latest_intent(c).unwrap();
                assert_eq!(kinds.first().map(String::as_str), Some("intent_open"));
                assert_eq!(kinds.last().map(String::as_str), Some("intent_close"));
                assert!(kinds.iter().any(|k| k == "receipt"));
                assert_eq!(prism::receipts::count(c).unwrap(), 1);
                assert_eq!(mind::message_count(c).unwrap(), 2);
                Ok(())
            })
            .unwrap();
        let history = boot.state.robot.history(owner, 0).unwrap();
        assert_eq!(history.len(), 2);

        // M5: invite -> member cell isolation (law #2 as files)
        let invite_reply = boot
            .state
            .robot
            .handle_message(owner, "invite".into())
            .unwrap()
            .text()
            .to_string();
        assert!(invite_reply.contains("/i/"), "{invite_reply}");
        let token = invite_reply.split("/i/").nth(1).unwrap().lines().next().unwrap().trim().to_string();
        let (member, name) = boot.state.robot.accept_invite(&token).unwrap();
        assert!(name.starts_with("member-"));
        assert_ne!(member, owner);
        // the same invite cannot be redeemed twice
        assert!(boot.state.robot.accept_invite(&token).is_err());
        // the member's history is empty -- not the owner's
        assert!(boot.state.robot.history(member, 0).unwrap().is_empty());
        // owner remembers something; the member's cell must not see it
        boot.state
            .robot
            .handle_message(owner, "remember that the launch code is 4242".into())
            .unwrap();
        let member_recall = boot
            .state
            .robot
            .handle_message(member, "what do you remember".into())
            .unwrap()
            .text()
            .to_string();
        assert!(
            !member_recall.contains("4242"),
            "cell isolation broken: {member_recall}"
        );
        // and the member's cell is its own encrypted file on disk
        let member_files: Vec<_> = std::fs::read_dir(dir.join("cells"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("member"))
            .collect();
        assert!(!member_files.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Removal is a crypto-shred, not a flag (spec 4.2.3.4).
    ///
    /// The assertion that matters is not "the reply said done" but that the
    /// file is gone AND the key is gone -- checked separately, because
    /// deleting only the file leaves a live key for a backup to resurrect,
    /// and deleting only the key leaves bytes that look like data.
    #[test]
    fn removing_someone_destroys_their_cell_beyond_recovery() {
        let (cfg, dir) = test_cfg();
        let boot = bootstrap(&cfg).unwrap();
        let owner = boot.robot.owner_principal;

        let reply = boot
            .state
            .robot
            .handle_message(owner, "invite".into())
            .unwrap()
            .text()
            .to_string();
        let token = reply.split("/i/").nth(1).unwrap().lines().next().unwrap().trim().to_string();
        let (member, _) = boot.state.robot.accept_invite(&token).unwrap();
        boot.state
            .robot
            .handle_message(member, "remember that my passport number is X99".into())
            .unwrap();

        let cell_id: String = {
            let core = boot.robot.core.lock().unwrap();
            core.query_row(
                "SELECT cell_id FROM principals WHERE id = ?1",
                rusqlite::params![member],
                |r| r.get(0),
            )
            .unwrap()
        };
        let cell_file = dir.join("cells").join(format!("{cell_id}.db"));
        assert!(cell_file.exists(), "the member's cell should exist first");

        // a member may not remove the owner, nor anyone but themselves
        assert!(boot.robot.remove_member(member, owner).is_err());
        assert!(boot.robot.remove_member(owner, owner).is_err(), "the owner is the robot");

        let said = boot.robot.remove_member(owner, member).unwrap();
        assert!(said.contains("key is destroyed"), "{said}");

        // 1. the bytes are gone -- including the WAL, which holds the most
        //    recent plaintext pages
        assert!(!cell_file.exists(), "the cell file survived a removal");
        assert!(!dir.join("cells").join(format!("{cell_id}.db-wal")).exists());

        // 2. the key is gone, so even a restored file would be noise
        let keys_left: i64 = {
            let core = boot.robot.core.lock().unwrap();
            core.query_row(
                "SELECT count(*) FROM cell_keys WHERE cell_id = ?1",
                rusqlite::params![cell_id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(keys_left, 0, "the wrapped DEK survived -- this is not a shred");

        // 3. the person can no longer act, and re-opening does not silently
        //    mint them a fresh cell
        assert!(boot.state.robot.history(member, 0).is_err());
        assert!(!boot.robot.principals_active().unwrap().contains(&member));

        // 4. one line survives, which is what makes this auditable rather
        //    than merely quiet
        let journaled: i64 = {
            let core = boot.robot.core.lock().unwrap();
            core.query_row(
                "SELECT count(*) FROM core_journal WHERE kind = 'member.removed'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(journaled, 1);

        // and removing twice is an error, not a second silent success
        assert!(boot.robot.remove_member(owner, member).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The only authorization boundary in the product, exercised for real:
    /// an owner and a member, both with journaled intents, asserting the
    /// ABSENCE of the effect for the member -- not just the refusal string.
    /// The previous tests could not reach the check at all (see
    /// `Capabilities::require_owner`).
    #[test]
    fn only_the_owner_can_mint_invites_and_bind_telegram() {
        let (cfg, dir) = test_cfg();
        let boot = bootstrap(&cfg).unwrap();
        let owner = boot.robot.owner_principal;

        // owner mints one -- an invites row appears
        let reply = boot
            .state
            .robot
            .handle_message(owner, "invite".into())
            .unwrap()
            .text()
            .to_string();
        assert!(reply.contains("/i/"), "{reply}");
        let token = reply.split("/i/").nth(1).unwrap().lines().next().unwrap().trim().to_string();
        let (member, _) = boot.state.robot.accept_invite(&token).unwrap();
        assert_ne!(member, owner);

        let invites_now = |r: &Arc<crate::robot::RobotCore>| -> i64 {
            let core = r.core.lock().unwrap();
            core.query_row("SELECT count(*) FROM invites", [], |row| row.get(0))
                .unwrap()
        };
        let before = invites_now(&boot.robot);

        // the member asks for an invite: refused, AND no row is created
        let reply = boot
            .state
            .robot
            .handle_message(member, "invite".into())
            .unwrap()
            .text()
            .to_string();
        assert!(
            reply.contains("only the owner"),
            "member should be refused: {reply}"
        );
        assert_eq!(
            invites_now(&boot.robot),
            before,
            "a refused invite must not mint a token"
        );

        // same for the telegram bind code: refused, and no code is stored
        let reply = boot
            .state
            .robot
            .handle_message(member, "telegram code".into())
            .unwrap()
            .text()
            .to_string();
        assert!(reply.contains("only the owner"), "{reply}");
        {
            let core = boot.robot.core.lock().unwrap();
            let stored = trust::schema::meta_get(&core, "tg_bind_code_hash").unwrap();
            assert!(stored.is_none(), "a refused request must not store a code");
        }

        // ...and the owner still can
        let reply = boot
            .state
            .robot
            .handle_message(owner, "telegram code".into())
            .unwrap()
            .text()
            .to_string();
        assert!(reply.contains("bind code"), "{reply}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn second_boot_reuses_identity_and_slug() {
        let (cfg, dir) = test_cfg();
        let a = bootstrap(&cfg).unwrap();
        let b = bootstrap(&cfg).unwrap();
        assert_eq!(a.slug_url, b.slug_url);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
