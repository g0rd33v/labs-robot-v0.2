//! The maintenance lane: watchdog + zombie sweeper.
//!
//! Watchdog (sec 6a, law 2): any turn "in" with no "out" within 60s -> an
//! error trace and an owner-visible alert. In Bender this is journal-
//! backed: an intent without a terminal close IS the alarm condition.
//!
//! Zombie sweeper (Q12): interactive intents reach a terminal state or an
//! honest failed receipt; anything open past its TTL (5 min) is closed by
//! the sweeper with the reason on the receipt.

use crate::robot::RobotCore;
use std::sync::Arc;
use std::time::Duration;

const WATCHDOG_MS: i64 = 60_000;
const ZOMBIE_TTL_MS: i64 = 5 * 60_000;

pub fn spawn(robot: Arc<RobotCore>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let robot = robot.clone();
            let swept = tokio::task::spawn_blocking(move || sweep(&robot)).await;
            if let Ok(Err(e)) = swept {
                tracing::error!("maintenance: {e:#}");
            }
        }
    });
}

pub fn sweep(robot: &RobotCore) -> anyhow::Result<(usize, usize)> {
    let mut alerted = 0;
    let mut closed = 0;
    for principal in robot.principals_active()? {
        let handle = robot.cell(principal)?;
        let cell = &handle.cell;
        let now = trust::ids::ts_ms();
        // short locks only: the whole point of this lane is to observe turns
        // that are stuck, and it cannot do that while holding their cell
        let open = cell.with(prism::journal::open_intents)?;
        for intent_id in open {
            let opened_at: i64 = cell.sql(|c| {
                c.query_row(
                    "SELECT min(started_at) FROM journal WHERE intent_id = ?1",
                    rusqlite::params![intent_id],
                    |r| r.get(0),
                )
            })?;
            let age = now - opened_at;

            if age > ZOMBIE_TTL_MS {
                // Q12: past TTL -> honest failed receipt, closed, visible
                let receipt = prism::lifecycle::failed_receipt(
                    &intent_id,
                    &format!(
                        "intent exceeded its {}s TTL and was closed by the zombie \
                         sweeper; no further effects will run for it",
                        ZOMBIE_TTL_MS / 1000
                    ),
                    "zombie-sweeper",
                );
                let receipt = cell.with(|c| prism::receipts::store(c, &receipt))?;
                cell.with(|c| {
                    prism::journal::intent_close(c, &intent_id, receipt.status.as_str())
                })?;
                let note = format!(
                    "⚠️ a turn of mine ({}) hung past its time limit and was closed \
                     honestly -- nothing was silently dropped, the receipt records it.",
                    &intent_id[..12.min(intent_id.len())]
                );
                cell.with(|c| Ok(mind::record_message(c, "out", "chat", &note)))??;
                tracing::error!("zombie sweeper closed {intent_id} (age {age}ms)");
                closed += 1;
                robot.notify(principal);
            } else if age > WATCHDOG_MS {
                // watchdog: alert once per intent
                let marker = format!("watchdog:{intent_id}");
                let seen: Option<String> = cell
                    .sql(|c| {
                        c.query_row(
                            "SELECT value FROM cell_meta WHERE key = ?1",
                            rusqlite::params![marker],
                            |r| r.get(0),
                        )
                    })
                    .ok();
                if seen.is_none() {
                    cell.sql(|c| {
                        c.execute(
                            "INSERT OR IGNORE INTO cell_meta(key, value) VALUES (?1, '1')",
                            rusqlite::params![marker],
                        )
                    })?;
                    tracing::error!(
                        "WATCHDOG: intent {intent_id} open {age}ms without a terminal receipt"
                    );
                    alerted += 1;
                }
            }
        }
    }
    Ok((alerted, closed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn test_robot() -> (Arc<RobotCore>, std::path::PathBuf) {
        mind::install_vec();
        let dir = std::env::temp_dir().join(format!("maint-{}", trust::ids::random_hex(6)));
        std::fs::create_dir_all(dir.join("cells")).unwrap();
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
    fn sweeper_closes_zombies_and_watchdog_alerts_once() {
        let (robot, dir) = test_robot();
        let handle = robot.cell(robot.owner_principal).unwrap();
        handle
            .cell
            .sql(|c| {
                // a synthetic hung intent, 2 minutes old: watchdog territory
                c.execute(
                    "INSERT INTO journal(step_id, intent_id, seq, kind, payload_json, started_at, completed_at) \
                     VALUES ('step_w', 'int_watch', 0, 'intent_open', '{}', ?1, ?1)",
                    params![trust::ids::ts_ms() - 2 * 60_000],
                )?;
                // a synthetic zombie, 6 minutes old: sweeper territory
                c.execute(
                    "INSERT INTO journal(step_id, intent_id, seq, kind, payload_json, started_at, completed_at) \
                     VALUES ('step_z', 'int_zombie', 0, 'intent_open', '{}', ?1, ?1)",
                    params![trust::ids::ts_ms() - 6 * 60_000],
                )
            })
            .unwrap();

        let (alerted, closed) = sweep(&robot).unwrap();
        assert_eq!(alerted, 1, "watchdog alerts the 2-minute intent");
        assert_eq!(closed, 1, "sweeper closes the 6-minute zombie");

        // second sweep: watchdog does not re-alert, nothing left to close
        let (alerted2, closed2) = sweep(&robot).unwrap();
        assert_eq!((alerted2, closed2), (0, 0));

        handle
            .cell
            .with(|c| {
                // the zombie is closed with an honest failed receipt
                let receipt = prism::receipts::get(c, "int_zombie").unwrap().unwrap();
                assert_eq!(receipt.status.as_str(), "failed");
                assert!(receipt.claims[0].claim.contains("zombie"));
                // the watchdog intent is still open (alerted, not killed yet)
                assert!(prism::journal::open_intents(c)
                    .unwrap()
                    .contains(&"int_watch".to_string()));
                Ok(())
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
