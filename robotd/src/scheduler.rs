//! The reminder scheduler: the commitment ledger's future axis made real.
//! Due reminders fire through the transactional outbox (Q11) as their own
//! journaled system intents with receipts -- the Second Law (never silently
//! drop a request) as a background lane.

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
            let fired = tokio::task::spawn_blocking(move || fire_due(&robot)).await;
            if let Ok(Err(e)) = fired {
                tracing::error!("scheduler: {e:#}");
            }
        }
    });
}

fn fire_due(robot: &RobotCore) -> anyhow::Result<usize> {
    let now = trust::ids::ts_ms();
    let mut count = 0;

    let due = {
        let cell = robot
            .owner_cell
            .lock()
            .map_err(|_| anyhow::anyhow!("cell lock poisoned"))?;
        mind::reminders::due_active(&cell, now)?
    };

    for rem in due {
        let text = format!("⏰ reminder: {}", rem.about);
        {
            let cell = robot
                .owner_cell
                .lock()
                .map_err(|_| anyhow::anyhow!("cell lock poisoned"))?;
            // a system intent of its own: journaled, receipted, closed
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
            // the delivery is an effect: through the outbox, deduped
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
            // delivered into the message store; the chat poll renders it
            mind::record_message(&cell, "out", "chat", &text)?;
            prism::outbox::mark(&cell, &effect_id, "sent", None)?;
            prism::outbox::mark(&cell, &effect_id, "confirmed", None)?;
            prism::journal::intent_close(&cell, &intent_id, receipt.status.as_str())?;
        }
        // the crossing: the reminder text leaves the process via the chat
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
        tracing::info!("reminder fired: {}", rem.about);
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex;

    fn robot_with_cell() -> (Arc<RobotCore>, std::path::PathBuf) {
        mind::install_vec();
        let dir = std::env::temp_dir().join(format!("sched-{}", trust::ids::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let key = trust::keys::KeyChain::new_dek();
        let core = trust::cells::open_encrypted(&dir.join("core.db"), &key).unwrap();
        trust::schema::init_core(&core).unwrap();
        let cell = trust::cells::open_encrypted(&dir.join("owner.db"), &key).unwrap();
        prism::init_cell_schema(&cell).unwrap();
        mind::init_cell_schema(&cell).unwrap();
        let robot = Arc::new(RobotCore {
            owner_principal: 1,
            core: Arc::new(Mutex::new(core)),
            owner_cell: Mutex::new(cell),
            embedder: None,
            gateway: None,
            research: None,
            ultra_daily_cap: 0,
        });
        (robot, dir)
    }

    fn cell_do<T>(robot: &RobotCore, f: impl FnOnce(&Connection) -> T) -> T {
        let cell = robot.owner_cell.lock().unwrap();
        f(&cell)
    }

    #[test]
    fn due_reminders_fire_once_with_receipts() {
        let (robot, dir) = robot_with_cell();
        cell_do(&robot, |c| {
            mind::reminders::create(c, "int_past", trust::ids::ts_ms() - 1000, "call mark")
                .unwrap();
            mind::reminders::create(c, "int_future", trust::ids::ts_ms() + 3_600_000, "later")
                .unwrap();
        });

        assert_eq!(fire_due(&robot).unwrap(), 1); // only the due one
        assert_eq!(fire_due(&robot).unwrap(), 0); // and only once

        cell_do(&robot, |c| {
            // fired, not active; the future one untouched
            assert_eq!(mind::reminders::count_active(c).unwrap(), 1);
            // delivered to the message store
            let msgs = mind::messages_after(c, 0, 10).unwrap();
            assert_eq!(msgs.len(), 1);
            assert!(msgs[0].2.contains("call mark"));
            // journaled with a terminal receipt, intent closed
            assert!(prism::journal::open_intents(c).unwrap().is_empty());
            assert_eq!(prism::receipts::count(c).unwrap(), 1);
        });
        let _ = std::fs::remove_dir_all(dir);
    }
}
