//! Merging another instance's knowledge into this one.
//!
//! Two instances of the same robot -- the machine and the stick -- hold the
//! same kind of state and diverge the moment either is used alone. What can
//! be merged is KNOWLEDGE: messages, facts, reminders, media. What cannot is
//! the history of doing -- journal, receipts, and above all the Boundary
//! Log, which is a hash chain and has no merge that is still a hash chain.
//! Each instance keeps its own, and each stays independently verifiable.
//!
//! Three rules decide everything here, and all three are chosen so that the
//! worse failure is the one that cannot happen:
//!
//! 1. **A deletion beats everything.** Tombstones are applied last, and a
//!    tombstoned id is never re-inserted. The alternative -- a fact the
//!    person deleted returning from the other machine, silently -- would
//!    make "deleted for real" a lie.
//! 2. **Conflicting edits both survive.** The registry already models
//!    correction as supersession rather than overwrite, so two divergent
//!    corrections are two chains over one ancestor. Keeping both loses
//!    nothing and stays inspectable; last-writer-wins discards an edit and
//!    trusts two clocks to agree.
//! 3. **A terminal state beats an active one.** For reminders,
//!    `cancelled`/`fired` win over `active` regardless of timestamps.
//!    Resurrecting a cancelled reminder nags about something called off;
//!    the opposite merely does nothing.

use crate::MindError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Everything one instance has learned since a watermark.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CellDelta {
    #[serde(default)]
    pub messages: Vec<Row>,
    #[serde(default)]
    pub facts: Vec<Row>,
    #[serde(default)]
    pub reminders: Vec<Row>,
    #[serde(default)]
    pub media: Vec<Row>,
    /// Names over that media. Travels separately because the bytes are
    /// content-addressed and the name is not.
    #[serde(default)]
    pub files: Vec<Row>,
    #[serde(default)]
    pub instructions: Vec<Row>,
    #[serde(default)]
    pub commitments: Vec<Row>,
    #[serde(default)]
    pub tombstones: Vec<Row>,
    /// Soul's relationship state. Knowledge, so it travels -- the robot on
    /// the stick should speak to you the way the one on the machine does.
    #[serde(default)]
    pub soul_persona: Vec<Row>,
    #[serde(default)]
    pub soul_revisions: Vec<Row>,
    /// `cell_meta` keys worth carrying. Preferences, not knowledge.
    #[serde(default)]
    pub meta: Vec<(String, String, i64)>,
}

pub type Row = serde_json::Map<String, serde_json::Value>;

/// What one merge changed. Goes into the receipt, so a sync can be audited
/// like any other effect rather than trusted.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MergeReport {
    pub messages: usize,
    pub facts: usize,
    pub reminders: usize,
    pub media: usize,
    pub files: usize,
    pub instructions: usize,
    pub commitments: usize,
    pub deleted: usize,
    /// Rows that arrived for something already deleted here, and were
    /// therefore refused. A non-zero count is the erase right working.
    pub refused_resurrections: usize,
}

impl MergeReport {
    pub fn total(&self) -> usize {
        self.messages + self.facts + self.reminders + self.media + self.files
            + self.instructions + self.commitments + self.deleted
    }

