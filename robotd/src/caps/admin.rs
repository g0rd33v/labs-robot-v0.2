//! Owner-only capabilities. The role check is the first thing each one
//! does, via `Ctx::require_owner` -- see `caps::Ctx` for why that ordering
//! matters (it used to sit behind an availability check that every test
//! tripped first, so the comparison never ran).

use super::{attested, note_evidence, row_evidence, Capability, Ctx};
use prism::types::{Effect, Outcome};
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
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let core = match ctx.require_owner("mint invites") {
            Ok(c) => c,
            Err(why) => return attested(note_evidence("member.invite"), why),
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
            ctx.say(
                "invite_created",
                &[(
                    "link",
                    &format!("{}/i/{token}", ctx.instance.public_base),
                )],
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
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let core = match ctx.require_owner("bind telegram") {
            Ok(c) => c,
            Err(why) => return attested(note_evidence("telegram.bind_code"), why),
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
            ctx.say("telegram_bind_code", &[("code", &code)]),
        )
    }
}
