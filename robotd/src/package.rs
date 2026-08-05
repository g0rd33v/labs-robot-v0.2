//! The Robot Package (arch sec 8): the deep answer to "transferable".
//! `robotd package` exports essence + keys as one sealed file, protected by
//! a one-time code printed to the owner; `robotd restore` unpacks it on any
//! blank directory into a ready-to-run Robot -- memory, receipts, persona
//! intact. Move (sec 8a): the flow is symmetric, state travels both ways.

use crate::archive::{self, StageSpec};
use crate::config::RobotConfig;
use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use trust::keys::KeyChain;

/// Export the Robot Package. Returns (package path, one-time code).
pub fn export(cfg: &RobotConfig, dest: Option<PathBuf>) -> anyhow::Result<(PathBuf, String)> {
    let data_dir = Path::new(&cfg.robot.data_dir);
    let keys = KeyChain::load_or_create(&data_dir.join("kek.key"))?;
    let ts = trust::ids::ts_ms();
    let staging = std::env::temp_dir().join(format!("bender-pkg-{ts}"));
    std::fs::create_dir_all(&staging)?;

    // essence: every cell, media, core last -- and the keys, because the
    // package IS the robot and the one-time code is the new perimeter
    // (sec 8: recoverable without any hosted service)
    let staged = archive::stage(
        &StageSpec {
            data_dir,
            inner_prefix: "data",
            include_keyfile: true,
        },
        &keys,
        &staging,
    )?;

    let manifest = serde_json::json!({
        "kind": "bender-robot-package",
        "format_version": 1,
        "robot_id": staged.robot_id,
        "robot_name": cfg.robot.name,
        "runtime_version": env!("CARGO_PKG_VERSION"),
        "created_at": ts,
        "cells": staged.cells,
        "media_files": staged.media_files,
        "excluded": ["models (runtime, re-fetchable)", "backups"],
    });
    std::fs::write(
        staging.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;

    let code = trust::ids::random_hex(5); // 10 hex chars, carried by the owner
    let sealed = archive::seal_dir(&staging, &trust::keys::passphrase_key(&code))?;
    let out = dest.unwrap_or_else(|| PathBuf::from(format!("bender-robot-{ts}.pkg")));
    std::fs::write(&out, sealed)?;
    std::fs::remove_dir_all(&staging).ok();
    Ok((out, code))
}

/// Restore a package into `into` as a ready-to-run Robot directory
/// (`robot.toml` + `data/`). Refuses to clobber existing data unless
/// `force` -- and even then the old data is moved aside, never deleted.
pub fn restore(
    pkg: &Path,
    code: &str,
    into: &Path,
    port: u16,
    force: bool,
) -> anyhow::Result<()> {
    let sealed = std::fs::read(pkg).with_context(|| format!("reading {}", pkg.display()))?;

    if into.join("data").exists() {
        if !force {
            bail!(
                "{} already holds a robot (data/ exists); use --force to move it aside",
                into.display()
            );
        }
        let aside = into.join(format!("data.pre-restore-{}", trust::ids::ts_ms()));
        std::fs::rename(into.join("data"), &aside)?;
        println!("existing data moved aside to {}", aside.display());
    }
    archive::unseal_into(&sealed, &trust::keys::passphrase_key(code), into)?;

    // manifest sanity + integrity check: the cells must open with the
    // travelled keys (sec 8a gate: integrity check on the target)
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(into.join("manifest.json"))?)?;
    if manifest["kind"] != "bender-robot-package" {
        bail!("not a robot package (manifest kind mismatch)");
    }
    let keys = KeyChain::load_or_create(&into.join("data").join("kek.key"))?;
    let core = trust::cells::open_encrypted(&into.join("data").join("core.db"), &keys.core_db_key())
        .context("integrity check: core.db did not open with the travelled keys")?;
    let cells: i64 = core.query_row("SELECT count(*) FROM cell_keys", [], |r| r.get(0))?;
    let robot_id = trust::schema::meta_get(&core, "robot_id")?.unwrap_or_default();

    // The restored copy is the SAME ROBOT and a DIFFERENT INSTANCE. Clearing
    // the instance id makes it mint its own on first boot, which is what
    // lets the two of them sync at all: a deletion has to be attributable,
    // and a watermark against yourself means nothing.
    core.execute("DELETE FROM meta WHERE key = 'instance_id'", [])?;
    // and it has never synced with anyone under its new identity
    core.execute("DELETE FROM sync_peers", []).ok();
    drop(core);

    // a ready-to-run config: same robot, this port; embeddings off until
    // models are (re)fetched -- recall degrades to FTS + recency, honestly
    std::fs::write(
        into.join("robot.toml"),
        format!(
            "# restored robot package -- {robot_id}\n\
             [robot]\nname = \"{name}\"\ndata_dir = \"./data\"\n\n\
             [server]\nhost = \"127.0.0.1\"\nport = {port}\n\n\
             [mind]\n# models are runtime, not essence; flip to true to re-fetch\n\
             embeddings = false\nmodel_cache = \"./data/models\"\n",
            name = manifest["robot_name"].as_str().unwrap_or("bender"),
        ),
    )?;
    println!(
        "restored {robot_id} into {} ({cells} cells, integrity ok) -- run it there on port {port}",
        into.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MindSection, RobotConfig, RobotSection, ServerSection};

    fn cfg_at(dir: &Path) -> RobotConfig {
        RobotConfig {
            robot: RobotSection {
                name: "bender-test".into(),
                data_dir: dir.join("data").to_string_lossy().into_owned(),
            },
            server: ServerSection {
                host: "127.0.0.1".into(),
                port: 0,
                public_base: String::new(),
            path_prefix: String::new(),
            },
            mind: MindSection {
                embeddings: false,
                model_cache: dir.join("models").to_string_lossy().into_owned(),
            },
            hub: Default::default(),
            backup: crate::config::BackupSection {
                every_hours: 0, // tests never shell out to a real backup
                script: String::new(),
            },
            sync: Default::default(),
            policy: Default::default(),
            update: Default::default(),
        }
    }

    #[test]
    fn package_restore_resumes_the_same_robot() {
        let dir = std::env::temp_dir().join(format!("pkg-{}", trust::ids::random_hex(6)));
        let cfg = cfg_at(&dir);
        let boot = crate::boot::bootstrap(&cfg).unwrap();
        let owner = boot.robot.owner_principal;
        boot.state
            .robot
            .handle_message(owner, "remember that the package test ran".into())
            .unwrap();
        drop(boot);

        let (pkg, code) = export(&cfg, Some(dir.join("robot.pkg"))).unwrap();
        // sealed: wrong code must fail
        assert!(restore(&pkg, "wrongcode", &dir.join("usb"), 7801, false).is_err());
        restore(&pkg, &code, &dir.join("usb"), 7801, false).unwrap();

        // the restored robot boots in the new home and remembers
        let usb_cfg = crate::config::load(&dir.join("usb").join("robot.toml")).unwrap();
        // data_dir in the restored toml is relative; make it absolute for the test
        let usb_cfg = RobotConfig {
            robot: RobotSection {
                name: usb_cfg.robot.name,
                data_dir: dir.join("usb").join("data").to_string_lossy().into_owned(),
            },
            ..cfg_at(&dir.join("usb"))
        };
        let boot2 = crate::boot::bootstrap(&usb_cfg).unwrap();
        let owner2 = boot2.robot.owner_principal;
        let reply = boot2
            .state
            .robot
            .handle_message(owner2, "what do you remember".into())
            .unwrap();
        assert!(reply.contains("package test ran"), "{reply}");

        // restoring over an existing robot requires --force and keeps the old
        assert!(restore(&pkg, &code, &dir.join("usb"), 7801, false).is_err());
        restore(&pkg, &code, &dir.join("usb"), 7801, true).unwrap();
        let aside: Vec<_> = std::fs::read_dir(dir.join("usb"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("data.pre-restore"))
            .collect();
        assert_eq!(aside.len(), 1, "old data moved aside, not deleted");
        let _ = std::fs::remove_dir_all(dir);
    }
}
