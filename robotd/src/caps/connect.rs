//! Connecting and disconnecting accounts.
//!
//! Connecting is deliberately not a tool a model can call. It is a control
//! surface: it hands the person a URL that grants a robot standing access to
//! their mail and calendar, and the one thing that must never happen is a
//! turn where something in the conversation talks the robot into producing
//! that link and something else follows it. So `connect.start` is
//! `exposed() == false` and reachable only from the slash command and the
//! dashboard, where a person is unambiguously the one asking.
//!
//! Disconnecting is the opposite: it should be as easy as possible, so it
//! is a tool, it needs no confirmation, and it deletes the row outright.

use super::{attested, mind_err, note_evidence, typed, Capability, Ctx};
use prism::types::{Effect, Outcome, Rendering};
use prism::PrismError;
use serde::Deserialize;

/// Begin linking a Google account.
///
/// Owner-only and NOT in the tool catalog. The link this produces grants
/// standing access to a mailbox, so the only thing that may ask for one is
/// a person typing `/connect` — never a model, and therefore never a
/// sentence inside a web page or an email that a model happened to read.
pub struct Start;

impl Capability for Start {
    fn name(&self) -> &'static str {
        "connect.start"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn exposed(&self) -> bool {
        false
    }
    fn description(&self) -> &'static str {
        "Begin connecting a Google account. Reached only by typing /connect."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        if ctx.principal != ctx.instance.owner_principal {
            return super::declined(
                "connect.start",
                Rendering::new("owner_only", serde_json::json!({ "what": "connecting accounts" })),
            );
        }
        let Some(app) = &ctx.services.oauth_app else {
            return super::declined(
                "connect.start",
                Rendering::bare("connect_unconfigured"),
            );
        };
        let scopes = hub::google::base_scopes();
        let (url, attempt) = hub::oauth::begin(app, "google", &scopes, ctx.principal);
        let Some(pending) = &ctx.services.pending_auth else {
            return super::declined("connect.start", Rendering::bare("connect_unconfigured"));
        };
        pending
            .lock()
            .map_err(|_| PrismError::Capability("pending sign-ins unavailable".into()))?
            .insert(attempt.state.clone(), attempt);
        attested(
            note_evidence("connect.start"),
            "offered a google consent link".to_string(),
            Rendering::new(
                "connect_start",
                serde_json::json!({ "provider": "google", "url": url }),
            ),
        )
    }
}

pub struct Status;

impl Capability for Status {
    fn name(&self) -> &'static str {
        "connect.status"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Show which outside accounts are connected, which account each one \
         is, and what it is allowed to do. Use when the person asks whether \
         their calendar or email is connected, or why something is not \
         working."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let all = ctx
            .cell
            .with(|c| mind::connections::list(c).map_err(mind_err))?;
        let say = if all.is_empty() {
            Rendering::bare("connect_none")
        } else {
            let items: Vec<serde_json::Value> = all
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "provider": c.provider,
                        "account": c.account,
                        "calendar": c.has_scope(hub::google::SCOPE_CALENDAR),
                        "mail_read": c.has_scope(hub::google::SCOPE_MAIL_READ),
                        "mail_send": c.has_scope(hub::google::SCOPE_MAIL_SEND),
                    })
                })
                .collect();
            Rendering::new("connect_status", serde_json::json!({ "items": items }))
        };
        attested(
            note_evidence("connect.status"),
            format!("{} accounts connected", all.len()),
            say,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderArgs {
    provider: String,
}

pub struct Disconnect;

impl Capability for Disconnect {
    fn name(&self) -> &'static str {
        "connect.forget"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Disconnect an outside account and delete its stored access. Use \
         whenever the person asks to disconnect, unlink or revoke one. It \
         takes effect immediately and they can always connect again."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "provider": {
                    "type": "string",
                    "enum": ["google"],
                    "description": "Which account to disconnect."
                }
            },
            "required": ["provider"],
            "additionalProperties": false
        })
    }
    fn validate(&self, args: &serde_json::Value) -> Result<(), String> {
        let a: ProviderArgs = typed(args)?;
        match a.provider.as_str() {
            "google" => Ok(()),
            other => Err(format!("no connector named {other}")),
        }
    }
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let a: ProviderArgs = typed(args).map_err(PrismError::Capability)?;
        let gone = ctx
            .cell
            .with(|c| mind::connections::disconnect(c, &a.provider).map_err(mind_err))?;
        let say = if gone {
            Rendering::new(
                "connect_forgotten",
                serde_json::json!({ "provider": a.provider }),
            )
        } else {
            Rendering::new(
                "connect_absent",
                serde_json::json!({ "provider": a.provider }),
            )
        };
        attested(
            note_evidence("connect.forget"),
            format!("disconnected {} ({gone})", a.provider),
            say,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_providers_can_be_named() {
        assert!(Disconnect
            .validate(&serde_json::json!({"provider": "google"}))
            .is_ok());
        assert!(Disconnect
            .validate(&serde_json::json!({"provider": "whatever"}))
            .is_err());
    }
}
