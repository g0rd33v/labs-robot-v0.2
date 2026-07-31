//! The sync lane: keep configured peers in agreement, automatically.
//!
//! The owner chose automatic-when-present, so this watches the configured
//! paths and syncs the ones that are actually there. A stick that is not
//! plugged in is not an error and is not reported — absence is the normal
//! state of a removable disk, and a robot that complains every ten minutes
//! about a drawer is a robot people stop reading.
//!
//! What IS reported, in chat, is a sync that ran and changed something, and
//! a sync that failed while the peer was present. The first is the owner's
//! memory moving between machines and they should know; the second is
//! something to fix.

use crate::robot::RobotCore;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub fn spawn(robot: Arc<RobotCore>, peers: Vec<String>, every_minutes: u64) {
    if peers.is_empty() || every_minutes == 0 {
        tracing::info!("sync lane disabled (no peers, or sync.every_minutes = 0)");
        return;
    }
    tokio::spawn(async move {
        // let boot settle before touching another instance's database
        tokio::time::sleep(Duration::from_secs(20)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(every_minutes * 60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let robot = robot.clone();
            let peers = peers.clone();
            let _ = tokio::task::spawn_blocking(move || sweep(&robot, &peers)).await;
        }
    });
}

/// Sync every peer that is present right now.
pub fn sweep(robot: &RobotCore, peers: &[String]) {
    for p in peers {
        let path = resolve(p);
        if !present(&path) {
            // not plugged in. That is not news.
            continue;
        }
        match crate::sync::sync_with(robot, &path) {
            Ok(rep) => {
                if rep.quiet() {
                    tracing::debug!("sync: {} already in agreement", rep.peer_instance);
                    continue;
                }
                tracing::info!("{}", rep.summary());
                // the owner's memory moved between machines: say so
                let _ = robot.tell_owner(&rep.summary());
            }
            Err(e) => {
                // present but unusable is worth a word -- a wrong path, a
                // different robot, a stick going bad
                tracing::error!("sync with {}: {e:#}", path.display());
                let _ = robot.tell_owner(&format!(
                    "couldn't sync with {}: {e}",
                    path.display()
                ));
            }
        }
    }
}

fn resolve(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

/// Present means "a robot lives here", not merely "the path exists": a
/// mount point with nothing under it is a stick that is not plugged in.
fn present(path: &Path) -> bool {
    let data = if path.join("data").join("core.db").exists() {
        path.join("data")
    } else {
        path.to_path_buf()
    };
    data.join("core.db").exists() && data.join("kek.key").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unplugged_stick_is_not_a_robot() {
        let dir = std::env::temp_dir().join(format!("sync-{}", trust::ids::random_hex(6)));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        // the mount point exists but holds nothing
        assert!(!present(&dir));

        std::fs::write(dir.join("data").join("core.db"), b"x").unwrap();
        assert!(!present(&dir), "a core without its key is not usable either");
        std::fs::write(dir.join("data").join("kek.key"), b"x").unwrap();
        assert!(present(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn home_relative_peers_resolve() {
        std::env::set_var("HOME", "/home/x");
        assert_eq!(resolve("~/stick"), PathBuf::from("/home/x/stick"));
        assert_eq!(resolve("/mnt/stick"), PathBuf::from("/mnt/stick"));
    }
}
