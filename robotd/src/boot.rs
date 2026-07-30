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
        public_base,
        cfg.robot.name.clone(),
    ));

    // crash replay (arch sec 3): resume every intent the last run left open
    // in every principal's cell; an intent without a terminal receipt is a
    // bug, never a silent drop
    let router = robot.router();
    for principal in robot.principals_active()? {
        let handle = robot.cell(principal)?;
        let replayed = prism::replay::resume_incomplete(&handle.cell, &router)?;
        if replayed.resumed + replayed.closed_failed > 0 {
            tracing::info!(
                "crash replay (principal {principal}): {} resumed, {} closed failed",
                replayed.resumed,
                replayed.closed_failed
            );
        }
    }

    let state = Arc::new(surfaces::WebState::new(robot.clone(), slug_hash));
    Ok(BootResult {
        robot,
        state,
        slug_url: format!("http://{addr}/a/{slug}"),
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
            },
            mind: MindSection {
                embeddings: false, // hermetic tests: no downloads
                model_cache: dir.join("models").to_string_lossy().into_owned(),
            },
            hub: Default::default(),
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
            .unwrap();
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
            .unwrap();
        assert!(invite_reply.contains("/i/"), "{invite_reply}");
        let token = invite_reply.split("/i/").nth(1).unwrap().trim().to_string();
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
            .unwrap();
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

    #[test]
    fn second_boot_reuses_identity_and_slug() {
        let (cfg, dir) = test_cfg();
        let a = bootstrap(&cfg).unwrap();
        let b = bootstrap(&cfg).unwrap();
        assert_eq!(a.slug_url, b.slug_url);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
