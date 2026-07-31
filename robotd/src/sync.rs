//! Two-way sync between instances of the same robot.
//!
//! The Robot Package (§8) carries a robot somewhere. This keeps two copies
//! of one robot in agreement afterwards: the machine and the stick, the
//! machine and a second machine.
//!
//! **What travels is knowledge — never history.** Messages, facts,
//! reminders, media. Not the journal, not receipts, and above all not the
//! Boundary Log: it is a hash chain, and two chains have no merge that is
//! still a chain. Each instance keeps its own and each stays independently
//! verifiable, which is a stronger claim than a stitched-together history
//! that neither machine could support.
//!
//! **No new plaintext exists.** The peer's cells are already SQLCipher
//! files under the same KEK — restore carries it — so this opens them and
//! merges in place. A delta file would have meant inventing a format and
//! leaving a decrypted-in-transit artifact on the very stick most likely to
//! be lost.
//!
//! Both directions happen in one pass, which is what lets tombstones be
//! collected: when this returns, both sides have applied each other's
//! deletions, so the record of them has done its job and goes.

use crate::robot::RobotCore;
use anyhow::{bail, Context};
use mind::merge::{self, MergeReport};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use trust::boundary::{Crossing, Direction};
use trust::keys::KeyChain;

/// Clock skew insurance. Watermarks are the peer's own timestamps, so a
/// peer whose clock steps backwards could hide rows behind the mark; a day
/// of overlap costs a few redundant idempotent inserts and removes the
/// class of problem.
const WATERMARK_MARGIN_MS: i64 = 86_400_000;

fn cap_err(e: mind::MindError) -> prism::PrismError {
    prism::PrismError::Capability(e.to_string())
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub peer_instance: String,
    pub pulled: MergeReport,
    pub pushed: MergeReport,
    pub media_files: usize,
    pub cells: usize,
    pub skipped_cells: Vec<String>,
}

impl SyncReport {
    pub fn quiet(&self) -> bool {
        self.pulled.is_empty() && self.pushed.is_empty() && self.media_files == 0
    }

    /// One line for the journal and the owner. Counts only -- a sync report
    /// naming contents would be a second copy of the data in the log.
    pub fn summary(&self) -> String {
        format!(
            "synced with {}: pulled {} rows ({} deletions), pushed {} rows ({} deletions), \
             {} media files, {} cells",
            self.peer_instance,
            self.pulled.total(),
            self.pulled.deleted,
            self.pushed.total(),
            self.pushed.deleted,
            self.media_files,
            self.cells
        )
    }
}

pub fn init_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "
CREATE TABLE IF NOT EXISTS sync_peers (
    instance_id   TEXT PRIMARY KEY,
    path          TEXT NOT NULL,
    pulled_through INTEGER NOT NULL DEFAULT 0,
    acked_through  INTEGER NOT NULL DEFAULT 0,
    last_sync_at   INTEGER NOT NULL DEFAULT 0
);
",
    )?;
    Ok(())
}

/// The other side, opened.
struct Peer {
    root: PathBuf,
    core: Connection,
    keys: KeyChain,
    instance_id: String,
}

fn data_dir_of(root: &Path) -> PathBuf {
    // a restored robot keeps its data under <root>/data
    let nested = root.join("data");
    if nested.join("core.db").exists() {
        nested
    } else {
        root.to_path_buf()
    }
}

fn open_peer(root: &Path, our_robot_id: &str) -> anyhow::Result<Peer> {
    let data = data_dir_of(root);
    let kek = data.join("kek.key");
    let core_path = data.join("core.db");
    if !kek.exists() || !core_path.exists() {
        bail!("no robot at {}", root.display());
    }
    let keys = KeyChain::load_or_create(&kek).context("peer kek")?;
    let core = trust::cells::open_encrypted(&core_path, &keys.core_db_key())
        .context("peer core.db -- different KEK?")?;

    let their_robot = trust::schema::meta_get(&core, "robot_id")?.unwrap_or_default();
    if their_robot != our_robot_id {
        // Merging two different robots' memories is never what anyone
        // meant, and it is not recoverable afterwards.
        bail!(
            "that is a different robot ({their_robot}, not {our_robot_id}) -- refusing to merge"
        );
    }
    // A copy restored before instance ids existed has none. Mint one and
    // write it to their side: a throwaway id would be different on every
    // sync, so the watermark would never advance and every sweep would
    // re-scan everything from the beginning.
    let instance_id = match trust::schema::meta_get(&core, "instance_id")? {
        Some(id) => id,
        None => {
            let id = format!("inst_{}", trust::ids::random_hex(8));
            trust::schema::meta_set(&core, "instance_id", &id)?;
            tracing::info!("peer had no instance id; minted {id} for it");
            id
        }
    };
    Ok(Peer {
        root: data,
        core,
        keys,
        instance_id,
    })
}

