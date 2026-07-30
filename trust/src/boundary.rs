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
pub fn append(conn: &Connection, c: &Crossing) -> Result<(i64, String), TrustError> {
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

    #[test]
    fn tampering_is_detected() {
        let conn = mem_core();
        append(&conn, &crossing(Direction::In, "h1")).unwrap();
        append(&conn, &crossing(Direction::Out, "h2")).unwrap();
        conn.execute(
            "UPDATE boundary_log SET payload_hash = 'forged' WHERE seq = 1",
            [],
        )
        .unwrap();
        assert!(!verify_chain(&conn).unwrap());
    }
}
