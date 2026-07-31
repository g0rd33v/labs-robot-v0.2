//! The Boundary Log (arch sec 7a): every crossing of the process boundary,
//! both directions, append-only, hash-chained. References and hashes only --
//! payloads live in cells, so the log never becomes a second copy of a life.

use crate::{ids, TrustError};
use rusqlite::{params, Connection, OptionalExtension};

const GENESIS: &str = "genesis";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

impl Direction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
        }
    }
}

/// One crossing of the process boundary.
pub struct Crossing {
    pub direction: Direction,
    pub channel: String,
    pub counterparty: String,
    pub purpose: String,
    pub categories: String,
    pub payload_hash: String,
    pub size: i64,
    /// owner | granted | untrusted (everything inbound from the open world
    /// is untrusted-by-origin; the local owner session is `owner`).
    pub trust_tag: String,
}

fn material(prev_hash: &str, ts: i64, c: &Crossing) -> String {
    format!(
        "{prev_hash}|{ts}|{}|{}|{}|{}|{}|{}|{}|{}",
        c.direction.as_str(),
        c.channel,
        c.counterparty,
        c.purpose,
        c.categories,
        c.payload_hash,
        c.size,
        c.trust_tag
    )
}

/// Append one crossing; returns (seq, entry_hash).
///
/// Reading the previous hash and inserting the new row happen inside one
/// IMMEDIATE transaction: as two separate statements they could interleave
/// with a second connection (the `backup`/`package` subcommands open the
/// same core.db while the daemon is live), producing two rows that share a
/// `prev_hash`. That forks the chain permanently, and `verify_chain` then
/// reports false forever -- indistinguishable from tampering.
///
/// If the caller already owns a transaction, this joins it rather than
/// nesting.
pub fn append(conn: &Connection, c: &Crossing) -> Result<(i64, String), TrustError> {
    if !conn.is_autocommit() {
        return append_in_tx(conn, c);
    }
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    match append_in_tx(conn, c) {
        Ok(v) => {
            conn.execute_batch("COMMIT;")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

fn append_in_tx(conn: &Connection, c: &Crossing) -> Result<(i64, String), TrustError> {
    let prev_hash: String = conn
        .query_row(
            "SELECT entry_hash FROM boundary_log ORDER BY seq DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_else(|| GENESIS.to_string());
    let ts = ids::ts_ms();
    let entry_hash = ids::sha256_hex(material(&prev_hash, ts, c).as_bytes());
    conn.execute(
        "INSERT INTO boundary_log \
         (ts, direction, channel, counterparty, purpose, categories, payload_hash, size, trust_tag, prev_hash, entry_hash) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            ts,
            c.direction.as_str(),
            c.channel,
            c.counterparty,
            c.purpose,
            c.categories,
            c.payload_hash,
            c.size,
            c.trust_tag,
            prev_hash,
            entry_hash
        ],
    )?;
    Ok((conn.last_insert_rowid(), entry_hash))
}

/// Recompute the chain from genesis; false if any link or hash is broken.
pub fn verify_chain(conn: &Connection) -> Result<bool, TrustError> {
    struct Row {
        ts: i64,
        direction: String,
        channel: String,
        counterparty: String,
        purpose: String,
        categories: String,
        payload_hash: String,
        size: i64,
        trust_tag: String,
        prev_hash: String,
        entry_hash: String,
    }
    let mut stmt = conn.prepare(
        "SELECT ts, direction, channel, counterparty, purpose, categories, payload_hash, size, trust_tag, prev_hash, entry_hash \
         FROM boundary_log ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Row {
            ts: r.get(0)?,
            direction: r.get(1)?,
            channel: r.get(2)?,
            counterparty: r.get(3)?,
            purpose: r.get(4)?,
            categories: r.get(5)?,
            payload_hash: r.get(6)?,
            size: r.get(7)?,
            trust_tag: r.get(8)?,
            prev_hash: r.get(9)?,
            entry_hash: r.get(10)?,
        })
    })?;
    let mut prev = GENESIS.to_string();
    for row in rows {
        let row = row?;
        if row.prev_hash != prev {
            return Ok(false);
        }
        let m = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            row.prev_hash,
            row.ts,
            row.direction,
            row.channel,
            row.counterparty,
            row.purpose,
            row.categories,
            row.payload_hash,
            row.size,
            row.trust_tag
        );
        if ids::sha256_hex(m.as_bytes()) != row.entry_hash {
            return Ok(false);
        }
        prev = row.entry_hash;
    }
    Ok(true)
}

