//! Stance: who the robot is to you.
//!
//! The dial's five dimensions are how a reply is worded. **Stance is the
//! thing a person actually chooses.** Nobody wants to set `directness` to
//! 60; they want a twin, or a friend, or a mentor, or a character they
//! invented. The stance is the headline and the dimensions are how it
//! cashes out — so choosing one moves all five at once, and the owner can
//! still fine-tune any of them afterwards.
//!
//! `Character` is the open end of the same axis: any role the owner
//! describes, in their own words. It is not a separate feature bolted on
//! beside the presets — twin, friend and mentor are simply the three worth
//! naming.

use crate::dial::{self, Dimension};
use crate::SoulError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const KEY: &str = "soul:stance";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stance {
    /// Like you: your register, your shorthand, no ceremony.
    Twin,
    /// Warm and easy; offers things; assumes goodwill.
    Friend,
    /// Explains, asks the better question, takes more initiative.
    Mentor,
    /// Anything the owner describes, in their words.
    Character(String),
}

impl Stance {
    pub fn parse(s: &str) -> Option<Stance> {
        let t = s.trim();
        match t.to_lowercase().as_str() {
            "twin" => Some(Stance::Twin),
            "friend" => Some(Stance::Friend),
            "mentor" => Some(Stance::Mentor),
            "" => None,
            _ => match t.strip_prefix("character:") {
                Some(brief) if !brief.trim().is_empty() => {
                    Some(Stance::Character(brief.trim().into()))
                }
                _ => None,
            },
        }
    }

    pub fn store_as(&self) -> String {
        match self {
            Stance::Twin => "twin".into(),
            Stance::Friend => "friend".into(),
            Stance::Mentor => "mentor".into(),
            Stance::Character(b) => format!("character:{b}"),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Stance::Twin => "twin".into(),
            Stance::Friend => "friend".into(),
            Stance::Mentor => "mentor".into(),
            Stance::Character(b) => format!("in character: {b}"),
        }
    }

    /// Where this stance puts the five dimensions.
    ///
    /// `Character` deliberately moves nothing: the owner described a voice,
    /// and overriding their directness setting because they asked for a
    /// pirate would be the robot second-guessing them.
    pub fn dial_preset(&self) -> Option<[(Dimension, i64); 5]> {
        use Dimension::*;
        Some(match self {
            // your own voice reflected: blunt, terse, no ceremony
            Stance::Twin => [
                (Directness, 80),
                (Warmth, 55),
                (Brevity, 85),
                (Initiative, 30),
                (Formality, 5),
            ],
            Stance::Friend => [
                (Directness, 60),
                (Warmth, 80),
                (Brevity, 60),
                (Initiative, 55),
                (Formality, 10),
            ],
            Stance::Mentor => [
                (Directness, 70),
                (Warmth, 60),
                (Brevity, 35),
                (Initiative, 75),
                (Formality, 45),
            ],
            Stance::Character(_) => return None,
        })
    }

    /// What the model is told about who it is being.
    pub fn instruction(&self) -> String {
        match self {
            Stance::Twin => "you are their twin: speak the way they speak, \
                 in their register and their shorthand. no ceremony, no \
                 explaining what they already know."
                .into(),
            Stance::Friend => "you are their friend: warm, easy, assume \
                 goodwill. offer things without being asked."
                .into(),
            Stance::Mentor => "you are their mentor: explain the reasoning, \
                 ask the better question, take the initiative on what they \
                 should consider next."
                .into(),
            Stance::Character(b) => format!("you are speaking in character: {b}"),
        }
    }
}

pub fn get(conn: &Connection) -> Result<Option<Stance>, SoulError> {
    Ok(conn
        .query_row(
            "SELECT value FROM cell_meta WHERE key = ?1",
            params![KEY],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| Stance::parse(&v)))
}

/// Take a stance, moving the dial to match.
///
/// Preset values are applied **within the owner's bounds**: a pinned
/// dimension stays pinned, and a bounded one lands at the nearest allowed
/// value rather than refusing the whole stance. Choosing "mentor" should
/// not fail because formality happens to be fenced.
pub fn set(conn: &Connection, stance: Option<&Stance>) -> Result<(), SoulError> {
    conn.execute(
        "INSERT INTO cell_meta(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![KEY, stance.map(|s| s.store_as()).unwrap_or_default()],
    )?;
    if let Some(preset) = stance.and_then(|s| s.dial_preset()) {
        let current = dial::load(conn)?;
        for (d, want) in preset {
            let cur = current.get(d);
            if cur.pinned() {
                continue;
            }
            let allowed = want.clamp(cur.floor, cur.ceiling);
            dial::set_value(conn, d, allowed)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE messages(id TEXT PRIMARY KEY);
             CREATE TABLE cell_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        crate::init_cell_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn stances_round_trip_including_a_character() {
        for s in [
            Stance::Twin,
            Stance::Friend,
            Stance::Mentor,
            Stance::Character("a laconic ship's engineer".into()),
        ] {
            assert_eq!(Stance::parse(&s.store_as()), Some(s.clone()), "{s:?}");
        }
        assert_eq!(Stance::parse(""), None);
        assert_eq!(Stance::parse("character:"), None);
    }

    /// Choosing a stance is choosing all five dimensions at once -- that is
    /// the point of having one.
    #[test]
    fn taking_a_stance_moves_the_dial() {
        let c = cell();
        set(&c, Some(&Stance::Mentor)).unwrap();
        let d = dial::load(&c).unwrap();
        assert_eq!(d.get(Dimension::Initiative).value, 75);
        assert_eq!(d.get(Dimension::Formality).value, 45);
        assert_eq!(get(&c).unwrap(), Some(Stance::Mentor));

        set(&c, Some(&Stance::Twin)).unwrap();
        let d = dial::load(&c).unwrap();
        assert_eq!(d.get(Dimension::Brevity).value, 85);
        assert_eq!(d.get(Dimension::Formality).value, 5);
    }

    /// The owner's bounds outrank a preset. A pinned dimension does not
    /// move, and a fenced one lands inside its fence -- but the stance
    /// still applies to everything else, because failing the whole thing
    /// over one fenced dimension would be useless.
    #[test]
    fn a_preset_cannot_cross_the_owners_bounds() {
        let c = cell();
        dial::set_value(&c, Dimension::Formality, 20).unwrap();
        dial::pin(&c, Dimension::Formality).unwrap();
        dial::set_bounds(&c, Dimension::Brevity, 0, 50).unwrap();

        set(&c, Some(&Stance::Twin)).unwrap();
        let d = dial::load(&c).unwrap();
        assert_eq!(d.get(Dimension::Formality).value, 20, "a pin outranks a preset");
        assert_eq!(d.get(Dimension::Brevity).value, 50, "clamped, not refused");
        assert_eq!(d.get(Dimension::Directness).value, 80, "the rest still applied");
    }

    /// A character is the owner's own words, so it moves nothing --
    /// overriding their directness because they asked for a pirate would be
    /// the robot second-guessing them.
    #[test]
    fn a_character_leaves_the_dial_alone() {
        let c = cell();
        dial::set_value(&c, Dimension::Warmth, 90).unwrap();
        set(
            &c,
            Some(&Stance::Character("a laconic ship's engineer".into())),
        )
        .unwrap();
        assert_eq!(dial::load(&c).unwrap().get(Dimension::Warmth).value, 90);
        assert!(get(&c).unwrap().unwrap().instruction().contains("engineer"));
    }
}
