//! The off-site backup lane.
//!
//! Backups run inside the Robot rather than from a launchd agent. That is
//! not laziness -- on macOS a background agent is blocked by TCC from
//! reading `~/Documents`, and the only fixes are granting Full Disk Access
//! to `/bin/bash` (every script on the machine gets your whole disk) or
//! moving the Robot out of Documents. The Robot already runs with the
//! owner's permissions and already has background lanes (arch sec 2a), so
//! the schedule belongs here.
//!
//! It also makes the honest thing easy: a failure is reported into the
//! owner's chat by the same process that noticed it, rather than dying in a
//! log file nobody opens.
//!
//! Consequence, stated plainly: backups happen while the Robot is running.
//! If it is off for a week there is no new backup -- and also no new
//! memory to lose, so the copy on the box still matches the last state that
//! existed.

use crate::robot::RobotCore;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use trust::schema;

const LAST_RUN_KEY: &str = "backup_last_run_ms";

pub fn spawn(robot: Arc<RobotCore>, every_hours: u64, script: String) {
    if every_hours == 0 {
        tracing::info!("off-site backup lane disabled (backup.every_hours = 0)");
        return;
    }
    tokio::spawn(async move {
        // settle first: don't compete with boot, and don't fire the instant
        // the owner starts the robot
        tokio::time::sleep(Duration::from_secs(120)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(600)); // check every 10 min
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let robot = robot.clone();
            let script = script.clone();
            let done =
                tokio::task::spawn_blocking(move || run_if_due(&robot, every_hours, &script))
                    .await;
            if let Ok(Err(e)) = done {
                tracing::error!("backup lane: {e:#}");
            }
        }
    });
}

fn last_run_ms(robot: &RobotCore) -> anyhow::Result<i64> {
    let core = robot
        .core
        .lock()
        .map_err(|_| anyhow::anyhow!("core lock poisoned"))?;
    Ok(schema::meta_get(&core, LAST_RUN_KEY)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

fn set_last_run(robot: &RobotCore, ts: i64) -> anyhow::Result<()> {
    let core = robot
        .core
        .lock()
        .map_err(|_| anyhow::anyhow!("core lock poisoned"))?;
    schema::meta_set(&core, LAST_RUN_KEY, &ts.to_string())?;
    Ok(())
}

pub fn run_if_due(robot: &RobotCore, every_hours: u64, script: &str) -> anyhow::Result<bool> {
    let now = trust::ids::ts_ms();
    let due_after = every_hours as i64 * 3_600_000;
    if now - last_run_ms(robot)? < due_after {
        return Ok(false);
    }
    // Mark the attempt BEFORE running. A destination that is failing must
    // not be retried every ten minutes -- that turns one broken backup into
    // a stream of identical complaints in the owner's chat.
    set_last_run(robot, now)?;

    tracing::info!("off-site backup starting");
    let out = Command::new("/bin/bash").arg(script).output();

    match out {
        Ok(o) if o.status.success() => {
            let tail = String::from_utf8_lossy(&o.stdout)
                .lines()
                .next_back()
                .unwrap_or("")
                .to_string();
            tracing::info!("off-site backup ok: {tail}");
            Ok(true)
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let stdout = String::from_utf8_lossy(&o.stdout);
            let why = stderr
                .lines()
                .chain(stdout.lines())
                .rfind(|l| l.starts_with("!!") || l.contains("FAILED"))
                .unwrap_or("no detail in output")
                .to_string();
            tracing::error!("off-site backup FAILED: {why}");
            robot.tell_owner(&format!(
                "⚠️ my off-site backup just failed.\n\n{why}\n\ni'm still running \
                 normally and nothing is lost -- but the copy on the storage box is \
                 now stale, so a disk failure would cost everything since the last \
                 good backup. i'll try again in {every_hours}h; run \
                 scripts/backup-offsite.sh by hand if you want it sooner."
            ))?;
            Ok(false)
        }
        Err(e) => {
            tracing::error!("off-site backup could not start: {e}");
            robot.tell_owner(&format!(
                "⚠️ my off-site backup could not even start: {e}. the backup script \
                 may be missing or not executable."
            ))?;
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackupSection, HubSection, MindSection, RobotConfig, RobotSection, ServerSection};

    fn test_cfg(dir: &std::path::Path) -> RobotConfig {
        RobotConfig {
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
            backup: BackupSection {
                every_hours: 0,
                script: String::new(),
            },
        }
    }

    fn script(dir: &std::path::Path, name: &str, body: &str) -> String {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().into_owned()
    }

    /// The whole reason this lane exists in-process: a failed backup has to
    /// reach the owner. A job that fails into a log file is worse than no
    /// job, because it looks like it is working.
    #[test]
    fn a_failed_backup_tells_the_owner_in_chat() {
        let dir = std::env::temp_dir().join(format!("lane-{}", trust::ids::random_hex(6)));
        let cfg = test_cfg(&dir);
        let boot = crate::boot::bootstrap(&cfg).unwrap();
        let owner = boot.robot.owner_principal;
        let before = boot.state.robot.history(owner, 0).unwrap().len();

        let bad = script(
            &dir,
            "fail.sh",
            "#!/usr/bin/env bash\necho '!! storage box unreachable: Connection timed out'\nexit 1\n",
        );
        let ok = run_if_due(&boot.robot, 24, &bad).unwrap();
        assert!(!ok, "a failing script must not report success");

        let hist = boot.state.robot.history(owner, 0).unwrap();
        assert_eq!(hist.len(), before + 1, "the owner should have been told");
        let msg = &hist.last().unwrap().2;
        assert!(msg.contains("backup just failed"), "{msg}");
        assert!(
            msg.contains("storage box unreachable"),
            "the notice must carry the actual reason: {msg}"
        );

        // ...and it does not nag: the attempt is marked before running, so a
        // persistently broken destination complains once per interval
        let again = run_if_due(&boot.robot, 24, &bad).unwrap();
        assert!(!again);
        assert_eq!(
            boot.state.robot.history(owner, 0).unwrap().len(),
            before + 1,
            "a still-broken backup must not repeat itself every check"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_successful_backup_is_quiet() {
        let dir = std::env::temp_dir().join(format!("lane-{}", trust::ids::random_hex(6)));
        let cfg = test_cfg(&dir);
        let boot = crate::boot::bootstrap(&cfg).unwrap();
        let owner = boot.robot.owner_principal;
        let before = boot.state.robot.history(owner, 0).unwrap().len();

        let good = script(&dir, "ok.sh", "#!/usr/bin/env bash\necho '==> done: 3 backups'\n");
        assert!(run_if_due(&boot.robot, 24, &good).unwrap());
        assert_eq!(
            boot.state.robot.history(owner, 0).unwrap().len(),
            before,
            "a working backup should say nothing at all"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
