//! `robotd notify <text>` -- put an operational message into the owner's
//! chat from outside the running daemon.
//!
//! Background jobs that fail silently are worse than no job at all. The
//! scheduled backup uses this to report failures where the owner actually
//! looks: their own conversation. The message is journaled and receipted
//! like any other action -- an ops notice is still an effect, and the
//! receipts law does not have an exception for cron.
//!
//! Safe to run against a live daemon: cells are WAL-mode with a busy
//! timeout, and the chat picks the message up on its next history poll.

use crate::config::RobotConfig;
use anyhow::Context;
use prism::types::Outcome;
use std::path::Path;
use trust::boundary::{Crossing, Direction};
use trust::keys::KeyChain;

pub fn notify_owner(cfg: &RobotConfig, text: &str) -> anyhow::Result<()> {
    let data_dir = Path::new(&cfg.robot.data_dir);
    mind::install_vec();

    let keys = KeyChain::load_or_create(&data_dir.join("kek.key"))?;
    let core = trust::cells::open_encrypted(&data_dir.join("core.db"), &keys.core_db_key())
        .context("opening core.db")?;

    let (owner_id, cell_id): (i64, String) = core
        .query_row(
            "SELECT id, cell_id FROM principals WHERE kind = 'owner' LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .context("no owner principal -- has the robot ever been booted?")?;

    let dek = crate::robot::ensure_cell_key(&core, &keys, &cell_id)?;
    let conn = trust::cells::open_encrypted(
        &data_dir.join("cells").join(format!("{cell_id}.db")),
        &dek,
    )
    .context("opening the owner cell")?;
    prism::init_cell_schema(&conn)?;
    mind::init_cell_schema(&conn)?;
    soul::init_cell_schema(&conn)?;
    let cell = prism::Cell::new(conn);

    // a journaled, receipted system intent -- not a bare row insert
    let intent_id = trust::ids::new_id("int");
    let open_json = serde_json::json!({
        "system": "ops.notify",
        "principal_id": owner_id,
        "chars": text.chars().count(),
    })
    .to_string();
    cell.with(|c| prism::journal::intent_open(c, &intent_id, &open_json))?;

    let outcome = Outcome::attested(
        trust::ids::new_id("pstep"),
        vec![prism::types::Evidence {
            kind: "deterministic".into(),
            provider: "ops".into(),
            external_id: "notify".into(),
            hash: trust::ids::sha256_hex(text.as_bytes()),
            ts: trust::ids::ts_ms(),
        }],
        format!(
            "delivered an operational notice to the owner ({} chars)",
            text.chars().count()
        ),
        prism::types::Rendering::bare("ops_notice"),
    );
    let outcome_json = serde_json::to_string(&outcome)?;
    cell.with(|c| prism::journal::step(c, &intent_id, "outcome", &outcome_json, None))?;

    let receipt = prism::lifecycle::build_receipt(&intent_id, &[outcome]);
    let receipt = cell.with(|c| prism::receipts::store(c, &receipt))?;
    cell.with(|c| Ok(mind::record_message(c, "out", "chat", text)))??;
    cell.with(|c| prism::journal::intent_close(c, &intent_id, receipt.status.as_str()))?;

    // the notice leaves this process for the owner's surface (law #3)
    trust::boundary::append(
        &core,
        &Crossing {
            direction: Direction::Out,
            channel: "ops".into(),
            counterparty: "local-web".into(),
            purpose: "operational-notice".into(),
            categories: "message".into(),
            payload_hash: trust::ids::sha256_hex(text.as_bytes()),
            size: text.len() as i64,
            trust_tag: "granted".into(),
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HubSection, MindSection, RobotConfig, RobotSection, ServerSection};

    #[test]
    fn a_notice_lands_in_the_owners_history_with_a_receipt() {
        let dir = std::env::temp_dir().join(format!("notify-{}", trust::ids::random_hex(6)));
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
                embeddings: false,
                model_cache: dir.join("models").to_string_lossy().into_owned(),
            },
            hub: HubSection::default(),
            backup: crate::config::BackupSection {
                every_hours: 0, // tests never shell out to a real backup
                script: String::new(),
            },
            sync: Default::default(),
            policy: Default::default(),
        };
        let boot = crate::boot::bootstrap(&cfg).unwrap();
        let owner = boot.robot.owner_principal;
        let before = boot.state.robot.history(owner, 0).unwrap().len();
        drop(boot);

        notify_owner(&cfg, "⚠️ test notice").unwrap();

        // a fresh boot sees it in history, and it carries a receipt
        let boot = crate::boot::bootstrap(&cfg).unwrap();
        let hist = boot.state.robot.history(owner, 0).unwrap();
        assert_eq!(hist.len(), before + 1);
        assert!(hist.last().unwrap().2.contains("test notice"));
        let handle = boot.robot.cell(owner).unwrap();
        handle
            .cell
            .with(|c| {
                assert!(prism::journal::open_intents(c).unwrap().is_empty());
                assert!(prism::receipts::count(c).unwrap() >= 1);
                Ok(())
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
