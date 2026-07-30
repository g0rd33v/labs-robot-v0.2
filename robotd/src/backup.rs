//! Encrypted backup (Q38): per-cell online snapshot + content-addressed
//! media + core last -> one sealed tarball with a manifest. Restore requires
//! the same kek.key -- the backup shreds with the keys, like everything else.
//!
//! The staging/sealing machinery is shared with `package` (see `archive`).

use crate::archive::{self, StageSpec};
use crate::config::RobotConfig;
use anyhow::Context;
use std::path::{Path, PathBuf};
use trust::keys::KeyChain;

fn backup_key(keys: &KeyChain) -> [u8; 32] {
    trust::keys::derive_key(&keys.core_db_key(), b"backup")
}

/// `robotd backup` -> data/backups/bender-backup-<ts>.tar.sealed
pub fn run(cfg: &RobotConfig) -> anyhow::Result<PathBuf> {
    let data_dir = Path::new(&cfg.robot.data_dir);
    let keys = KeyChain::load_or_create(&data_dir.join("kek.key"))?;
    let ts = trust::ids::ts_ms();
    let staging = data_dir.join("backups").join(format!("staging-{ts}"));
    std::fs::create_dir_all(&staging)?;

    let staged = archive::stage(
        &StageSpec {
            data_dir,
            inner_prefix: "",
            include_keyfile: false,
        },
        &keys,
        &staging,
    )?;

    let manifest = serde_json::json!({
        "kind": "bender-backup",
        "version": env!("CARGO_PKG_VERSION"),
        "robot_id": staged.robot_id,
        "created_at": ts,
        "cells": staged.cells,
        "media_files": staged.media_files,
        "note": "restore requires the instance kek.key; contents are encrypted \
                 at rest and the tarball is sealed",
    });
    std::fs::write(
        staging.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;

    let sealed = archive::seal_dir(&staging, &backup_key(&keys))?;
    let sealed_path = data_dir
        .join("backups")
        .join(format!("bender-backup-{ts}.tar.sealed"));
    std::fs::write(&sealed_path, sealed)?;
    std::fs::remove_dir_all(&staging).ok();
    Ok(sealed_path)
}

/// `robotd backup-restore <sealed> <dest-dir>` -- unseal + expand. Needs the
/// same kek.key beside the configured data dir.
pub fn restore(cfg: &RobotConfig, sealed_path: &Path, dest: &Path) -> anyhow::Result<()> {
    let data_dir = Path::new(&cfg.robot.data_dir);
    let keys = KeyChain::load_or_create(&data_dir.join("kek.key"))?;
    let sealed = std::fs::read(sealed_path)
        .with_context(|| format!("reading {}", sealed_path.display()))?;
    archive::unseal_into(&sealed, &backup_key(&keys), dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MindSection, RobotConfig, RobotSection, ServerSection};

    #[test]
    fn backup_roundtrip_sealed_and_encrypted() {
        let dir = std::env::temp_dir().join(format!("bkp-{}", trust::ids::random_hex(6)));
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
            hub: Default::default(),
        };
        // boot a robot with one fact so there's something real to back up
        let boot = crate::boot::bootstrap(&cfg).unwrap();
        let owner = boot.robot.owner_principal;
        boot.state
            .robot
            .handle_message(owner, "remember that backups matter".into())
            .unwrap();

        let sealed = run(&cfg).unwrap();
        assert!(sealed.exists());

        // the sealed blob is opaque: no tar magic, no SQLite magic, and the
        // plaintext of the fact does not appear anywhere in it
        let bytes = std::fs::read(&sealed).unwrap();
        assert!(!bytes.starts_with(b"SQLite format 3"));
        let needle = b"backups matter";
        assert!(
            !bytes.windows(needle.len()).any(|w| w == needle),
            "plaintext leaked into the sealed backup"
        );
        assert!(
            !bytes.windows(5).any(|w| w == b"ustar"),
            "tar header leaked into the sealed backup"
        );

        // restore round-trips and the cell opens with the instance keys
        let restore_dir = dir.join("restored");
        restore(&cfg, &sealed, &restore_dir).unwrap();
        assert!(restore_dir.join("manifest.json").exists());
        let keys = trust::keys::KeyChain::load_or_create(&dir.join("kek.key")).unwrap();
        let core =
            trust::cells::open_encrypted(&restore_dir.join("core.db"), &keys.core_db_key())
                .unwrap();
        let dek = crate::robot::ensure_cell_key(&core, &keys, "owner").unwrap();
        let cell =
            trust::cells::open_encrypted(&restore_dir.join("cells").join("owner.db"), &dek)
                .unwrap();
        assert_eq!(mind::facts::count_active(&cell).unwrap(), 1);
        drop(cell);

        // a backup sealed by one robot must not open with another's keys
        let other_dir = dir.join("other");
        std::fs::create_dir_all(&other_dir).unwrap();
        let other_keys =
            trust::keys::KeyChain::load_or_create(&other_dir.join("kek.key")).unwrap();
        assert!(
            archive::unseal_into(
                &std::fs::read(&sealed).unwrap(),
                &super::backup_key(&other_keys),
                &dir.join("nope")
            )
            .is_err(),
            "a foreign key must not open this backup"
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
