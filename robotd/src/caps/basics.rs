//! Time, self-description and help.
//!
//! These used to be string constants inside `prism::lifecycle`, which put
//! user-facing prose in the kernel -- the thing law 4 forbids. They are
//! capabilities like any other now, which also means a person who does not
//! speak English can reach them: previously they existed only behind an
//! English floor match.

use super::{attested, note_evidence, Capability, Ctx};
use chrono::Local;
use prism::types::{Effect, Outcome};
use prism::PrismError;

pub struct TimeNow;

impl Capability for TimeNow {
    fn name(&self) -> &'static str {
        "time.now"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Tell the current local time and date. Use whenever the person asks \
         what time it is, what the date is, or what day of the week it is."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        let now = Local::now();
        attested(
            note_evidence("time.now"),
            ctx.say("time_now", &[("time", &ctx.pack().datetime("now", &now))]),
        )
    }
}

pub struct About;

impl Capability for About {
    fn name(&self) -> &'static str {
        "robot.about"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "Explain what this robot is, where it runs, and how it treats the \
         person's data. Use when they ask who or what you are."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        attested(note_evidence("robot.about"), ctx.say("self_meta", &[]))
    }
}

pub struct Help;

impl Capability for Help {
    fn name(&self) -> &'static str {
        "robot.help"
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    fn description(&self) -> &'static str {
        "List what this robot can actually do, with examples. Use when the \
         person asks for help, for commands, or what you are capable of."
    }
    fn schema(&self) -> serde_json::Value {
        super::no_args()
    }
    fn validate(&self, _args: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    fn execute(&self, ctx: &Ctx<'_>, _args: &serde_json::Value) -> Result<Outcome, PrismError> {
        attested(note_evidence("robot.help"), ctx.say("help", &[]))
    }
}
