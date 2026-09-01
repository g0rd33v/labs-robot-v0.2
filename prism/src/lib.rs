//! prism: the governed execution kernel (arch sec 3).
//!
//! M2 ships the full lifecycle on real capabilities:
//! intent -> decision (deterministic floor Q17 first, verdict Q16 otherwise)
//! -> plan -> grant -> execute -> verify -> receipt -> reply, all journaled,
//! with crash replay (`replay`) and a transactional outbox (Q10/Q11).

pub mod approval;
pub mod floor;
pub mod journal;
pub mod lifecycle;
pub mod repeats;
pub mod outbox;
pub mod pending;
pub mod receipts;
pub mod replay;
pub mod types;
pub mod verdict;

pub use lifecycle::{run_turn, CapabilityRouter, TurnDeps, TurnOutput, CRASH_POINTS};
pub use types::*;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// A principal's cell, lockable in short bursts.
///
/// The kernel used to receive a `&Connection`, which meant the caller held
/// the cell's mutex for the whole turn -- including model calls. One
/// `web.research` turn could hold it for two minutes, during which that
/// person's history, dashboard, SSE and reminders all blocked on the same
/// lock, and the watchdog could not even observe the hang because it needed
/// that lock to look.
///
/// Journal writes are milliseconds of NVMe; model calls are seconds of
/// network. `Cell` keeps them apart: every database touch is a short
/// `with(...)`, and everything slow happens between them, unlocked.
#[derive(Clone)]
pub struct Cell(Arc<Mutex<Connection>>);

impl Cell {
    pub fn new(conn: Connection) -> Self {
        Self(Arc::new(Mutex::new(conn)))
    }

    pub fn from_shared(inner: Arc<Mutex<Connection>>) -> Self {
        Self(inner)
    }

    /// Run one short unit of database work with the cell locked. Do not
    /// perform network or other slow IO inside `f`.
    pub fn with<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, PrismError>,
    ) -> Result<T, PrismError> {
        let conn = self
            .0
            .lock()
            .map_err(|_| PrismError::CellUnavailable)?;
        f(&conn)
    }

    /// As `with`, for rusqlite-native calls.
    pub fn sql<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, PrismError> {
        self.with(|c| f(c).map_err(PrismError::from))
    }
}

#[derive(Debug, Error)]
pub enum PrismError {
    #[error("sql: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("trust: {0}")]
    Trust(#[from] trust::TrustError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("capability: {0}")]
    Capability(String),
    #[error("this cell is unavailable (a previous turn panicked while holding it)")]
    CellUnavailable,
    #[error("simulated crash at {0}")]
    SimulatedCrash(String),
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
    /// The recorded inbound message id -- the provenance anchor (law #5):
    /// capabilities that store knowledge take their source pointer from
    /// here, via the journaled intent_open payload.
    #[serde(default)]
    pub source_msg_id: Option<String>,
}

/// Per-cell journal + outbox + receipts tables (decisions Q10). Idempotent.
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
CREATE TABLE IF NOT EXISTS receipts (
    receipt_id TEXT PRIMARY KEY,
    intent_id  TEXT NOT NULL UNIQUE,
    status     TEXT NOT NULL,
    body_json  TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
",
    )?;
    pending::init_schema(conn)?;
    repeats::init_schema(conn)?;
    Ok(())
}
