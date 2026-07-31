//! Two real instances of one robot, deliberately diverged, then synced.
//!
//! The merge rules have unit tests in `mind::merge`. These drive the whole
//! thing: package, restore, use both copies independently, sync, and check
//! the two properties that make it sync rather than copying —
//! **convergence** and **no resurrection**.

use robotd::config::{MindSection, RobotConfig, RobotSection, ServerSection};
use std::path::{Path, PathBuf};

/// These tests each boot a whole robot, and booting registers the
/// sqlite-vec auto-extension -- a process-global SQLite mutation. It is
/// `Once`-guarded, so it happens exactly once, but one thread registering
/// while another opens a connection is still a race, and it shows up here
/// as "file is not a database" from an unrelated open.
///
/// Production never meets it: `bootstrap` runs once per process, before any
/// cell is opened. Only a test harness boots several robots at once. So the
/// file serialises rather than the code pretending to be re-entrant.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn cfg_at(dir: &Path) -> RobotConfig {
    RobotConfig {
        robot: RobotSection {
            name: "bender-sync-test".into(),
            data_dir: dir.join("data").to_string_lossy().into_owned(),
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
        hub: Default::default(),
        backup: robotd::config::BackupSection {
            every_hours: 0,
            script: String::new(),
        },
        sync: Default::default(),
    }
}

/// Two instances of the same robot: one packaged, one restored from it.
fn two_instances() -> (PathBuf, RobotConfig, RobotConfig) {
    let dir = std::env::temp_dir().join(format!("sync-{}", trust::ids::random_hex(6)));
    let main = cfg_at(&dir.join("main"));

    let boot = robotd::boot::bootstrap(&main).unwrap();
    let owner = boot.robot.owner_principal;
    boot.state
        .robot
        .handle_message(owner, "remember that the sync test began".into())
        .unwrap();
    drop(boot);

    let (pkg, code) =
        robotd::package::export(&main, Some(dir.join("robot.pkg"))).unwrap();
    robotd::package::restore(&pkg, &code, &dir.join("stick"), 7899, false).unwrap();

    let stick = RobotConfig {
        robot: RobotSection {
            name: "bender-sync-test".into(),
            data_dir: dir
                .join("stick")
                .join("data")
                .to_string_lossy()
                .into_owned(),
        },
        ..cfg_at(&dir.join("stick"))
    };
    (dir, main, stick)
}

fn say(cfg: &RobotConfig, text: &str) -> String {
    let boot = robotd::boot::bootstrap(cfg).unwrap();
    let owner = boot.robot.owner_principal;
    boot.state.robot.handle_message(owner, text.into()).unwrap()
}

fn sync(from: &RobotConfig, peer: &Path) -> robotd::sync::SyncReport {
    let boot = robotd::boot::bootstrap(from).unwrap();
    robotd::sync::sync_with(&boot.robot, peer).unwrap()
}

/// The claim: use either copy, sync, and both know everything.
#[test]
fn two_instances_converge_after_being_used_apart() {
    let _serial = serial();
    let (dir, main, stick) = two_instances();

    // each copy learns something the other has never heard
    say(&main, "remember that the main machine has a blue lamp");
    say(&stick, "remember that the stick was used on a train");

    let rep = sync(&main, &dir.join("stick"));
    assert!(rep.cells >= 1, "at least the owner cell syncs");

    for cfg in [&main, &stick] {
        let facts = say(cfg, "my facts");
        assert!(facts.contains("blue lamp"), "{facts}");
        assert!(facts.contains("train"), "{facts}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// The promise that replication most easily breaks: something deleted must
/// not come back from the other machine, however often they sync.
#[test]
fn a_deletion_survives_syncing() {
    let _serial = serial();
    let (dir, main, stick) = two_instances();

    say(&main, "remember that the secret is hunter2");
    sync(&main, &dir.join("stick"));
    assert!(say(&stick, "my facts").contains("hunter2"));

    // delete it on the main machine, for real
    let listed = say(&main, "my facts");
    let n = listed
        .lines()
        .find(|l| l.contains("hunter2"))
        .and_then(|l| l.split('.').next())
        .and_then(|n| n.trim().parse::<usize>().ok())
        .expect("the fact is numbered in the registry");
    let gone = say(&main, &format!("forget fact {n}"));
    assert!(gone.contains("forgotten for real"), "{gone}");

    // sync twice, in both directions
    sync(&main, &dir.join("stick"));
    sync(&stick, &dir.join("main"));
    sync(&main, &dir.join("stick"));

    for cfg in [&main, &stick] {
        let facts = say(cfg, "my facts");
        assert!(
            !facts.contains("hunter2"),
            "a deleted fact came back: {facts}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Reminders are a state machine, and a called-off one must stay called
/// off even if the peer still believes it is live.
#[test]
fn a_cancelled_reminder_is_not_resurrected() {
    let _serial = serial();
    let (dir, main, stick) = two_instances();

    say(&main, "remind me at 23:30 to lock the door");
    sync(&main, &dir.join("stick"));
    assert!(say(&stick, "my reminders").contains("lock the door"));

    say(&main, "cancel reminder");
    sync(&main, &dir.join("stick"));
    sync(&stick, &dir.join("main"));

    for cfg in [&main, &stick] {
        let r = say(cfg, "my reminders");
        assert!(!r.contains("lock the door"), "cancelled and back again: {r}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Merging a stranger's memory into yours is never what anyone meant, and
/// it cannot be undone afterwards.
#[test]
fn syncing_with_a_different_robot_is_refused() {
    let _serial = serial();
    let (dir, main, _stick) = two_instances();
    let other = cfg_at(&dir.join("other"));
    drop(robotd::boot::bootstrap(&other).unwrap()); // a genuinely different robot

    let boot = robotd::boot::bootstrap(&main).unwrap();
    let err = robotd::sync::sync_with(&boot.robot, &dir.join("other")).unwrap_err();
    assert!(
        err.to_string().contains("different robot"),
        "expected a refusal, got: {err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Syncing again with nothing new must be a no-op, not a slow re-copy --
/// this is what makes the automatic lane safe to run every ten minutes.
#[test]
fn a_second_sync_with_nothing_new_is_quiet() {
    let _serial = serial();
    let (dir, main, _stick) = two_instances();
    say(&main, "remember that quiet syncs are the normal case");

    let first = sync(&main, &dir.join("stick"));
    assert!(!first.quiet(), "the first sync moved rows");

    let second = sync(&main, &dir.join("stick"));
    assert!(
        second.quiet(),
        "a repeat sync should move nothing: {}",
        second.summary()
    );

    let _ = std::fs::remove_dir_all(&dir);
}