pub fn count(conn: &Connection) -> Result<i64, TrustError> {
    Ok(conn.query_row("SELECT count(*) FROM boundary_log", [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_core() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::init_core(&conn).unwrap();
        conn
    }

    fn crossing(dir: Direction, hash: &str) -> Crossing {
        Crossing {
            direction: dir,
            channel: "chat".into(),
            counterparty: "local-web".into(),
            purpose: "conversation".into(),
            categories: "message".into(),
            payload_hash: hash.into(),
            size: 5,
            trust_tag: "owner".into(),
        }
    }

    #[test]
    fn chain_links_and_verifies() {
        let conn = mem_core();
        append(&conn, &crossing(Direction::In, "h1")).unwrap();
        append(&conn, &crossing(Direction::Out, "h2")).unwrap();
        append(&conn, &crossing(Direction::In, "h3")).unwrap();
        assert_eq!(count(&conn).unwrap(), 3);
        assert!(verify_chain(&conn).unwrap());
    }

    /// Edits are refused outright by the append-only triggers.
    #[test]
    fn the_log_cannot_be_edited_or_deleted() {
        let conn = mem_core();
        append(&conn, &crossing(Direction::In, "h1")).unwrap();
        append(&conn, &crossing(Direction::Out, "h2")).unwrap();

        let edit = conn.execute(
            "UPDATE boundary_log SET payload_hash = 'forged' WHERE seq = 1",
            [],
        );
        assert!(edit.is_err(), "boundary log must reject UPDATE");
        let delete = conn.execute("DELETE FROM boundary_log WHERE seq = 1", []);
        assert!(delete.is_err(), "boundary log must reject DELETE");

        // and the chain is still intact after the refused attempts
        assert!(verify_chain(&conn).unwrap());
        assert_eq!(count(&conn).unwrap(), 2);
    }

    /// If a row ever does get in with a broken link (a forked chain from a
    /// second connection, or a schema-level attack), verification says so.
    #[test]
    fn a_broken_link_fails_verification() {
        let conn = mem_core();
        append(&conn, &crossing(Direction::In, "h1")).unwrap();
        // an appended row that does not commit to the real previous entry
        conn.execute(
            "INSERT INTO boundary_log \
             (ts, direction, channel, counterparty, purpose, categories, payload_hash, \
              size, trust_tag, prev_hash, entry_hash) \
             VALUES (1, 'out', 'chat', 'x', 'y', '', 'h2', 1, 'owner', 'not-the-real-prev', 'zz')",
            [],
        )
        .unwrap();
        assert!(!verify_chain(&conn).unwrap());
    }

    /// Concurrent appends must not fork the chain: every row's prev_hash is
    /// the previous row's entry_hash, with no duplicates.
    #[test]
    fn appends_are_serialized_into_one_chain() {
        let conn = mem_core();
        for i in 0..25 {
            append(&conn, &crossing(Direction::In, &format!("h{i}"))).unwrap();
        }
        assert!(verify_chain(&conn).unwrap());
        let distinct: i64 = conn
            .query_row(
                "SELECT count(DISTINCT prev_hash) FROM boundary_log",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(distinct, 25, "each entry must commit to a unique predecessor");
    }
}

// ------------------------------------------------------- anchoring

/// The chain's current head: (seq, entry_hash, ts). `None` on an empty log.
pub fn head(conn: &Connection) -> Result<Option<(i64, String, i64)>, TrustError> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT seq, entry_hash, ts FROM boundary_log ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

/// The hash recorded at one position, for checking an anchor.
pub fn hash_at(conn: &Connection, seq: i64) -> Result<Option<String>, TrustError> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT entry_hash FROM boundary_log WHERE seq = ?1",
            rusqlite::params![seq],
            |r| r.get(0),
        )
        .optional()?)
}

/// A published head: what the chain looked like at a moment, recorded
/// somewhere this machine can no longer reach.
///
/// The chain is unkeyed on purpose. An HMAC would have to be keyed under
/// the KEK, and the log lives inside a database encrypted under that same
/// KEK -- so anyone able to rewrite an entry already holds the key the MAC
/// would use. It would look like protection and provide none.
///
/// What does help is putting the head somewhere the machine cannot rewrite
/// afterwards. An adversary holding the KEK can rewrite local history; they
/// cannot rewrite a hash that left for two off-site destinations last week.
/// The row below is the local copy of that record: sloppy tampering trips
/// it at the next boot, and the off-site copy is what settles an argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub seq: i64,
    pub hash: String,
    pub ts: i64,
    /// where the head was published -- a backup destination, a manifest
    pub published_to: String,
}

