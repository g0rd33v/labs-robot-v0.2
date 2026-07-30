//! Shared archive machinery for `backup` and `package`.
//!
//! Both are the same algorithm with four parameters: snapshot every cell
//! online (cipher preserved), copy the sealed media tree, snapshot core
//! LAST (Q38: core-last captures the freshest registry), write a manifest,
//! tar it, seal the tar. They were written twice; this is the single copy.

use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use std::process::Command;
use trust::keys::KeyChain;

/// Snapshot one encrypted db into `dest` via VACUUM INTO (online,
/// consistent, keeps SQLCipher encryption).
pub fn snapshot_db(path: &Path, key: &[u8; 32], dest: &Path) -> anyhow::Result<()> {
    let conn = trust::cells::open_encrypted(path, key)
        .with_context(|| format!("opening {} for snapshot", path.display()))?;
    conn.execute("VACUUM INTO ?1", rusqlite::params![dest.to_string_lossy()])?;
    // a snapshot that came out readable would be a silent downgrade of
    // encryption-at-rest; refuse rather than ship it
    if !trust::cells::file_looks_encrypted(dest)? {
        std::fs::remove_file(dest).ok();
        bail!("snapshot of {} came out unencrypted; aborting", path.display());
    }
    Ok(())
}

pub fn walk(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = vec![];
    if !dir.exists() {
        return Ok(out);
    }
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

pub fn copy_tree(from: &Path, to: &Path) -> anyhow::Result<u64> {
    let mut files = 0;
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

pub struct StageSpec<'a> {
    pub data_dir: &'a Path,
    /// Sub-path inside the archive: "" for backups, "data" for packages
    /// (so a package unpacks straight into a runnable robot directory).
    pub inner_prefix: &'a str,
    /// Packages carry the keys -- the package IS the robot, and its
    /// one-time code is the perimeter. Backups do not.
    pub include_keyfile: bool,
}

pub struct Staged {
    pub cells: Vec<serde_json::Value>,
    pub media_files: u64,
    pub robot_id: String,
}

/// Lay out an archive in a temporary directory. Caller writes the manifest
/// and seals it.
pub fn stage(spec: &StageSpec, keys: &KeyChain, staging: &Path) -> anyhow::Result<Staged> {
    let inner = if spec.inner_prefix.is_empty() {
        staging.to_path_buf()
    } else {
        staging.join(spec.inner_prefix)
    };
    std::fs::create_dir_all(inner.join("cells"))?;

    let core_path = spec.data_dir.join("core.db");
    let core = trust::cells::open_encrypted(&core_path, &keys.core_db_key())?;
    let cell_ids: Vec<String> = {
        let mut stmt = core.prepare("SELECT cell_id FROM cell_keys")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids
    };
    let robot_id = trust::schema::meta_get(&core, "robot_id")?.unwrap_or_default();

    let mut cells = vec![];
    for cell_id in &cell_ids {
        let dek = crate::robot::ensure_cell_key(&core, keys, cell_id)?;
        let src = spec.data_dir.join("cells").join(format!("{cell_id}.db"));
        if !src.exists() {
            continue;
        }
        let dest = inner.join("cells").join(format!("{cell_id}.db"));
        snapshot_db(&src, &dek, &dest)?;
        cells.push(serde_json::json!({
            "cell_id": cell_id,
            "bytes": std::fs::metadata(&dest)?.len(),
        }));
    }
    let media_files = copy_tree(&spec.data_dir.join("media"), &inner.join("media"))?;
    // core last: it holds the registry of everything above
    snapshot_db(&core_path, &keys.core_db_key(), &inner.join("core.db"))?;
    drop(core);

    if spec.include_keyfile {
        std::fs::copy(spec.data_dir.join("kek.key"), inner.join("kek.key"))?;
    }

    Ok(Staged {
        cells,
        media_files,
        robot_id,
    })
}

/// tar a staged directory and seal the tarball under `key`.
pub fn seal_dir(staging: &Path, key: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    let tar_path = staging.with_extension("tar");
    let status = Command::new("tar")
        .arg("-cf")
        .arg(&tar_path)
        .arg("-C")
        .arg(staging)
        .arg(".")
        .status()
        .context("running tar")?;
    if !status.success() {
        bail!("tar failed: {status}");
    }
    let bytes = std::fs::read(&tar_path)?;
    let sealed = trust::keys::seal_bytes(key, &bytes)?;
    std::fs::remove_file(&tar_path).ok();
    Ok(sealed)
}

/// Unseal an archive and expand it into `dest`.
pub fn unseal_into(sealed: &[u8], key: &[u8; 32], dest: &Path) -> anyhow::Result<()> {
    let bytes = trust::keys::open_bytes(key, sealed)
        .context("unsealing failed (wrong code or wrong keys?)")?;
    std::fs::create_dir_all(dest)?;
    let tar_path = dest.join(".incoming.tar");
    std::fs::write(&tar_path, bytes)?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&tar_path)
        .arg("-C")
        .arg(dest)
        .status()
        .context("running tar -x")?;
    std::fs::remove_file(&tar_path).ok();
    if !status.success() {
        bail!("tar -x failed: {status}");
    }
    Ok(())
}
