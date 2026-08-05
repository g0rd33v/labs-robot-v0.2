//! Inbound double-send coalescing (spec §4.1.6): the same message twice
//! inside two seconds is one turn, not two.
//!
//! This is the inbound mirror of `outbox::enqueue`. The outbox makes a
//! double *send* structurally impossible with a UNIQUE `dedupe_key`; this
//! makes a double *arrival* structurally harmless with a claim on a
//! fingerprint. Both exist for the same reason: "the surface disables the
//! button" is a statistical argument, and the guarantee the spec wants is
//! not statistical.
//!
//! The chat's send button *is* disabled while a turn runs, so the button
//! is not the hole. The holes are the ones a client cannot close: an HTTP
//! retry after a timeout where the first request actually landed, a second
//! tab or a phone open on the same cell, and Telegram's at-least-once
//! redelivery. None of those are rare, and each of them costs the person a
//! duplicated reply and a duplicated effect.
//!
//! **Why a table rather than an in-memory map.** Not for surviving
//! restarts — a restart takes longer than the two-second window, so a
//! claim almost never needs to outlive the process. It is a table because
//! per-person state belongs in that person's cell, where the five-category
//! census can see it. A process-local map would be a second store with its
//! own lifetime, holding hashes of what someone just said, invisible to
//! the very check that exists so nothing about a person hides outside the
//! five categories.

use crate::PrismError;
use rusqlite::{params, Connection, OptionalExtension};
use trust::ids;

/// The spec's window. Two seconds is short enough that nobody types the
/// same sentence twice inside it on purpose, and long enough to cover the
/// double-tap and the immediate retry.
pub const WINDOW_MS: i64 = 2_000;

/// What the claim decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// This message is new (or the window has passed): run the turn.
    Fresh,
    /// An identical message is already being handled, or was handled
    /// moments ago. `into` is that turn's intent, so the caller can point
    /// at the one receipt that covers both arrivals rather than inventing
    /// a second.
    Duplicate { into: String },
}

pub fn init_schema(conn: &Connection) -> Result<(), PrismError> {
    conn.execute_batch(
        "
CREATE TABLE IF NOT EXISTS inbound_claims (
    fingerprint TEXT PRIMARY KEY,
    intent_id   TEXT NOT NULL,
    ts          INTEGER NOT NULL
);
",
    )?;
    Ok(())
}

/// What makes two arrivals "the same message".
///
/// Content only, deliberately *not* the surface: the strongest case for
/// coalescing is the same words arriving through two doors at once — a
/// phone and a laptop on the same cell, or a retry that reconnected
/// elsewhere — and keying on the surface would let exactly that case
/// through. The trade is that saying one word on chat and the same word on
/// Telegram inside two seconds counts as one utterance, which it almost
/// certainly is.
fn fingerprint(content: &str) -> String {
    ids::sha256_hex(content.trim().as_bytes())
}

