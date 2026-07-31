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
        policy: Default::default(),
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

/// S1's gate, the half a unit test cannot reach: the dial is state, so it
/// must survive a restart and travel between instances. A robot on a stick
/// that spoke to you differently from the one on your machine would be a
/// different robot wearing the same name.
#[test]
fn the_persona_dial_survives_a_restart_and_reaches_the_other_instance() {
    let _serial = serial();
    let (dir, main, stick) = two_instances();

    say(&main, "be much blunter and much more formal");
    // set it deterministically rather than depending on a model to route
    {
        let boot = robotd::boot::bootstrap(&main).unwrap();
        let owner = boot.robot.owner_principal;
        let cell = boot.robot.cell(owner).unwrap();
        cell.cell
            .with(|c| {
                soul::dial::set_value(c, soul::dial::Dimension::Directness, 95).unwrap();
                soul::dial::set_value(c, soul::dial::Dimension::Formality, 90).unwrap();
                soul::dial::pin(c, soul::dial::Dimension::Formality).unwrap();
                Ok(())
            })
            .unwrap();
    }

    // a fresh boot reads it back -- the dial is rows, not memory
    let shown = say(&main, "/soul");
    assert!(shown.contains("directness"), "{shown}");
    assert!(shown.contains("95"), "{shown}");
    assert!(shown.contains("pinned"), "{shown}");

    // and it reaches the stick
    sync(&main, &dir.join("stick"));
    let there = say(&stick, "/soul");
    assert!(there.contains("95"), "the dial did not travel: {there}");
    assert!(there.contains("pinned"), "the pin did not travel: {there}");

    // the pin holds on the far side too -- bounds travel with the value
    {
        let boot = robotd::boot::bootstrap(&stick).unwrap();
        let owner = boot.robot.owner_principal;
        let cell = boot.robot.cell(owner).unwrap();
        let refused = cell
            .cell
            .with(|c| Ok(soul::dial::set_value(c, soul::dial::Dimension::Formality, 10)))
            .unwrap();
        assert!(refused.is_err(), "a pin must survive the crossing");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Item 2's gate, and the whole point of a DURABLE interrupt: park a step,
/// **kill the process**, come back, approve, and watch it complete exactly
/// once. Nothing waits in memory.
#[test]
fn an_approval_survives_a_restart_and_runs_once() {
    let _serial = serial();
    let dir = std::env::temp_dir().join(format!("appr-{}", trust::ids::random_hex(6)));
    let mut cfg = cfg_at(&dir);
    // the owner asks to approve invites by hand
    cfg.policy.approval_required = vec!["member.invite".into()];

    let invites = |c: &RobotConfig| -> i64 {
        let boot = robotd::boot::bootstrap(c).unwrap();
        let core = boot.robot.core.lock().unwrap();
        core.query_row("SELECT COUNT(*) FROM invites", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(invites(&cfg), 0);

    // ask for one: it parks rather than running
    let asked = say(&cfg, "invite");
    assert!(asked.contains("needs your say-so"), "{asked}");
    assert!(!asked.contains("/i/"), "no link before approval: {asked}");
    assert_eq!(invites(&cfg), 0, "nothing ran");

    // the process is gone and back -- each `say` boots a fresh robot, so
    // this is a real restart, not a simulated one
    {
        let boot = robotd::boot::bootstrap(&cfg).unwrap();
        let owner = boot.robot.owner_principal;
        let cell = boot.robot.cell(owner).unwrap();
        let w = prism::approval::waiting(&cell.cell).unwrap();
        assert_eq!(w.len(), 1, "the wait survived the restart");
        assert_eq!(w[0].capability, "member.invite");
    }

    // approve
    let done = say(&cfg, "yes");
    assert!(done.contains("/i/"), "the invite should exist now: {done}");
    assert_eq!(invites(&cfg), 1);

    // and a second yes has nothing left to run
    say(&cfg, "yes");
    assert_eq!(invites(&cfg), 1, "an approval is spent once");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Declining closes the intent honestly rather than leaving it parked
/// forever or, worse, running it anyway.
#[test]
fn declining_an_approval_runs_nothing() {
    let _serial = serial();
    let dir = std::env::temp_dir().join(format!("decl-{}", trust::ids::random_hex(6)));
    let mut cfg = cfg_at(&dir);
    cfg.policy.approval_required = vec!["member.invite".into()];

    say(&cfg, "invite");
    let no = say(&cfg, "no");
    assert!(no.contains("didn't run"), "{no}");

    let boot = robotd::boot::bootstrap(&cfg).unwrap();
    let count: i64 = {
        let core = boot.robot.core.lock().unwrap();
        core.query_row("SELECT COUNT(*) FROM invites", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count, 0);
    let cell = boot.robot.cell(boot.robot.owner_principal).unwrap();
    assert!(
        prism::approval::waiting(&cell.cell).unwrap().is_empty(),
        "a decline must end the wait, not leave it asking forever"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Item 3's gate end to end: a fact marked restricted stays known to the
/// robot and stays off the wire. Checked by driving the real recall path
/// and the real filter, not by trusting the flag.
#[test]
fn a_restricted_fact_is_known_locally_and_withheld_from_a_model() {
    let _serial = serial();
    let dir = std::env::temp_dir().join(format!("class-{}", trust::ids::random_hex(6)));
    let cfg = cfg_at(&dir);

    say(&cfg, "remember that my passport number is QQ7788");
    say(&cfg, "remember that i drink green tea");

    let boot = robotd::boot::bootstrap(&cfg).unwrap();
    let owner = boot.robot.owner_principal;
    let cell = boot.robot.cell(owner).unwrap();

    // find the passport fact and restrict it
    let n = {
        let listed = cell
            .cell
            .with(|c| Ok(mind::facts::registry_list(c, 50).unwrap()))
            .unwrap();
        listed
            .iter()
            .position(|(f, _, _)| f.content.contains("QQ7788"))
            .expect("the fact is stored")
            + 1
    };
    cell.cell
        .with(|c| {
            mind::facts::classify_by_index(c, n, "restricted").unwrap();
            Ok(())
        })
        .unwrap();

    // the robot still KNOWS it -- the registry shows it, recall finds it
    let facts = say(&cfg, "my facts");
    assert!(facts.contains("QQ7788"), "still known locally: {facts}");

    // but the model-context filter drops it while keeping the rest
    let recalled = cell
        .cell
        .with(|c| Ok(mind::facts::recall(c, "passport tea", None, 10).unwrap()))
        .unwrap();
    assert!(
        recalled.iter().any(|f| f.content.contains("QQ7788")),
        "recall itself does not hide it"
    );
    let sendable: Vec<_> = recalled
        .iter()
        .filter(|f| {
            trust::classes::DataClass::parse(&f.class)
                .unwrap_or_default()
                .may_leave_the_machine()
        })
        .collect();
    assert!(
        !sendable.iter().any(|f| f.content.contains("QQ7788")),
        "a restricted fact must not reach model context"
    );
    assert!(
        sendable.iter().any(|f| f.content.contains("green tea")),
        "and the filter must not be a blanket refusal"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