    /// What moved that the person would care about.
    ///
    /// Messages are excluded on purpose. A sync notice is itself a chat
    /// message, so it syncs, so the next sweep has a row to move, so it
    /// announces itself again -- a trickle that sustains itself forever and
    /// makes the log look busy while nothing is happening. Announce
    /// KNOWLEDGE moving; let the transcript catch up quietly.
    pub fn knowledge(&self) -> usize {
        self.facts + self.reminders + self.media + self.files + self.instructions
            + self.commitments + self.deleted
    }
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

fn rows(conn: &Connection, sql: &str, since: i64) -> Result<Vec<Row>, MindError> {
    let mut stmt = conn.prepare(sql)?;
    let cols: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let out = stmt
        .query_map(params![since], |r| {
            let mut m = Row::new();
            for (i, name) in cols.iter().enumerate() {
                let v = match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                    rusqlite::types::ValueRef::Integer(x) => serde_json::json!(x),
                    rusqlite::types::ValueRef::Real(x) => serde_json::json!(x),
                    rusqlite::types::ValueRef::Text(x) => {
                        serde_json::json!(String::from_utf8_lossy(x))
                    }
                    rusqlite::types::ValueRef::Blob(_) => serde_json::Value::Null,
                };
                m.insert(name.clone(), v);
            }
            Ok(m)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(out)
}

/// Everything in this cell newer than `since`.
pub fn export(conn: &Connection, since: i64) -> Result<CellDelta, MindError> {
    let meta: Vec<(String, String, i64)> = conn
        .prepare("SELECT key, value FROM cell_meta WHERE key IN ('lang', 'soul:evolution')")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, 0i64)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CellDelta {
        messages: rows(
            conn,
            "SELECT id, ts, direction, surface, lang, content, media_ref \
             FROM messages WHERE ts > ?1",
            since,
        )?,
        facts: rows(
            conn,
            // `class` travels with the fact. Without it a restricted fact
            // arrives on the other instance as ordinary and is cleared to
            // reach a model there -- the classification would protect
            // exactly one machine, which is worse than none because it
            // reads as protection.
            //
            // `local_only` does not travel at all: that is what the class
            // means, and enforcing it at export is the only place that can.
            "SELECT id, entity, content, source_msg_id, intent_id, status, \
             confidence, created_at, superseded_by, class FROM facts \
             WHERE created_at > ?1 AND class NOT IN ('local_only', 'credential')",
            since,
        )?,
        reminders: rows(
            conn,
            "SELECT id, intent_id, created_at, fire_at, about, status \
             FROM reminders WHERE created_at > ?1",
            since,
        )?,
        media: rows(
            conn,
            "SELECT hash, mime, size, created_at, source FROM media WHERE created_at > ?1",
            since,
        )?,
        files: rows(
            conn,
            // classes gate files exactly as they gate facts: what must not
            // leave the machine does not leave it in a document either.
            "SELECT id, name, hash, size, class, source_msg_id, created_at, updated_at \
             FROM files WHERE updated_at > ?1 \
               AND class NOT IN ('local_only', 'credential')",
            since,
        )
        .unwrap_or_default(),
        instructions: rows(
            conn,
            "SELECT id, body, source_msg_id, status, superseded_by, class, created_at \
             FROM instructions WHERE created_at > ?1 \
               AND class NOT IN ('local_only', 'credential')",
            since,
        )
        .unwrap_or_default(),
        commitments: rows(
            conn,
            "SELECT id, what, kind, status, source_msg_id, intent_id, due_at, \
             created_at, closed_at, closed_why FROM commitments \
             WHERE created_at > ?1 OR closed_at > ?1",
            since,
        )
        .unwrap_or_default(),
        tombstones: rows(
            conn,
            "SELECT id, kind, deleted_at, origin FROM tombstones WHERE deleted_at > ?1",
            since,
        )?,
        soul_persona: rows(
            conn,
            "SELECT dimension, value, floor, ceiling, updated_at \
             FROM soul_persona WHERE updated_at > ?1",
            since,
        )
        .unwrap_or_default(),
        soul_revisions: rows(
            conn,
            "SELECT id, created_at, reason, diff_json, rolls_back_to, applied, \
             evaluator_verdict FROM soul_revisions WHERE created_at > ?1",
            since,
        )
        .unwrap_or_default(),
        meta,
    })
}

fn s(r: &Row, k: &str) -> Option<String> {
    r.get(k).and_then(|v| v.as_str()).map(String::from)
}

fn i(r: &Row, k: &str) -> i64 {
    r.get(k).and_then(|v| v.as_i64()).unwrap_or(0)
}

