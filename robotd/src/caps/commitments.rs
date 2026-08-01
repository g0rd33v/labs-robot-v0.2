//! The Second Law as a screen (§4.5).
//!
//! The ledger itself lives in `mind::commitments` and is written by hooks —
//! this is only the window onto it: everything still owed, and the most
//! recent closures **with their reasons**. The reason column is the
//! product: "cancelled by you", "fired on time", "declined by you; nothing
//! ran", "closed by the sweeper" — a robot that can show this list is a
//! robot whose drops are visible, which is the whole gate.

use super::{attested, mind_err, note_evidence, Capability, Ctx};
use prism::types::{Effect, Outcome, Rendering};
use prism::PrismError;

pub struct List;

impl Capability for List {
    fn name(&self) -> &'static str {
        "commitment.list"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Show everything the person has asked that is still owed -- \
         reminders waiting to fire, actions waiting for approval -- and \
         what recently closed, with the reason each closed. Use when they \
         ask what you are waiting on, what is pending, what they asked you \
         to do, or whether anything got dropped."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let (owed, settled) = ctx.cell.with(|c| {
            Ok((
                mind::commitments::outstanding(c).map_err(mind_err)?,
                mind::commitments::recently_closed(c, 5).map_err(mind_err)?,
            ))
        })?;
        let say = if owed.is_empty() && settled.is_empty() {
            Rendering::bare("commitment_list_empty")
        } else {
            let open: Vec<serde_json::Value> = owed
                .iter()
                .map(|x| {
                    serde_json::json!({
                        "what": x.what, "kind": x.kind, "due_ms": x.due_at,
                    })
                })
                .collect();
            let closed: Vec<serde_json::Value> = settled
                .iter()
                .map(|x| {
                    serde_json::json!({
                        "what": x.what, "status": x.status, "why": x.closed_why,
                    })
                })
                .collect();
            Rendering::new(
                "commitment_list",
                serde_json::json!({ "open": open, "closed": closed }),
            )
        };
        attested(
            note_evidence("commitment.list"),
            format!("{} owed, {} recently closed", owed.len(), settled.len()),
            say,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ledger_window_is_a_read() {
        assert_eq!(List.effect(), Effect::Read);
        assert!(List.validate(&serde_json::json!({})).is_ok());
    }
}
