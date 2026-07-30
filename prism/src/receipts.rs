//! Receipts (arch sec 3): compiled from system evidence, never narrated by
//! the model that acted. One receipt per intent; statuses are honest.

use crate::types::Receipt;
use crate::PrismError;
use rusqlite::{params, Connection, OptionalExtension};
use trust::ids;

/// Store a receipt. Idempotent per intent: a second store for the same
/// intent leaves the first untouched and returns it.
pub fn store(conn: &Connection, receipt: &Receipt) -> Result<Receipt, PrismError> {
    conn.execute(
        "INSERT OR IGNORE INTO receipts(receipt_id, intent_id, status, body_json, created_at) \
         VALUES (?1,?2,?3,?4,?5)",
        params![
            receipt.receipt_id,
            receipt.intent_id,
            receipt.status.as_str(),
            serde_json::to_string(receipt)?,
            ids::ts_ms()
        ],
    )?;
    Ok(get(conn, &receipt.intent_id)?.expect("receipt just stored"))
}

pub fn get(conn: &Connection, intent_id: &str) -> Result<Option<Receipt>, PrismError> {
    let body: Option<String> = conn
        .query_row(
            "SELECT body_json FROM receipts WHERE intent_id = ?1",
            params![intent_id],
            |r| r.get(0),
        )
        .optional()?;
    match body {
        Some(b) => Ok(Some(serde_json::from_str(&b)?)),
        None => Ok(None),
    }
}

pub fn count(conn: &Connection) -> Result<i64, PrismError> {
    Ok(conn.query_row("SELECT count(*) FROM receipts", [], |r| r.get(0))?)
}