fn tombstoned(conn: &Connection, id: &str) -> Result<bool, MindError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM tombstones WHERE id = ?1",
            params![id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Merge a peer's delta into this cell. Idempotent: applying the same delta
/// twice changes nothing the second time, which is what makes an
/// interrupted sync safe to simply run again.
pub fn apply(conn: &Connection, d: &CellDelta) -> Result<MergeReport, MindError> {
    let tx = conn.unchecked_transaction()?;
    let mut rep = MergeReport::default();

    // Tombstones FIRST, so nothing in this delta can resurrect something
    // already deleted, and the ids to refuse are known before the inserts.
    for t in &d.tombstones {
        let Some(id) = s(t, "id") else { continue };
        let existed = tx.execute(
            "INSERT OR IGNORE INTO tombstones(id, kind, deleted_at, origin) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                id,
                s(t, "kind").unwrap_or_else(|| "fact".into()),
                i(t, "deleted_at"),
                s(t, "origin").unwrap_or_default()
            ],
        )?;
        // apply it locally: clear inbound links, then the row
        tx.execute(
            "UPDATE facts SET superseded_by = NULL WHERE superseded_by = ?1",
            params![id],
        )?;
        let mut gone = tx.execute("DELETE FROM facts WHERE id = ?1", params![id])?;
        gone += tx.execute("DELETE FROM files WHERE id = ?1", params![id])?;
        if existed == 1 || gone > 0 {
            rep.deleted += gone;
        }
    }

    for m in &d.messages {
        let Some(id) = s(m, "id") else { continue };
        rep.messages += tx.execute(
            "INSERT OR IGNORE INTO messages(id, ts, direction, surface, lang, content, media_ref) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                i(m, "ts"),
                s(m, "direction").unwrap_or_else(|| "in".into()),
                s(m, "surface").unwrap_or_else(|| "chat".into()),
                s(m, "lang"),
                s(m, "content").unwrap_or_default(),
                s(m, "media_ref")
            ],
        )?;
    }

    for f in &d.facts {
        let Some(id) = s(f, "id") else { continue };
        if tombstoned(&tx, &id)? {
            rep.refused_resurrections += 1;
            continue;
        }
        // law 5 travels with the fact: without its source message this is
        // not a fact we are willing to hold
        let Some(src) = s(f, "source_msg_id") else { continue };
        let have_src: bool = tx
            .query_row("SELECT 1 FROM messages WHERE id = ?1", params![src], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        if !have_src {
            continue;
        }
        // superseded_by may point at a fact later in this same delta, so it
        // is set in a second pass once every row exists
        rep.facts += tx.execute(
            "INSERT OR IGNORE INTO facts(id, entity, content, source_msg_id, intent_id, \
             status, confidence, created_at, class) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                s(f, "entity"),
                s(f, "content").unwrap_or_default(),
                src,
                s(f, "intent_id"),
                s(f, "status").unwrap_or_else(|| "stable".into()),
                f.get("confidence").and_then(|v| v.as_f64()).unwrap_or(1.0),
                i(f, "created_at"),
                // an unreadable or absent class lands on the protective
                // default, never on "no restriction"
                s(f, "class").unwrap_or_else(|| "owner_private".into())
            ],
        )?;
    }
    for f in &d.facts {
        let (Some(id), Some(sup)) = (s(f, "id"), s(f, "superseded_by")) else {
            continue;
        };
        if tombstoned(&tx, &sup)? || tombstoned(&tx, &id)? {
            continue;
        }
        let _ = tx.execute(
            "UPDATE facts SET superseded_by = ?2, status = 'superseded' \
             WHERE id = ?1 AND superseded_by IS NULL \
               AND EXISTS (SELECT 1 FROM facts WHERE id = ?2)",
            params![id, sup],
        );
    }

    for r in &d.reminders {
        let Some(id) = s(r, "id") else { continue };
        let theirs = s(r, "status").unwrap_or_else(|| "active".into());
        rep.reminders += tx.execute(
            "INSERT OR IGNORE INTO reminders(id, intent_id, created_at, fire_at, about, status) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                s(r, "intent_id").unwrap_or_else(|| format!("sync:{id}")),
                i(r, "created_at"),
                i(r, "fire_at"),
                s(r, "about").unwrap_or_default(),
                theirs
            ],
        )?;
        // a terminal state wins wherever it came from
        if theirs != "active" {
            tx.execute(
                "UPDATE reminders SET status = ?2 WHERE id = ?1 AND status = 'active'",
                params![id, theirs],
            )?;
        }
    }

    for m in &d.media {
        let Some(hash) = s(m, "hash") else { continue };
        rep.media += tx.execute(
            "INSERT OR IGNORE INTO media(hash, mime, size, created_at, source) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                hash,
                s(m, "mime").unwrap_or_default(),
                i(m, "size"),
                i(m, "created_at"),
                s(m, "source").unwrap_or_default()
            ],
        )?;
    }

    // Files. Rule 2 applies here as it does to facts: when both instances
    // edited one document, the newer edit keeps the name and the older is
    // preserved beside it rather than discarded. The conflict copy's id and
    // name are DERIVED from the losing content hash, so both instances
    // compute the same copy and a second sync finds nothing new -- a random
    // id here would breed a fresh copy on every sweep, forever.
    for f in &d.files {
        let (Some(id), Some(name), Some(hash)) = (s(f, "id"), s(f, "name"), s(f, "hash")) else {
            continue;
        };
        if tombstoned(&tx, &id)? {
            rep.refused_resurrections += 1;
            continue;
        }
        // law 5 travels with the file, exactly as with a fact
        let Some(src) = s(f, "source_msg_id") else { continue };
        let have_src: bool = tx
            .query_row("SELECT 1 FROM messages WHERE id = ?1", params![src], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        if !have_src {
            continue;
        }
        let class = s(f, "class").unwrap_or_else(|| "owner_private".into());
        let theirs_at = i(f, "updated_at");

        let mine: Option<(String, String, i64)> = tx
            .query_row(
                "SELECT id, hash, updated_at FROM files WHERE id = ?1 OR name = ?2",
                params![id, name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        match mine {
            // nothing here by that id or name: take theirs
            None => {
                rep.files += tx.execute(
                    "INSERT OR IGNORE INTO files(id, name, hash, size, class, \
                     source_msg_id, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        id,
                        name,
                        hash,
                        i(f, "size"),
                        class,
                        src,
                        i(f, "created_at"),
                        theirs_at
                    ],
                )?;
            }
            // same content: already agreed
            Some((_, mine_hash, _)) if mine_hash == hash => {}
            // divergent content. The newer edit takes the name; the other
            // is kept beside it under a derived name.
            Some((mine_id, mine_hash, mine_at)) => {
                let (keep_hash, keep_size, keep_at, copy_hash) = if theirs_at > mine_at {
                    (hash.clone(), i(f, "size"), theirs_at, mine_hash)
                } else {
                    (mine_hash.clone(), 0i64, mine_at, hash.clone())
                };
                if theirs_at > mine_at {
                    tx.execute(
                        "UPDATE files SET hash = ?2, size = ?3, updated_at = ?4 WHERE id = ?1",
                        params![mine_id, keep_hash, keep_size, keep_at],
                    )?;
                    rep.files += 1;
                }
                let copy_id = format!("fil_c{}", &copy_hash[..copy_hash.len().min(16)]);
                let copy_name = format!("{name} (conflicted copy)");
                let size: i64 = tx
                    .query_row(
                        "SELECT size FROM media WHERE hash = ?1",
                        params![copy_hash],
                        |r| r.get(0),
                    )
                    .optional()?
                    .unwrap_or(0);
                rep.files += tx.execute(
                    "INSERT OR IGNORE INTO files(id, name, hash, size, class, \
                     source_msg_id, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?7)",
                    params![copy_id, copy_name, copy_hash, size, class, src, keep_at],
                )?;
            }
        }
    }

    // Instructions: like facts -- insert, then supersede in a second pass,
    // never overwrite. One asymmetric rule on top: **retired beats
    // active**, with no timestamp arbitration, because the two mistakes
    // are not symmetric. Following a rule the person dropped on the other
    // machine does the thing they said to stop doing; the converse merely
    // asks them to re-add a rule.
    for i in &d.instructions {
        let Some(id) = s(i, "id") else { continue };
        if tombstoned(&tx, &id)? {
            rep.refused_resurrections += 1;
            continue;
        }
        let Some(src) = s(i, "source_msg_id") else { continue };
        let have_src: bool = tx
            .query_row("SELECT 1 FROM messages WHERE id = ?1", params![src], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        if !have_src {
            continue;
        }
        let theirs = s(i, "status").unwrap_or_else(|| "active".into());
        rep.instructions += tx.execute(
            "INSERT OR IGNORE INTO instructions(id, body, source_msg_id, status, \
             class, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                id,
                s(i, "body").unwrap_or_default(),
                src,
                theirs,
                s(i, "class").unwrap_or_else(|| "owner_private".into()),
                i.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0)
            ],
        )?;
        if theirs == "retired" {
            tx.execute(
                "UPDATE instructions SET status = 'retired' \
                 WHERE id = ?1 AND status = 'active'",
                params![id],
            )?;
        }
    }
    for i in &d.instructions {
        let (Some(id), Some(sup)) = (s(i, "id"), s(i, "superseded_by")) else {
            continue;
        };
        let _ = tx.execute(
            "UPDATE instructions SET superseded_by = ?2, status = 'superseded' \
             WHERE id = ?1 AND superseded_by IS NULL \
               AND EXISTS (SELECT 1 FROM instructions WHERE id = ?2)",
            params![id, sup],
        );
    }

    // The ledger: closed beats open, and the FIRST reason stands -- an
    // arriving closure lands only on a row still open here, so two
    // instances that both closed one commitment keep their own first
    // account and converge on "closed" without either rewriting the other.
    for cm in &d.commitments {
        let Some(id) = s(cm, "id") else { continue };
        rep.commitments += tx.execute(
            "INSERT OR IGNORE INTO commitments(id, what, kind, status, source_msg_id, \
             intent_id, due_at, created_at, closed_at, closed_why) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                id,
                s(cm, "what").unwrap_or_default(),
                s(cm, "kind").unwrap_or_else(|| "promise".into()),
                s(cm, "status").unwrap_or_else(|| "open".into()),
                s(cm, "source_msg_id"),
                s(cm, "intent_id"),
                cm.get("due_at").and_then(|v| v.as_i64()),
                i(cm, "created_at"),
                cm.get("closed_at").and_then(|v| v.as_i64()),
                s(cm, "closed_why")
            ],
        )?;
        if let (Some(closed_at), Some(why)) =
            (cm.get("closed_at").and_then(|v| v.as_i64()), s(cm, "closed_why"))
        {
            tx.execute(
                "UPDATE commitments SET status = ?2, closed_at = ?3, closed_why = ?4 \
                 WHERE id = ?1 AND closed_at IS NULL",
                params![
                    id,
                    s(cm, "status").unwrap_or_else(|| "done".into()),
                    closed_at,
                    why
                ],
            )?;
        }
    }

    // the dial: newest write per dimension wins. Two people cannot both be
    // adjusting one person's dial, so this is a straight recency merge --
    // and the bounds travel with the value, so a pin set on the machine is
    // a pin on the stick.
    for r in &d.soul_persona {
        let Some(dim) = s(r, "dimension") else { continue };
        tx.execute(
            "INSERT INTO soul_persona(dimension, value, floor, ceiling, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(dimension) DO UPDATE SET \
               value = excluded.value, floor = excluded.floor, \
               ceiling = excluded.ceiling, updated_at = excluded.updated_at \
             WHERE excluded.updated_at > soul_persona.updated_at",
            params![
                dim,
                i(r, "value"),
                i(r, "floor"),
                i(r, "ceiling"),
                i(r, "updated_at")
            ],
        )
        .ok();
    }
    // revisions are append-only history: union, never overwrite
    for r in &d.soul_revisions {
        let Some(id) = s(r, "id") else { continue };
        tx.execute(
            "INSERT OR IGNORE INTO soul_revisions(id, created_at, reason, diff_json, \
             rolls_back_to, applied, evaluator_verdict) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                id,
                i(r, "created_at"),
                s(r, "reason").unwrap_or_default(),
                s(r, "diff_json").unwrap_or_else(|| "{}".into()),
                s(r, "rolls_back_to"),
                i(r, "applied"),
                s(r, "evaluator_verdict")
            ],
        )
        .ok();
    }

    for (k, v, _) in &d.meta {
        tx.execute(
            "INSERT INTO cell_meta(key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![k, v],
        )?;
    }

    tx.commit()?;
    Ok(rep)
}