/// Claim this message for `intent_id`, or discover it is a duplicate.
///
/// The race this has to survive is not theoretical: a double-tap issues
/// two requests about a hundred milliseconds apart and a turn takes
/// seconds, so the second arrives squarely inside the first, and an
/// unguarded read-then-write would let both decide they were first.
///
/// **What actually serialises them is the cell lock**, not this
/// transaction. Every caller reaches this through `Cell::with`, which
/// holds the connection's mutex for the whole closure, so the read and the
/// write cannot interleave with another claim in this process. The
/// transaction is here so the pair is one durable unit, not because
/// `DEFERRED` would exclude anybody — it would not.
///
/// That is a single-process guarantee, and it is the right one for a robot
/// that owns its data directory. A second process holding the same cell
/// file open could still race this; nothing in the MVP does, and if
/// something ever does, the fix is `BEGIN IMMEDIATE` here rather than
/// hoping.
pub fn claim(
    conn: &Connection,
    content: &str,
    intent_id: &str,
    now: i64,
) -> Result<Claim, PrismError> {
    let fp = fingerprint(content);
    let tx = conn.unchecked_transaction()?;
    let prior: Option<(String, i64)> = tx
        .query_row(
            "SELECT intent_id, ts FROM inbound_claims WHERE fingerprint = ?1",
            params![fp],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((into, ts)) = prior {
        // still inside the window: this is the same utterance arriving
        // twice. The claim's timestamp is NOT refreshed -- otherwise a
        // client retrying every second would hold the window open forever
        // and the person's second, deliberate message would vanish.
        if now.saturating_sub(ts) < WINDOW_MS {
            tx.commit()?;
            return Ok(Claim::Duplicate { into });
        }
    }
    tx.execute(
        "INSERT INTO inbound_claims(fingerprint, intent_id, ts) VALUES (?1,?2,?3) \
         ON CONFLICT(fingerprint) DO UPDATE SET intent_id = excluded.intent_id, \
         ts = excluded.ts",
        params![fp, intent_id, now],
    )?;
    tx.commit()?;
    Ok(Claim::Fresh)
}

/// Drop claims older than the window.
///
/// The table would otherwise grow one row per distinct thing ever said,
/// which is a second copy of the conversation in a place nobody would
/// think to look when asked what the robot stores. Called on the
/// maintenance lane; correctness does not depend on it, because `claim`
/// compares timestamps rather than trusting the row's existence.
pub fn sweep(conn: &Connection, now: i64) -> Result<usize, PrismError> {
    let n = conn.execute(
        "DELETE FROM inbound_claims WHERE ts < ?1",
        params![now - WINDOW_MS],
    )?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_cell_schema(&conn).unwrap();
        conn
    }

    /// The property the spec asks for: twice inside two seconds is one turn.
    #[test]
    fn the_same_message_twice_inside_the_window_is_one_turn() {
        let c = cell();
        let t = 1_000_000;
        assert_eq!(claim(&c, "send the invoice", "int_1", t).unwrap(), Claim::Fresh);
        // 120ms later -- a double-tap
        assert_eq!(
            claim(&c, "send the invoice", "int_2", t + 120).unwrap(),
            Claim::Duplicate { into: "int_1".into() },
            "the second arrival must point at the FIRST turn, not its own"
        );
        // and at the edge of the window it is still the same utterance
        assert_eq!(
            claim(&c, "send the invoice", "int_3", t + 1_999).unwrap(),
            Claim::Duplicate { into: "int_1".into() }
        );
    }

    /// Past the window, the same words are a person saying it again.
    #[test]
    fn the_same_message_later_is_a_new_turn() {
        let c = cell();
        let t = 1_000_000;
        claim(&c, "what time is it", "int_1", t).unwrap();
        assert_eq!(
            claim(&c, "what time is it", "int_2", t + WINDOW_MS).unwrap(),
            Claim::Fresh,
            "two seconds later is a question, not a duplicate"
        );
        // and the new turn owns the window from here
        assert_eq!(
            claim(&c, "what time is it", "int_3", t + WINDOW_MS + 10).unwrap(),
            Claim::Duplicate { into: "int_2".into() }
        );
    }

    /// A duplicate must not push the window out.
    ///
    /// The window is measured from the FIRST arrival, never from the last
    /// one seen. If a duplicate refreshed the timestamp, a client retrying
    /// every second would hold the window open indefinitely and the
    /// person's next deliberate message — the same words, minutes later —
    /// would be swallowed with no turn and no receipt.
    ///
    /// Note what this test does NOT claim: that a retrier only ever gets
    /// one turn. Past two seconds nothing can distinguish a stubborn retry
    /// from a person saying it again, so an identical message after the
    /// window always earns its own turn. That is the cost of a time
    /// window, and it errs toward answering rather than toward silence.
    #[test]
    fn a_duplicate_does_not_extend_the_window() {
        let c = cell();
        let t = 1_000_000;
        claim(&c, "ping", "int_1", t).unwrap();
        for step in [500, 1_000, 1_500, 1_900] {
            assert_eq!(
                claim(&c, "ping", "int_x", t + step).unwrap(),
                Claim::Duplicate { into: "int_1".into() },
                "at +{step}ms this is still the first utterance"
            );
        }
        // measured from t, not from the retry at t+1900
        assert_eq!(
            claim(&c, "ping", "int_2", t + WINDOW_MS).unwrap(),
            Claim::Fresh,
            "the window was pushed out by duplicates -- a later real \
             message would be swallowed"
        );
    }

    /// Different messages never coalesce, however close together.
    #[test]
    fn different_messages_are_never_merged() {
        let c = cell();
        let t = 1_000_000;
        assert_eq!(claim(&c, "yes", "int_1", t).unwrap(), Claim::Fresh);
        assert_eq!(claim(&c, "no", "int_2", t + 5).unwrap(), Claim::Fresh);
        assert_eq!(claim(&c, "yes please", "int_3", t + 10).unwrap(), Claim::Fresh);
    }

    /// Whitespace is not meaning: a client that trims and one that does
    /// not are sending the same message.
    #[test]
    fn surrounding_whitespace_does_not_make_it_a_different_message() {
        let c = cell();
        let t = 1_000_000;
        claim(&c, "book the table", "int_1", t).unwrap();
        assert_eq!(
            claim(&c, "  book the table\n", "int_2", t + 50).unwrap(),
            Claim::Duplicate { into: "int_1".into() }
        );
    }

    /// The table must not become a shadow copy of the conversation.
    #[test]
    fn the_claim_table_does_not_accumulate() {
        let c = cell();
        let t = 1_000_000;
        for i in 0..50 {
            claim(&c, &format!("message {i}"), &format!("int_{i}"), t + i).unwrap();
        }
        let before: i64 = c
            .query_row("SELECT count(*) FROM inbound_claims", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 50);
        let swept = sweep(&c, t + 50 + WINDOW_MS).unwrap();
        assert_eq!(swept, 50);
        let after: i64 = c
            .query_row("SELECT count(*) FROM inbound_claims", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 0);
        // and sweeping does not resurrect anything: a fresh claim still works
        assert_eq!(claim(&c, "message 0", "int_new", t + 9_000).unwrap(), Claim::Fresh);
    }

    /// Sweeping must never drop a claim that is still protecting a turn.
    #[test]
    fn sweeping_leaves_the_live_window_alone() {
        let c = cell();
        let t = 1_000_000;
        claim(&c, "still running", "int_1", t).unwrap();
        sweep(&c, t + 500).unwrap();
        assert_eq!(
            claim(&c, "still running", "int_2", t + 600).unwrap(),
            Claim::Duplicate { into: "int_1".into() },
            "a live claim was swept and a duplicate got through"
        );
    }
}
