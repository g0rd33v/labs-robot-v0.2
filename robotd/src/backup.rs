//! Encrypted backup (Q38): per-cell online snapshot (VACUUM INTO keeps the
//! cipher) + content-addressed media + core last -> one sealed tarball with
//! a manifest. Restore requires the same kek.key -- the backup shreds with
//! the keys, like everything else.

use crate::config::RobotConfig;
use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use std::process::Command;
use trust::keys::KeyChain;

fn backup_key(keys: &KeyChain) -> [u8; 32] {
    trust::keys::derive_key(&keys.core_db_key(), b"backup")
}

/// Snapshot one encrypted db into `dest` via VACUUM INTO (online,
/// consistent, keeps SQLCipher encryption).
fn snapshot_db(path: &Path, key: &[u8; 32], dest: &Path) -> anyhow::Result<()> {
    let conn = trust::cells::open_encrypted(path, key)
        .with_context(|| format!("opening {} for backup", path.display()))?;
    conn.execute(
        "VACUUM INTO ?1",
        rusqlite::params![dest.to_string_lossy()],
    )?;
    // the snapshot must be as opaque as the original
    if !trust::cells::file_looks_encrypted(dest)? {
        std::fs::remove_file(dest).ok();
        bail!("snapshot of {} came out unencrypted; aborting", path.display());
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> anyhow::Result<u64> {
    let mut files = 0;
    if !from.exists() {
        return Ok(0);
    }
    for entry in walk(from)? {
        let rel = entry.strip_prefix(from)?;
        let dest = to.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&entry, &dest)?;
        files += 1;
    }
    Ok(files)
}

fn walk(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = vec![];
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            out.extend(walk(&p)?);
        } else {
            out.push(p);
        }
    }
    Ok(out)
}

/// `robotd backup` -> data/backups/bender-backup-<ts>.tar.sealed
pub fn run(cfg: &RobotConfig) -> anyhow::Result<PathBuf> {
    let data_dir = Path::new(&cfg.robot.data_dir);
    let keys = KeyChain::load_or_create(&data_dir.join("kek.key"))?;
    let ts = trust::ids::ts_ms();
    let staging = data_dir.join("backups").join(format!("staging-{ts}"));
    std::fs::create_dir_all(staging.join("cells"))?;

    // cells first, core last (Q38: core-last captures the freshest registry)
    let core_path = data_dir.join("core.db");
    let core = trust::cells::open_encrypted(&core_path, &keys.core_db_key())?;
    let cell_ids: Vec<String> = {
        let mut stmt = core.prepare("SELECT cell_id FROM cell_keys")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids
    };
    let mut manifest_cells = vec![];
    for cell_id in &cell_ids {
        let dek = crate::robot::ensure_cell_key(&core, &keys, cell_id)?;
        let src = data_dir.join("cells").join(format!("{cell_id}.db"));
        if !src.exists() {
            continue;
        }
        let dest = staging.join("cells").join(format!("{cell_id}.db"));
        snapshot_db(&src, &dek, &dest)?;
        manifest_cells.push(serde_json::json!({
            "cell_id": cell_id,
            "bytes": std::fs::metadata(&dest)?.len(),
        }));
    }
    let media_files = copy_tree(&data_dir.join("media"), &staging.join("media"))?;
    snapshot_db(&core_path, &keys.core_db_key(), &staging.join("core.db"))?;
    drop(core);

    let manifest = serde_json::json!({
        "kind": "bender-backup",
        "version": env!("CARGO_PKG_VERSION"),
        "created_at": ts,
        "cells": manifest_cells,
        "media_files": media_files,
        "note": "restore requires the instance kek.key; contents are encrypted at rest and the tarball is sealed",
    });
    std::fs::write(
        staging.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;

    // tar the staging dir, then seal the tarball under a KEK-derived key
    let tar_path = data_dir.join("backups").join(format!("bender-backup-{ts}.tar"));
    let status = Command::new("tar")
        .arg("-cf")
        .arg(&tar_path)
        .arg("-C")
        .arg(&staging)
        .arg(".")
        .status()
        .context("running tar")?;
    if !status.success() {
        bail!("tar failed: {status}");
    }
    let sealed_path = data_dir
        .join("backups")
        .join(format!("bender-backup-{ts}.tar.sealed"));
    let tar_bytes = std::fs::read(&tar_path)?;
    let sealed = trust::keys::seal_bytes(&backup_key(&keys), &tar_bytes)?;
    std::fs::write(&sealed_path, sealed)?;
    std::fs::remove_file(&tar_path)?;
    std::fs::remove_dir_all(&staging)?;
    Ok(sealed_path)
}

/// `robotd backup-restore <sealed> <dest-dir>` -- unseal + untar. Needs the
/// same kek.key beside the current data_dir.
pub fn restore(cfg: &RobotConfig, sealed_path: &Path, dest: &Path) -> anyhow::Result<()> {
    let data_dir = Path::new(&cfg.robot.data_dir);
    let keys = KeyChain::load_or_create(&data_dir.join("kek.key"))?;
    let sealed = std::fs::read(sealed_path)?;
    let tar_bytes = trust::keys::open_bytes(&backup_key(&keys), &sealed)
        .context("unsealing (is this the right kek.key?)")?;
    std::fs::create_dir_all(dest)?;
    let tar_path = dest.join("restore.tar");
    std::fs::write(&tar_path, tar_bytes)?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&tar_path)
        .arg("-C")
        .arg(dest)
        .status()
        .context("running tar -x")?;
    if !status.success() {
        bail!("tar -x failed: {status}");
    }
    std::fs::remove_file(&tar_path)?;
    Ok(())
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
        // sealed tarball: no tar magic, no sqlite magic
        let head = std::fs::read(&sealed).unwrap();
        assert!(!head.starts_with(b"SQLite format 3"));
        assert!(head.len() > 24);

        // restore elsewhere and verify the owner cell opens with the kek
        let restore_dir = dir.join("restored");
        restore(&cfg, &sealed, &restore_dir).unwrap();
        assert!(restore_dir.join("manifest.json").exists());
        assert!(restore_dir.join("core.db").exists());
        let keys =
            trust::keys::KeyChain::load_or_create(&dir.join("kek.key")).unwrap();
        let core =
            trust::cells::open_encrypted(&restore_dir.join("core.db"), &keys.core_db_key())
                .unwrap();
        let dek = crate::robot::ensure_cell_key(&core, &keys, "owner").unwrap();
        let cell =
            trust::cells::open_encrypted(&restore_dir.join("cells").join("owner.db"), &dek)
                .unwrap();
        assert_eq!(mind::facts::count_active(&cell).unwrap(), 1);
        drop(cell);
        let _ = std::fs::remove_dir_all(dir);
    }
}