/// Drop tombstones every peer has already applied. The deletion has done
/// its work by then, and keeping the list forever would turn "deleted for
/// real" into "deleted, and permanently listed as deleted".
pub fn collect_tombstones(conn: &Connection, acked_through: i64) -> Result<usize, MindError> {
    Ok(conn.execute(
        "DELETE FROM tombstones WHERE deleted_at <= ?1",
        params![acked_through],
    )?)
}

/// The newest tombstone we hold, so a peer can tell us how far it has read.
pub fn tombstone_high_water(conn: &Connection) -> Result<i64, MindError> {
    Ok(conn
        .query_row("SELECT COALESCE(MAX(deleted_at), 0) FROM tombstones", [], |r| {
            r.get(0)
        })
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_cell_schema(&conn).unwrap();
        conn
    }

    fn say(conn: &Connection, id: &str, text: &str, ts: i64) {
        conn.execute(
            "INSERT INTO messages(id, ts, direction, surface, content) \
             VALUES (?1, ?2, 'in', 'chat', ?3)",
            params![id, ts, text],
        )
        .unwrap();
    }

    fn fact(conn: &Connection, id: &str, content: &str, src: &str, ts: i64) {
        conn.execute(
            "INSERT INTO facts(id, content, source_msg_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, content, src, ts],
        )
        .unwrap();
    }

    fn doc(conn: &Connection, id: &str, name: &str, hash: &str, src: &str, at: i64) {
        conn.execute(
            "INSERT OR IGNORE INTO media(hash, size, created_at) VALUES (?1, 10, ?2)",
            params![hash, at],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files(id, name, hash, size, class, source_msg_id, \
                               created_at, updated_at) \
             VALUES (?1, ?2, ?3, 10, 'owner_private', ?4, ?5, ?5)",
            params![id, name, hash, src, at],
        )
        .unwrap();
    }

    fn names(conn: &Connection) -> Vec<String> {
        let mut st = conn.prepare("SELECT name FROM files ORDER BY name").unwrap();
        let v = st
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        v
    }

    /// A document written on the stick is a document on the machine.
    #[test]
    fn files_travel_with_their_bytes_and_their_class() {
        let a = cell();
        let b = cell();
        say(&a, "m1", "save this", 10);
        doc(&a, "f1", "notes.md", "hash_a", "m1", 20);
        // a local-only file is not a travelling file -- that is the class
        say(&a, "m2", "and this", 10);
        a.execute(
            "INSERT INTO files(id, name, hash, size, class, source_msg_id, \
                               created_at, updated_at) \
             VALUES ('f2', 'secret.md', 'hash_a', 10, 'local_only', 'm2', 20, 20)",
            [],
        )
        .unwrap();

        let d = export(&a, 0).unwrap();
        assert_eq!(d.files.len(), 1, "local_only never leaves");
        let rep = apply(&b, &d).unwrap();
        assert_eq!(rep.files, 1);
        assert_eq!(names(&b), vec!["notes.md"]);

        // idempotent: the same delta twice changes nothing
        assert_eq!(apply(&b, &d).unwrap().files, 0);
    }

    /// Rule 2 for documents: the newer edit keeps the name, the older is
    /// kept beside it. Losing a document edit silently is the one outcome
    /// worth any amount of ugliness to avoid.
    #[test]
    fn a_document_edited_on_both_sides_loses_neither_edit() {
        let a = cell();
        let b = cell();
        say(&a, "m1", "save this", 10);
        say(&b, "m1", "save this", 10);
        doc(&a, "f1", "notes.md", "hash_new", "m1", 200);
        doc(&b, "f1", "notes.md", "hash_old", "m1", 100);

        apply(&b, &export(&a, 0).unwrap()).unwrap();
        assert_eq!(
            names(&b),
            vec!["notes.md", "notes.md (conflicted copy)"],
            "both edits survive"
        );
        let winner: String = b
            .query_row("SELECT hash FROM files WHERE name = 'notes.md'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(winner, "hash_new", "the newer edit keeps the name");

        // and it converges: syncing again breeds no further copies
        apply(&b, &export(&a, 0).unwrap()).unwrap();
        assert_eq!(names(&b).len(), 2);
    }

    /// A deleted document must not come back from the other instance.
    #[test]
    fn a_deleted_file_stays_deleted_across_a_sync() {
        let a = cell();
        let b = cell();
        say(&a, "m1", "save this", 10);
        say(&b, "m1", "save this", 10);
        doc(&a, "f1", "notes.md", "hash_a", "m1", 20);
        doc(&b, "f1", "notes.md", "hash_a", "m1", 20);

        assert!(crate::files::delete(&b, "notes.md", "stick").unwrap());
        apply(&b, &export(&a, 0).unwrap()).unwrap();
        assert!(names(&b).is_empty(), "the erasure held");

        // and it propagates the other way
        apply(&a, &export(&b, 0).unwrap()).unwrap();
        assert!(names(&a).is_empty());
    }

    /// A rule dropped on one machine must not keep running on the other:
    /// retired beats active, with no clock to argue with.
    #[test]
    fn a_retired_instruction_stays_retired_across_a_sync() {
        let a = cell();
        let b = cell();
        say(&a, "m1", "from now on, no meetings before 10", 10);
        say(&b, "m1", "from now on, no meetings before 10", 10);
        crate::instructions::add(&a, "no meetings before 10", "m1", "owner_private").unwrap();
        apply(&b, &export(&a, 0).unwrap()).unwrap();
        assert_eq!(crate::instructions::active(&b).unwrap().len(), 1, "it travelled");

        // dropped on b; a syncs from b -> retired everywhere
        crate::instructions::retire(&b, 1).unwrap().unwrap();
        apply(&a, &export(&b, 0).unwrap()).unwrap();
        assert!(
            crate::instructions::active(&a).unwrap().is_empty(),
            "following a dropped rule does the thing they said to stop doing"
        );

        // and a local_only rule never travels at all
        say(&b, "m2", "secret rule", 20);
        crate::instructions::add(&b, "the safe code is 4471", "m2", "local_only").unwrap();
        let d = export(&b, 0).unwrap();
        assert!(
            d.instructions.iter().all(|r| r
                .get("body")
                .and_then(|v| v.as_str())
                .map(|b| !b.contains("4471"))
                .unwrap_or(true)),
            "local_only leaked into the delta"
        );
    }

    /// The ledger converges on "closed", and the first reason stands --
    /// the instance that watched it happen keeps its account.
    #[test]
    fn a_closed_commitment_closes_everywhere_with_its_first_reason() {
        let a = cell();
        let b = cell();
        crate::commitments::open(&a, "rem_1", "call mark", "reminder", "open", None, None, None)
            .unwrap();
        apply(&b, &export(&a, 0).unwrap()).unwrap();
        assert_eq!(crate::commitments::outstanding(&b).unwrap().len(), 1);

        crate::commitments::close(&a, "rem_1", "done", "fired on time").unwrap();
        apply(&b, &export(&a, 0).unwrap()).unwrap();
        assert!(crate::commitments::outstanding(&b).unwrap().is_empty());
        let settled = crate::commitments::recently_closed(&b, 1).unwrap();
        assert_eq!(settled[0].closed_why.as_deref(), Some("fired on time"));

        // b already had its own account? the first one written locally stands
        crate::commitments::open(&a, "rem_2", "x", "reminder", "open", None, None, None).unwrap();
        apply(&b, &export(&a, 0).unwrap()).unwrap();
        crate::commitments::close(&b, "rem_2", "cancelled", "cancelled by you").unwrap();
        crate::commitments::close(&a, "rem_2", "done", "fired on time").unwrap();
        apply(&b, &export(&a, 0).unwrap()).unwrap();
        let b2 = crate::commitments::recently_closed(&b, 5).unwrap();
        let mine = b2.iter().find(|c| c.id == "cmt_rem_2").unwrap();
        assert_eq!(mine.closed_why.as_deref(), Some("cancelled by you"));
    }

    /// Sync twice, in either order, and both sides agree. Anything less is
    /// not sync, it is copying.
    #[test]
    fn two_instances_converge() {
        let a = cell();
        let b = cell();
        say(&a, "m1", "i drink green tea", 100);
        fact(&a, "f1", "i drink green tea", "m1", 100);
        say(&b, "m2", "the demo is on friday", 200);
        fact(&b, "f2", "the demo is on friday", "m2", 200);

        apply(&b, &export(&a, 0).unwrap()).unwrap();
        apply(&a, &export(&b, 0).unwrap()).unwrap();

        for c in [&a, &b] {
            let n: i64 = c
                .query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 2, "both instances hold both facts");
        }
    }

    /// The erase right survives replication: what one side deleted does not
    /// come back from the other, ever, however many times they sync.
    #[test]
    fn a_deletion_is_not_resurrected_by_the_peer() {
        let a = cell();
        let b = cell();
        say(&a, "m1", "i drink green tea", 100);
        fact(&a, "f1", "i drink green tea", "m1", 100);
        apply(&b, &export(&a, 0).unwrap()).unwrap();

        // A deletes it for real
        crate::facts::forget_by_index(&a, 1, "int_del", "inst_a")
            .unwrap()
            .unwrap();

        // B still has it and pushes; A must refuse
        let back = apply(&a, &export(&b, 0).unwrap()).unwrap();
        assert_eq!(back.refused_resurrections, 1);
        assert_eq!(
            a.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0,
            "the deleted fact must not return"
        );

        // and A's tombstone removes it from B
        let rep = apply(&b, &export(&a, 0).unwrap()).unwrap();
        assert_eq!(rep.deleted, 1);
        assert_eq!(
            b.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );

        // repeated syncs keep it gone
        for _ in 0..3 {
            apply(&a, &export(&b, 0).unwrap()).unwrap();
            apply(&b, &export(&a, 0).unwrap()).unwrap();
        }
        for c in [&a, &b] {
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    /// Both corrections survive: nothing is discarded because two clocks
    /// disagreed, and the registry still shows the history.
    #[test]
    fn conflicting_corrections_both_survive() {
        let a = cell();
        let b = cell();
        say(&a, "m1", "the demo is on friday", 100);
        fact(&a, "f1", "the demo is on friday", "m1", 100);
        apply(&b, &export(&a, 0).unwrap()).unwrap();

        // each side corrects the same ancestor, differently
        say(&a, "m2", "no, monday", 200);
        fact(&a, "f2", "the demo is on monday", "m2", 200);
        a.execute(
            "UPDATE facts SET superseded_by = 'f2', status = 'superseded' WHERE id = 'f1'",
            [],
        )
        .unwrap();
        say(&b, "m3", "no, tuesday", 210);
        fact(&b, "f3", "the demo is on tuesday", "m3", 210);
        b.execute(
            "UPDATE facts SET superseded_by = 'f3', status = 'superseded' WHERE id = 'f1'",
            [],
        )
        .unwrap();

        apply(&b, &export(&a, 0).unwrap()).unwrap();
        apply(&a, &export(&b, 0).unwrap()).unwrap();

        for c in [&a, &b] {
            let n: i64 = c
                .query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 3, "the ancestor and both corrections are all held");
        }
    }

    /// A cancelled reminder must not be resurrected by a peer that still
    /// thinks it is live.
    #[test]
    fn a_terminal_reminder_state_wins() {
        let a = cell();
        let b = cell();
        for c in [&a, &b] {
            c.execute(
                "INSERT INTO reminders(id, intent_id, created_at, fire_at, about, status) \
                 VALUES ('r1', 'i1', 100, 999999, 'stretch', 'active')",
                [],
            )
            .unwrap();
        }
        a.execute("UPDATE reminders SET status = 'cancelled' WHERE id = 'r1'", [])
            .unwrap();

        apply(&b, &export(&a, 0).unwrap()).unwrap();
        apply(&a, &export(&b, 0).unwrap()).unwrap();

        for c in [&a, &b] {
            let st: String = c
                .query_row("SELECT status FROM reminders WHERE id = 'r1'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(st, "cancelled", "a called-off reminder must stay called off");
        }
    }

    /// A class that protected one machine and not the other would be worse
    /// than none, because it would read as protection.
    #[test]
    fn a_facts_class_travels_with_it_and_local_only_does_not_travel_at_all() {
        let a = cell();
        let b = cell();
        say(&a, "m1", "my passport number", 100);
        fact(&a, "f1", "my passport number", "m1", 100);
        say(&a, "m2", "the wifi code", 110);
        fact(&a, "f2", "the wifi code", "m2", 110);
        a.execute("UPDATE facts SET class='restricted' WHERE id='f1'", [])
            .unwrap();
        a.execute("UPDATE facts SET class='local_only' WHERE id='f2'", [])
            .unwrap();

        apply(&b, &export(&a, 0).unwrap()).unwrap();

        let class: String = b
            .query_row("SELECT class FROM facts WHERE id='f1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(class, "restricted", "the class crossed with the fact");

        let local: i64 = b
            .query_row("SELECT COUNT(*) FROM facts WHERE id='f2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(local, 0, "local_only means local_only");
    }

    /// A fact whose source message did not travel is not stored. Law 5 is a
    /// constraint at the boundary too, not only inside one instance.
    #[test]
    fn a_fact_without_its_source_is_refused() {
        let a = cell();
        let b = cell();
        say(&a, "m1", "i drink green tea", 100);
        fact(&a, "f1", "i drink green tea", "m1", 100);

        let mut d = export(&a, 0).unwrap();
        d.messages.clear(); // the source never arrives
        let rep = apply(&b, &d).unwrap();
        assert_eq!(rep.facts, 0, "no fact without the words it came from");
    }

    /// Applying the same delta twice changes nothing, which is what makes
    /// an interrupted sync safe to simply run again.
    #[test]
    fn applying_a_delta_twice_is_a_no_op() {
        let a = cell();
        let b = cell();
        say(&a, "m1", "i drink green tea", 100);
        fact(&a, "f1", "i drink green tea", "m1", 100);
        let d = export(&a, 0).unwrap();

        let first = apply(&b, &d).unwrap();
        let second = apply(&b, &d).unwrap();
        assert_eq!(first.total(), 2);
        assert_eq!(second.total(), 0, "the second application is inert");
    }
}
