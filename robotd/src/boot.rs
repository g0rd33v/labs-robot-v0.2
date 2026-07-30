//! First boot and every boot: keys, core, owner cell, slug. One directory --
//! `core.db + cells/ + media/` -- is the whole Robot (decisions Q9).

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

    // owner principal + cell
    let owner_principal = ensure_owner(&core)?;
    let dek = ensure_cell_key(&core, &keys, "owner")?;
    let owner_cell =
        trust::cells::open_encrypted(&data_dir.join("cells").join("owner.db"), &dek)
            .context("opening owner cell")?;
    prism::init_cell_schema(&owner_cell)?;
    mind::init_cell_schema(&owner_cell)?;

    // the local embedding seat (Q24): weights fetched through the hub
    // gateway on first run, boundary-logged; offline or disabled -> the
    // robot still boots, recall degrades to FTS + recency
    let embedder = if cfg.mind.embeddings {
        match hub::Embedder::init(Path::new(&cfg.mind.model_cache), Some(&core)) {
            Ok(e) => Some(std::sync::Arc::new(e)),
            Err(e) => {
                tracing::warn!("embedder unavailable, vector door closed: {e}");
                None
            }
        }
    } else {
        None
    };

    // crash replay (arch sec 3): resume every intent the last run left open;
    // an intent without a terminal receipt is a bug, never a silent drop
    let router = crate::robot::Capabilities {
        embedder: embedder.clone(),
    };
    let replayed = prism::replay::resume_incomplete(&owner_cell, &router)?;
    if replayed.resumed + replayed.closed_failed > 0 {
        tracing::info!(
            "crash replay: {} resumed, {} closed failed",
            replayed.resumed,
            replayed.closed_failed
        );
    }

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

    // the hub gateway exists from M1 on; zero endpoints is the honest default
    let gateway = hub::Gateway::new();
    schema::core_journal(
        &core,
        "boot",
        &serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "name": cfg.robot.name,
            "hub_endpoints": gateway.endpoints(),
        })
        .to_string(),
    )?;

    let robot = Arc::new(RobotCore {
        owner_principal,
        core: Mutex::new(core),
        owner_cell: Mutex::new(owner_cell),
        embedder,
    });
    let state = Arc::new(surfaces::WebState::new(robot, slug_hash));
    let addr = SocketAddr::new(
        cfg.server.host.parse().context("server.host")?,
        cfg.server.port,
    );
    Ok(BootResult {
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

fn ensure_cell_key(core: &Connection, keys: &KeyChain, cell_id: &str) -> anyhow::Result<[u8; 32]> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RobotConfig, RobotSection, ServerSection};

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
            },
            mind: crate::config::MindSection {
                embeddings: false, // hermetic tests: no downloads
                model_cache: dir.join("models").to_string_lossy().into_owned(),
            },
        };
        (cfg, dir)
    }

    #[test]
    fn m1_gate_boot_encrypt_journal_boundary() {
        let (cfg, dir) = test_cfg();
        let boot = bootstrap(&cfg).unwrap();
        assert!(boot.slug_url.starts_with("http://127.0.0.1:0/a/"));

        // one full turn through the robot
        let reply = boot
            .state
            .robot
            .handle_message("hello robot".into())
            .unwrap();
        assert!(reply.contains("M4"), "fallback reply expected, got: {reply}");

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

        // the turn is journaled in the cell: opens with intent_open, closes
        // with intent_close, and a receipt row exists in between
        let dek = ensure_cell_key(&core, &keys, "owner").unwrap();
        let cell =
            trust::cells::open_encrypted(&dir.join("cells").join("owner.db"), &dek).unwrap();
        let kinds = prism::journal::kinds_for_latest_intent(&cell).unwrap();
        assert_eq!(kinds.first().map(String::as_str), Some("intent_open"));
        assert_eq!(kinds.last().map(String::as_str), Some("intent_close"));
        assert!(kinds.iter().any(|k| k == "receipt"));
        assert_eq!(prism::receipts::count(&cell).unwrap(), 1);
        assert_eq!(mind::message_count(&cell).unwrap(), 2);

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
