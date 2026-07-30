//! The reminder scheduler: the commitment ledger's future axis made real,
//! now per principal -- every member's cell gets its own fires. Due
//! reminders fire through the transactional outbox (Q11) as journaled
//! system intents with receipts; the Second Law as a background lane.

use crate::robot::RobotCore;
use std::sync::Arc;
use std::time::Duration;
use trust::boundary::{Crossing, Direction};

pub fn spawn(robot: Arc<RobotCore>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let robot = robot.clone();
            let fired = tokio::task::spawn_blocking(move || fire_all_due(&robot)).await;
            if let Ok(Err(e)) = fired {
                tracing::error!("scheduler: {e:#}");
            }
        }
    });
}

fn fire_all_due(robot: &RobotCore) -> anyhow::Result<usize> {
    let mut total = 0;
    for principal in robot.principals_active()? {
        total += fire_due_for(robot, principal)?;
    }
    Ok(total)
}

fn fire_due_for(robot: &RobotCore, principal: i64) -> anyhow::Result<usize> {
    let now = trust::ids::ts_ms();
    let handle = robot.cell(principal)?;
    let due = {
        let cell = handle
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("cell lock poisoned"))?;
        mind::reminders::due_active(&cell, now)?
    };
    let mut count = 0;

    for rem in due {
        let text = format!("⏰ reminder: {}", rem.about);
        {
            let cell = handle
                .conn
                .lock()
                .map_err(|_| anyhow::anyhow!("cell lock poisoned"))?;
            let intent_id = trust::ids::new_id("int");
            prism::journal::intent_open(
                &cell,
                &intent_id,
                &serde_json::json!({
                    "system": "reminder.fire",
                    "reminder_id": rem.id,
                    "about": rem.about,
                    "fire_at": rem.fire_at,
                })
                .to_string(),
            )?;
            let (effect_id, _) =
                prism::outbox::enqueue(&cell, &intent_id, "surface:chat", &text)?;
            mind::reminders::mark_fired(&cell, &rem.id)?;
            let outcome = prism::types::Outcome {
                step_id: trust::ids::new_id("pstep"),
                ok: true,
                evidence: vec![prism::types::Evidence {
                    kind: "row".into(),
                    provider: "cell".into(),
                    external_id: rem.id.clone(),
                    hash: trust::ids::sha256_hex(rem.about.as_bytes()),
                    ts: now,
                }],
                detail: format!("reminder fired: {}", rem.about),
            };
            prism::journal::step(
                &cell,
                &intent_id,
                "outcome",
                &serde_json::to_string(&outcome)?,
                None,
            )?;
            let receipt = prism::lifecycle::build_receipt(&intent_id, &[outcome]);
            let receipt = prism::receipts::store(&cell, &receipt)?;
            prism::journal::step(
                &cell,
                &intent_id,
                "receipt",
                &serde_json::json!({
                    "receipt_id": receipt.receipt_id,
                    "status": receipt.status.as_str()
                })
                .to_string(),
                None,
            )?;
            mind::record_message(&cell, "out", "chat", &text)?;
            prism::outbox::mark(&cell, &effect_id, "sent", None)?;
            prism::outbox::mark(&cell, &effect_id, "confirmed", None)?;
            prism::journal::intent_close(&cell, &intent_id, receipt.status.as_str())?;
        }
        {
            let core = robot
                .core
                .lock()
                .map_err(|_| anyhow::anyhow!("core lock poisoned"))?;
            let _ = trust::boundary::append(
                &core,
                &Crossing {
                    direction: Direction::Out,
                    channel: "chat".into(),
                    counterparty: "local-web".into(),
                    purpose: "reminder-delivery".into(),
                    categories: "message".into(),
                    payload_hash: trust::ids::sha256_hex(text.as_bytes()),
                    size: text.len() as i64,
                    trust_tag: "owner".into(),
                },
            );
        }
        robot.notify(principal);
        tracing::info!("reminder fired for principal {principal}: {}", rem.about);
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn test_robot() -> (Arc<RobotCore>, std::path::PathBuf) {
        mind::install_vec();
        let dir = std::env::temp_dir().join(format!("sched-{}", trust::ids::random_hex(6)));
        std::fs::create_dir_all(dir.join("cells")).unwrap();
        std::fs::create_dir_all(dir.join("media")).unwrap();
        let keys = trust::keys::KeyChain::load_or_create(&dir.join("kek.key")).unwrap();
        let core =
            trust::cells::open_encrypted(&dir.join("core.db"), &keys.core_db_key()).unwrap();
        trust::schema::init_core(&core).unwrap();
        core.execute(
            "INSERT INTO principals(kind, display_name, cell_id, created_at) \
             VALUES ('owner','owner','owner',?1)",
            params![trust::ids::ts_ms()],
        )
        .unwrap();
        let owner = core.last_insert_rowid();
        let robot = Arc::new(RobotCore::new(
            owner,
            Arc::new(std::sync::Mutex::new(core)),
            keys,
            dir.clone(),
            None,
            None,
            None,
            0,
            "http://127.0.0.1:0".into(),
            "bender-test".into(),
        ));
        (robot, dir)
    }

    #[test]
    fn due_reminders_fire_once_with_receipts() {
        let (robot, dir) = test_robot();
        let owner = robot.owner_principal;
        let handle = robot.cell(owner).unwrap();
        {
            let cell = handle.conn.lock().unwrap();
            mind::reminders::create(&cell, "int_past", trust::ids::ts_ms() - 1000, "call mark")
                .unwrap();
            mind::reminders::create(
                &cell,
                "int_future",
                trust::ids::ts_ms() + 3_600_000,
                "later",
            )
            .unwrap();
        }

        assert_eq!(fire_all_due(&robot).unwrap(), 1);
        assert_eq!(fire_all_due(&robot).unwrap(), 0);

        let cell = handle.conn.lock().unwrap();
        assert_eq!(mind::reminders::count_active(&cell).unwrap(), 1);
        let msgs = mind::messages_after(&cell, 0, 10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].2.contains("call mark"));
        assert!(prism::journal::open_intents(&cell).unwrap().is_empty());
        assert_eq!(prism::receipts::count(&cell).unwrap(), 1);
        drop(cell);
        let _ = std::fs::remove_dir_all(dir);
    }
}
