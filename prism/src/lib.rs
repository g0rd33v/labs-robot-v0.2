//! prism: the governed execution kernel (arch sec 3).
//!
//! M1 ships: the event envelope, the per-cell durable journal + outbox
//! schema (decisions Q10), and a minimal journaled turn. The full lifecycle
//! (verdict -> plan -> grant -> execute -> verify -> receipt) lands in M2.

pub mod journal;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrismError {
    #[error("sql: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("trust: {0}")]
    Trust(#[from] trust::TrustError),
}

/// One normalized event from any surface (arch sec 10): every message becomes
/// this before it reaches Prism, regardless of where it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub surface: String,
    pub principal_id: i64,
    pub modality: String,
    pub content: String,
    pub ts: i64,
    pub device_trust: String,
}

/// Per-cell journal + outbox tables (decisions Q10). Idempotent.
pub fn init_cell_schema(conn: &Connection) -> Result<(), PrismError> {
    conn.execute_batch(
        "
CREATE TABLE IF NOT EXISTS journal (
    step_id      TEXT PRIMARY KEY,
    intent_id    TEXT NOT NULL,
    seq          INTEGER NOT NULL,
    kind         TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    started_at   INTEGER NOT NULL,
    completed_at INTEGER,
    outcome_hash TEXT,
    UNIQUE (intent_id, seq)
);
CREATE TABLE IF NOT EXISTS outbox (
    effect_id   TEXT PRIMARY KEY,
    intent_id   TEXT NOT NULL,
    dedupe_key  TEXT NOT NULL UNIQUE,
    target      TEXT NOT NULL,
    payload_ref TEXT NOT NULL,
    state       TEXT NOT NULL CHECK (state IN ('pending','sent','confirmed','failed')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
",
    )?;
    Ok(())
}

/// The M1 canned line. English-only internally (arch sec 2d); Soul's
/// user-language rendering arrives with the later milestones. It claims no
/// external effect, so the receipts law is honored by construction.
const M1_REPLY: &str = "i hear you. this is my M1 skeleton: your message is \
journaled in your encrypted cell and its boundary crossings are logged. \
my reasoning arrives with M2.";

/// Handle one turn, M1-style: open an intent, journal the reply step,
/// close the intent. Returns the reply text.
pub fn handle_turn(conn: &Connection, env: &Envelope) -> Result<String, PrismError> {
    let intent_id = trust::ids::new_id("int");
    let opened = serde_json::json!({
        "surface": env.surface,
        "modality": env.modality,
        "principal_id": env.principal_id,
        "content_hash": trust::ids::sha256_hex(env.content.as_bytes()),
        "device_trust": env.device_trust,
    });
    journal::intent_open(conn, &intent_id, &opened.to_string())?;
    let reply = M1_REPLY.to_string();
    journal::step(
        conn,
        &intent_id,
        1,
        "reply.compose",
        "{}",
        Some(&trust::ids::sha256_hex(reply.as_bytes())),
    )?;
    journal::intent_close(conn, &intent_id, "closed")?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_cell() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_cell_schema(&conn).unwrap();
        conn
    }

    fn envelope(content: &str) -> Envelope {
        Envelope {
            surface: "chat".into(),
            principal_id: 1,
            modality: "text".into(),
            content: content.into(),
            ts: trust::ids::ts_ms(),
            device_trust: "owner-session".into(),
        }
    }

    #[test]
    fn turn_is_journaled_open_step_close() {
        let conn = mem_cell();
        let reply = handle_turn(&conn, &envelope("hello")).unwrap();
        assert!(reply.contains("M1"));
        let kinds = journal::kinds_for_latest_intent(&conn).unwrap();
        assert_eq!(kinds, vec!["intent_open", "reply.compose", "intent_close"]);
    }

    #[test]
    fn outbox_rejects_duplicate_dedupe_key() {
        let conn = mem_cell();
        let insert = |id: &str| {
            conn.execute(
                "INSERT INTO outbox(effect_id,intent_id,dedupe_key,target,payload_ref,state,created_at,updated_at) \
                 VALUES (?1,'int_x','same-key','chat','ref','pending',0,0)",
                [id],
            )
        };
        insert("eff_1").unwrap();
        // the UNIQUE dedupe_key makes double-enqueue structurally impossible (Q11)
        assert!(insert("eff_2").is_err());
    }
}
