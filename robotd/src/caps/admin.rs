//! Owner-only capabilities. The role check is the first thing each one
//! does, via `Ctx::require_owner` -- see `caps::Ctx` for why that ordering
//! matters (it used to sit behind an availability check that every test
//! tripped first, so the comparison never ran).

use super::{attested, row_evidence, Capability, Ctx};
use prism::types::{Effect, Outcome, Rendering};
use prism::PrismError;
use rusqlite::params;
use trust::schema;

pub struct Invite;

impl Capability for Invite {
    fn name(&self) -> &'static str {
        "member.invite"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Mint a single-use invite link so another person can join this robot \
         with their own sealed, private cell. Owner only."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let core = match ctx.require_owner("mint invites") {
            Ok(c) => c,
            Err(say) => return super::declined("member.invite", say),
        };
        let token = trust::ids::random_hex(12);
        {
            let core = core
                .lock()
                .map_err(|_| PrismError::Capability("core lock poisoned".into()))?;
            core.execute(
                "INSERT INTO invites(token_hash, role, created_at) VALUES (?1,'member',?2)",
                params![trust::ids::sha256_hex(token.as_bytes()), trust::ids::ts_ms()],
            )
            .map_err(|e| PrismError::Capability(e.to_string()))?;
        }
        attested(
            row_evidence("invite", ""),
            "minted a single-use invite",
            Rendering::new(
                "invite_created",
                serde_json::json!({
                    "link": format!("{}/i/{token}", ctx.instance.public_base)
                }),
            ),
        )
    }
}

pub struct TelegramBindCode;

impl Capability for TelegramBindCode {
    fn name(&self) -> &'static str {
        "telegram.bind_code"
    }
    fn effect(&self) -> Effect {
        Effect::ReversibleWrite
    }
    fn description(&self) -> &'static str {
        "Issue a short-lived code that binds a Telegram chat to this robot, \
         so the person can talk to it from Telegram. Owner only."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let core = match ctx.require_owner("bind telegram") {
            Ok(c) => c,
            Err(say) => return super::declined("telegram.bind_code", say),
        };
        let code = format!(
            "{:06}",
            u32::from_str_radix(&trust::ids::random_hex(4), 16).unwrap_or(0) % 1_000_000
        );
        {
            let core = core
                .lock()
                .map_err(|_| PrismError::Capability("core lock poisoned".into()))?;
            schema::meta_set(
                &core,
                "tg_bind_code_hash",
                &trust::ids::sha256_hex(code.as_bytes()),
            )
            .map_err(|e| PrismError::Capability(e.to_string()))?;
            schema::meta_set(
                &core,
                "tg_bind_expiry",
                &(trust::ids::ts_ms() + 10 * 60_000).to_string(),
            )
            .map_err(|e| PrismError::Capability(e.to_string()))?;
        }
        attested(
            row_evidence("telegram.bind_code", ""),
            "issued a telegram bind code valid for ten minutes",
            Rendering::new("telegram_bind_code", serde_json::json!({ "code": code })),
        )
    }
}