/// cell_id -> dek, for every cell an instance can actually open.
fn cells_of(core: &Connection, keys: &KeyChain) -> anyhow::Result<Vec<(String, [u8; 32])>> {
    let mut stmt = core.prepare("SELECT cell_id, wrapped_dek, nonce FROM cell_keys")?;
    let rows: Vec<(String, Vec<u8>, Vec<u8>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![];
    for (id, wrapped, nonce) in rows {
        match keys.unwrap_dek(&nonce, &wrapped) {
            Ok(dek) => out.push((id, dek)),
            // a cell we hold no key for is not ours to read; skip it rather
            // than fail the whole sync
            Err(e) => tracing::warn!("cell {id}: no usable key ({e})"),
        }
    }
    Ok(out)
}

fn watermark(core: &Connection, peer: &str) -> anyhow::Result<(i64, i64)> {
    Ok(core
        .query_row(
            "SELECT pulled_through, acked_through FROM sync_peers WHERE instance_id = ?1",
            params![peer],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, 0)))
}

fn high_water(d: &merge::CellDelta) -> i64 {
    let mx = |rows: &Vec<merge::Row>, key: &str| {
        rows.iter()
            .filter_map(|r| r.get(key).and_then(|v| v.as_i64()))
            .max()
            .unwrap_or(0)
    };
    [
        mx(&d.messages, "ts"),
        mx(&d.facts, "created_at"),
        mx(&d.reminders, "created_at"),
        mx(&d.media, "created_at"),
        mx(&d.tombstones, "deleted_at"),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

/// Copy content-addressed blobs the other side is missing. They are sealed
/// under a key derived from the cell's DEK, which both instances share, so
/// the bytes need no re-encryption -- only carrying.
fn copy_media(from: &Path, to: &Path) -> anyhow::Result<usize> {
    if !from.exists() {
        return Ok(0);
    }
    let mut copied = 0;
    for src in crate::archive::walk(from)? {
        let rel = src.strip_prefix(from)?;
        let dst = to.join(rel);
        if dst.exists() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // same-directory temp then rename: a half-copied blob would fail
        // its own content hash, and a torn file on a stick is exactly the
        // failure this must not produce
        let tmp = dst.with_extension("part");
        std::fs::copy(&src, &tmp)?;
        std::fs::rename(&tmp, &dst)?;
        copied += 1;
    }
    Ok(copied)
}

/// Sync this robot with the instance at `path`. Both directions, one pass.
pub fn sync_with(robot: &RobotCore, path: &Path) -> anyhow::Result<SyncReport> {
    let our_robot_id = {
        let core = robot.core.lock().map_err(|_| anyhow::anyhow!("core lock"))?;
        trust::schema::meta_get(&core, "robot_id")?.unwrap_or_default()
    };
    let peer = open_peer(path, &our_robot_id)?;
    if peer.instance_id == robot.instance_id {
        bail!("that is this same instance");
    }

    let mut rep = SyncReport {
        peer_instance: peer.instance_id.clone(),
        ..Default::default()
    };

    let (pulled_through, _) = {
        let core = robot.core.lock().map_err(|_| anyhow::anyhow!("core lock"))?;
        init_schema(&core)?;
        watermark(&core, &peer.instance_id)?
    };
    let since = (pulled_through - WATERMARK_MARGIN_MS).max(0);

    let ours: Vec<(String, [u8; 32])> = {
        let core = robot.core.lock().map_err(|_| anyhow::anyhow!("core lock"))?;
        cells_of(&core, &robot.keychain())?
    };
    let theirs = cells_of(&peer.core, &peer.keys)?;

    let mut new_watermark = pulled_through;
    let mut pushed_tombstones = 0i64;

    for (cell_id, dek) in &ours {
        let Some((_, their_dek)) = theirs.iter().find(|(id, _)| id == cell_id) else {
            // present here, absent there: a member added on one side only.
            // Creating their cell would mean minting keys on their behalf;
            // say so instead of guessing.
            rep.skipped_cells.push(cell_id.clone());
            continue;
        };
        if dek != their_dek {
            rep.skipped_cells.push(cell_id.clone());
            continue;
        }
        // short bursts on our own cell, so a live turn is not blocked for
        // the length of a sync
        let mine = robot.open_cell_db(cell_id, dek)?;
        let peer_db = trust::cells::open_encrypted(
            &peer.root.join("cells").join(format!("{cell_id}.db")),
            their_dek,
        )?;
        prism::init_cell_schema(&peer_db)?;
        mind::init_cell_schema(&peer_db)?;

        // pull: theirs -> ours
        let incoming = merge::export(&peer_db, since)?;
        new_watermark = new_watermark.max(high_water(&incoming));
        let pulled = mine.with(|c| merge::apply(c, &incoming).map_err(cap_err))?;

        // push: ours -> theirs. Their watermark for us lives on their side;
        // exporting everything is correct and idempotent, and the volumes
        // here are a personal robot's, not a warehouse's.
        let outgoing = mine.with(|c| merge::export(c, 0).map_err(cap_err))?;
        pushed_tombstones = pushed_tombstones
            .max(mine.with(|c| merge::tombstone_high_water(c).map_err(cap_err))?);
        let pushed = merge::apply(&peer_db, &outgoing)?;

        rep.media_files += copy_media(
            &robot.media_dir(cell_id),
            &peer.root.join("media").join(cell_id),
        )?;
        rep.media_files += copy_media(
            &peer.root.join("media").join(cell_id),
            &robot.media_dir(cell_id),
        )?;

        // Both sides have now applied the other's tombstones, so the record
        // of a deletion has done its work and can go. This is the whole
        // reason the pass is two-way: a one-way push could never know.
        let ack = high_water(&incoming).min(pushed_tombstones);
        mine.with(|c| merge::collect_tombstones(c, ack).map_err(cap_err))?;
        merge::collect_tombstones(&peer_db, pushed_tombstones)?;

        rep.pulled.messages += pulled.messages;
        rep.pulled.facts += pulled.facts;
        rep.pulled.reminders += pulled.reminders;
        rep.pulled.media += pulled.media;
        rep.pulled.deleted += pulled.deleted;
        rep.pulled.refused_resurrections += pulled.refused_resurrections;
        rep.pushed.messages += pushed.messages;
        rep.pushed.facts += pushed.facts;
        rep.pushed.reminders += pushed.reminders;
        rep.pushed.media += pushed.media;
        rep.pushed.deleted += pushed.deleted;
        rep.pushed.refused_resurrections += pushed.refused_resurrections;
        rep.cells += 1;
    }

    {
        let core = robot.core.lock().map_err(|_| anyhow::anyhow!("core lock"))?;
        core.execute(
            "INSERT INTO sync_peers(instance_id, path, pulled_through, acked_through, last_sync_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(instance_id) DO UPDATE SET path = excluded.path, \
               pulled_through = excluded.pulled_through, \
               acked_through = excluded.acked_through, \
               last_sync_at = excluded.last_sync_at",
            params![
                peer.instance_id,
                path.display().to_string(),
                new_watermark,
                pushed_tombstones,
                trust::ids::ts_ms()
            ],
        )?;

        // law 3: bytes left this process and bytes came in. Counts only --
        // the payload is the person's memory and belongs in the cell, not
        // in a log line.
        for (dir, n) in [
            (Direction::Out, rep.pushed.total()),
            (Direction::In, rep.pulled.total()),
        ] {
            trust::boundary::append(
                &core,
                &Crossing {
                    direction: dir,
                    channel: "sync".into(),
                    counterparty: peer.instance_id.clone(),
                    purpose: "two-way sync with another instance of this robot".into(),
                    categories: "messages,facts,reminders,media".into(),
                    payload_hash: String::new(),
                    size: n as i64,
                    trust_tag: "same-robot".into(),
                },
            )?;
        }
    }
    Ok(rep)
}