pub fn record_anchor(conn: &Connection, a: &Anchor) -> Result<(), TrustError> {
    conn.execute(
        "INSERT OR REPLACE INTO chain_anchors(seq, hash, ts, published_to) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![a.seq, a.hash, a.ts, a.published_to],
    )?;
    Ok(())
}

pub fn anchors(conn: &Connection) -> Result<Vec<Anchor>, TrustError> {
    let mut stmt = conn.prepare(
        "SELECT seq, hash, ts, published_to FROM chain_anchors ORDER BY seq ASC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Anchor {
                seq: r.get(0)?,
                hash: r.get(1)?,
                ts: r.get(2)?,
                published_to: r.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Every anchor that no longer matches the chain: history was rewritten
/// behind a point we had already published.
///
/// An anchor whose `seq` is missing entirely counts too -- truncating the
/// log is a rewrite, and the loudest kind.
pub fn broken_anchors(conn: &Connection) -> Result<Vec<Anchor>, TrustError> {
    let mut broken = vec![];
    for a in anchors(conn)? {
        match hash_at(conn, a.seq)? {
            Some(h) if h == a.hash => {}
            _ => broken.push(a),
        }
    }
    Ok(broken)
}

#[cfg(test)]
mod anchor_tests {
    use super::*;
    use rusqlite::Connection;

    fn core() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::init_core(&conn).unwrap();
        conn
    }

    fn crossing(purpose: &str) -> Crossing {
        Crossing {
            direction: Direction::Out,
            channel: "test".into(),
            counterparty: "peer".into(),
            purpose: purpose.into(),
            categories: String::new(),
            payload_hash: String::new(),
            size: 0,
            trust_tag: "test".into(),
        }
    }

    /// The attack the anchor exists for: an adversary holding the KEK can
    /// rewrite the log AND recompute every hash, so `verify_chain` passes on
    /// a history that never happened. Only a head published somewhere they
    /// could not reach exposes it.
    #[test]
    fn a_rewritten_history_verifies_but_fails_its_anchor() {
        let c = core();
        for i in 0..5 {
            append(&c, &crossing(&format!("real-{i}"))).unwrap();
        }
        let (seq, hash, ts) = head(&c).unwrap().unwrap();
        record_anchor(
            &c,
            &Anchor {
                seq,
                hash,
                ts,
                published_to: "backup:offsite".into(),
            },
        )
        .unwrap();
        assert!(broken_anchors(&c).unwrap().is_empty());

        // rewrite the whole log from genesis, as someone with the key could
        c.execute("DROP TRIGGER boundary_log_no_delete", []).unwrap();
        c.execute("DELETE FROM boundary_log", []).unwrap();
        c.execute("DELETE FROM sqlite_sequence WHERE name='boundary_log'", [])
            .ok();
        for i in 0..5 {
            append(&c, &crossing(&format!("forged-{i}"))).unwrap();
        }

        // the chain is internally perfect -- and a different history
        assert!(
            verify_chain(&c).unwrap(),
            "a recomputed chain verifies against itself; that is the point"
        );
        let broken = broken_anchors(&c).unwrap();
        assert_eq!(broken.len(), 1, "the published head no longer matches");
        assert_eq!(broken[0].published_to, "backup:offsite");
    }

    /// Truncation is a rewrite too, and the loudest kind.
    #[test]
    fn a_truncated_log_fails_its_anchor() {
        let c = core();
        for i in 0..4 {
            append(&c, &crossing(&format!("e{i}"))).unwrap();
        }
        let (seq, hash, ts) = head(&c).unwrap().unwrap();
        record_anchor(
            &c,
            &Anchor {
                seq,
                hash,
                ts,
                published_to: "backup:offsite".into(),
            },
        )
        .unwrap();

        c.execute("DROP TRIGGER boundary_log_no_delete", []).unwrap();
        c.execute("DELETE FROM boundary_log WHERE seq >= ?1", [seq])
            .unwrap();
        assert_eq!(
            broken_anchors(&c).unwrap().len(),
            1,
            "an anchor whose entry is simply gone must count as broken"
        );
    }
}
