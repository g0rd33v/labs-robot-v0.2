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
    let cell = &handle.cell;
    let due = cell.with(|c| Ok(mind::reminders::due_active(c, now)))??;
    let mut count = 0;

    let lang = crate::robot::cell_lang(cell);
    let speak = crate::render::Speak {
        gateway: robot.gateway.clone(),
    };
    for rem in due {
        // a reminder firing at 03:00 speaks the language its person uses
        let text = prism::lifecycle::Renderer::render(
            &speak,
            &lang,
            &[prism::types::ReplyPart::Say(prism::types::Rendering::new(
                "reminder_fired",
                serde_json::json!({ "about": rem.about }),
            ))],
            &[],
        );
        {
            let intent_id = trust::ids::new_id("int");
            let open_json = serde_json::json!({
                "system": "reminder.fire",
                "reminder_id": rem.id,
                "about": rem.about,
                "fire_at": rem.fire_at,
            })
            .to_string();
            cell.with(|c| prism::journal::intent_open(c, &intent_id, &open_json))?;
            let (effect_id, _) =
                cell.with(|c| prism::outbox::enqueue(c, &intent_id, "surface:chat", &text))?;
            cell.with(|c| Ok(mind::reminders::mark_fired(c, &rem.id)))??;
            // the commitment really closed: the row moved to `fired`
            let outcome = prism::types::Outcome::attested(
                trust::ids::new_id("pstep"),
                vec![prism::types::Evidence {
                    kind: "row".into(),
                    provider: "cell".into(),
                    external_id: rem.id.clone(),
                    hash: trust::ids::sha256_hex(rem.about.as_bytes()),
                    ts: now,
                }],
                format!("reminder fired: {}", rem.about),
                prism::types::Rendering::new(
                    "reminder_fired",
                    serde_json::json!({ "about": rem.about }),
                ),
            );
            let outcome_json = serde_json::to_string(&outcome)?;
            cell.with(|c| prism::journal::step(c, &intent_id, "outcome", &outcome_json, None))?;
            let receipt = prism::lifecycle::build_receipt(&intent_id, &[outcome]);
            let receipt = cell.with(|c| prism::receipts::store(c, &receipt))?;
            let receipt_json = serde_json::json!({
                "receipt_id": receipt.receipt_id,
                "status": receipt.status.as_str()
            })
            .to_string();
            cell.with(|c| prism::journal::step(c, &intent_id, "receipt", &receipt_json, None))?;
            cell.with(|c| prism::outbox::mark(c, &effect_id, "sent", None))?;
            cell.with(|c| Ok(mind::record_message(c, "out", "chat", &text)))??;
            cell.with(|c| prism::outbox::mark(c, &effect_id, "confirmed", None))?;
            cell.with(|c| prism::journal::intent_close(c, &intent_id, receipt.status.as_str()))?;
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
        handle
            .cell
            .with(|c| {
                mind::reminders::create(c, "int_past", trust::ids::ts_ms() - 1000, "call mark")
                    .unwrap();
                mind::reminders::create(c, "int_future", trust::ids::ts_ms() + 3_600_000, "later")
                    .unwrap();
                Ok(())
            })
            .unwrap();

        assert_eq!(fire_all_due(&robot).unwrap(), 1);
        assert_eq!(fire_all_due(&robot).unwrap(), 0);

        handle
            .cell
            .with(|c| {
                assert_eq!(mind::reminders::count_active(c).unwrap(), 1);
                let msgs = mind::messages_after(c, 0, 10).unwrap();
                assert_eq!(msgs.len(), 1);
                assert!(msgs[0].2.contains("call mark"));
                assert!(prism::journal::open_intents(c).unwrap().is_empty());
                assert_eq!(prism::receipts::count(c).unwrap(), 1);
                Ok(())
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
