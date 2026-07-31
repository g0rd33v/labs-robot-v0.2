//! The persona dial: five dimensions, and who is allowed to move what.
//!
//! **Values are Soul's; bounds are the owner's.** Adaptation may move a
//! value inside `[floor, ceiling]` and can do nothing else. Only the owner
//! moves the bounds. `floor == ceiling` pins a dimension, which is the
//! intended way to say *stop changing this* -- expressed as a constraint
//! the code cannot route around rather than a flag it might forget to
//! check.
//!
//! The clamp lives in one place on purpose. Every path that sets a value
//! goes through `set_value`, so "the dial cannot leave its bounds" is one
//! function to audit rather than a property to hope for.

use crate::SoulError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Directness,
    Warmth,
    Brevity,
    Initiative,
    Formality,
}

impl Dimension {
    pub const ALL: [Dimension; 5] = [
        Dimension::Directness,
        Dimension::Warmth,
        Dimension::Brevity,
        Dimension::Initiative,
        Dimension::Formality,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Dimension::Directness => "directness",
            Dimension::Warmth => "warmth",
            Dimension::Brevity => "brevity",
            Dimension::Initiative => "initiative",
            Dimension::Formality => "formality",
        }
    }

    pub fn parse(s: &str) -> Option<Dimension> {
        Dimension::ALL
            .into_iter()
            .find(|d| d.as_str() == s.trim().to_lowercase())
    }

    /// The starting point, chosen to match the voice the robot already has
    /// rather than a neutral 50 across the board.
    pub fn default_value(&self) -> i64 {
        match self {
            Dimension::Directness => 60,
            Dimension::Warmth => 55,
            Dimension::Brevity => 70,
            // the dimension most likely to annoy, so it starts low
            Dimension::Initiative => 35,
            Dimension::Formality => 25,
        }
    }

    /// What each end means, for the person reading `/soul` and for the
    /// expression prompt. Both need it, and they must not drift apart.
    pub fn poles(&self) -> (&'static str, &'static str) {
        match self {
            Dimension::Directness => ("hedged and softened", "blunt, no cushioning"),
            Dimension::Warmth => ("clinical", "affectionate"),
            Dimension::Brevity => ("expansive", "terse"),
            Dimension::Initiative => (
                "answers only what was asked",
                "offers, suggests and follows up",
            ),
            Dimension::Formality => ("casual, lower-case", "formal register"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    pub dimension: Dimension,
    pub value: i64,
    pub floor: i64,
    pub ceiling: i64,
}

impl Setting {
    pub fn pinned(&self) -> bool {
        self.floor == self.ceiling
    }

    /// How far from the shipped default, which is what decides whether a
    /// reply needs shaping at all (see `Dial::is_default`).
    pub fn drift(&self) -> i64 {
        (self.value - self.dimension.default_value()).abs()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dial {
    pub settings: Vec<Setting>,
    pub evolution: bool,
}

impl Dial {
    pub fn get(&self, d: Dimension) -> Setting {
        self.settings
            .iter()
            .copied()
            .find(|s| s.dimension == d)
            .unwrap_or(Setting {
                dimension: d,
                value: d.default_value(),
                floor: 0,
                ceiling: 100,
            })
    }

    /// True when every dimension still sits where it shipped.
    ///
    /// This is load-bearing for cost, not cosmetics: at the default the
    /// English renderer uses its templates, which are free, instant and
    /// work with no network. Shaping a reply to a moved dial needs a model,
    /// and asking for a different voice is what buys that call.
    pub fn is_default(&self) -> bool {
        self.settings.iter().all(|s| s.drift() == 0)
    }
}

const EVOLUTION_KEY: &str = "soul:evolution";

/// Load the dial, filling in defaults for anything never set.
pub fn load(conn: &Connection) -> Result<Dial, SoulError> {
    let mut settings = vec![];
    for d in Dimension::ALL {
        let row: Option<(i64, i64, i64)> = conn
            .query_row(
                "SELECT value, floor, ceiling FROM soul_persona WHERE dimension = ?1",
                params![d.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        settings.push(match row {
            Some((value, floor, ceiling)) => Setting {
                dimension: d,
                value,
                floor,
                ceiling,
            },
            None => Setting {
                dimension: d,
                value: d.default_value(),
                floor: 0,
                ceiling: 100,
            },
        });
    }
    let evolution = conn
        .query_row(
            "SELECT value FROM cell_meta WHERE key = ?1",
            params![EVOLUTION_KEY],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .map(|v| v != "off")
        .unwrap_or(true);
    Ok(Dial {
        settings,
        evolution,
    })
}

fn upsert(conn: &Connection, s: Setting) -> Result<(), SoulError> {
    conn.execute(
        "INSERT INTO soul_persona(dimension, value, floor, ceiling, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(dimension) DO UPDATE SET value = excluded.value, \
           floor = excluded.floor, ceiling = excluded.ceiling, \
           updated_at = excluded.updated_at",
        params![
            s.dimension.as_str(),
            s.value,
            s.floor,
            s.ceiling,
            trust::ids::ts_ms()
        ],
    )?;
    Ok(())
}

/// Move a value. **The one place a value changes**, so the bounds are one
/// function to audit rather than a property to hope for.
///
/// Out-of-range is refused rather than clamped: silently rounding a
/// deliberate instruction down to the ceiling would leave the person
/// believing they had set something they had not.
pub fn set_value(conn: &Connection, d: Dimension, value: i64) -> Result<Setting, SoulError> {
    let cur = load(conn)?.get(d);
    if cur.pinned() {
        return Err(SoulError::Refused(format!(
            "{} is pinned at {}",
            d.as_str(),
            cur.value
        )));
    }
    if !(0..=100).contains(&value) {
        return Err(SoulError::Refused("a dial runs from 0 to 100".into()));
    }
    if value < cur.floor || value > cur.ceiling {
        return Err(SoulError::Refused(format!(
            "{} is bounded to {}..{}",
            d.as_str(),
            cur.floor,
            cur.ceiling
        )));
    }
    let next = Setting { value, ..cur };
    upsert(conn, next)?;
    Ok(next)
}

/// Move the bounds. **Owner only** -- the caller enforces that; this
/// enforces that a value cannot be left outside its own bounds.
pub fn set_bounds(
    conn: &Connection,
    d: Dimension,
    floor: i64,
    ceiling: i64,
) -> Result<Setting, SoulError> {
    if !(0..=100).contains(&floor) || !(0..=100).contains(&ceiling) || floor > ceiling {
        return Err(SoulError::Refused(
            "bounds run from 0 to 100, and the floor cannot exceed the ceiling".into(),
        ));
    }
    let cur = load(conn)?.get(d);
    // tightening bounds around a value pulls the value in with them; the
    // alternative is a row that violates its own constraint
    let value = cur.value.clamp(floor, ceiling);
    let next = Setting {
        dimension: d,
        value,
        floor,
        ceiling,
    };
    upsert(conn, next)?;
    Ok(next)
}

/// Freeze a dimension where it stands.
pub fn pin(conn: &Connection, d: Dimension) -> Result<Setting, SoulError> {
    let cur = load(conn)?.get(d);
    set_bounds(conn, d, cur.value, cur.value)
}

/// Unpin, restoring the full range.
pub fn unpin(conn: &Connection, d: Dimension) -> Result<Setting, SoulError> {
    set_bounds(conn, d, 0, 100)
}

pub fn set_evolution(conn: &Connection, on: bool) -> Result<(), SoulError> {
    conn.execute(
        "INSERT INTO cell_meta(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![EVOLUTION_KEY, if on { "on" } else { "off" }],
    )?;
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
    fn an_unset_dial_reads_as_its_defaults() {
        let c = cell();
        let d = load(&c).unwrap();
        assert!(d.is_default());
        assert_eq!(d.get(Dimension::Brevity).value, 70);
        assert!(d.evolution, "evolution is on until told otherwise");
    }

    /// The S1 gate: nothing can put a value outside its bounds. Not
    /// adaptation, not the owner's own set, not a bounds change.
    #[test]
    fn the_dial_cannot_leave_its_bounds() {
        let c = cell();
        set_bounds(&c, Dimension::Warmth, 40, 60).unwrap();

        assert!(set_value(&c, Dimension::Warmth, 50).is_ok());
        assert!(
            set_value(&c, Dimension::Warmth, 90).is_err(),
            "above the ceiling must be refused, not clamped"
        );
        assert!(set_value(&c, Dimension::Warmth, 10).is_err());
        assert!(set_value(&c, Dimension::Warmth, 200).is_err());
        assert_eq!(load(&c).unwrap().get(Dimension::Warmth).value, 50);

        // tightening the bounds pulls the value in rather than leaving a row
        // that violates its own constraint
        set_bounds(&c, Dimension::Warmth, 40, 45).unwrap();
        assert_eq!(load(&c).unwrap().get(Dimension::Warmth).value, 45);
    }

    #[test]
    fn pinning_stops_a_dimension_moving_at_all() {
        let c = cell();
        set_value(&c, Dimension::Directness, 80).unwrap();
        pin(&c, Dimension::Directness).unwrap();

        let s = load(&c).unwrap().get(Dimension::Directness);
        assert!(s.pinned());
        let err = set_value(&c, Dimension::Directness, 70).unwrap_err();
        assert!(err.to_string().contains("pinned"), "{err}");

        unpin(&c, Dimension::Directness).unwrap();
        assert!(set_value(&c, Dimension::Directness, 70).is_ok());
    }

    #[test]
    fn evolution_is_a_switch_that_persists() {
        let c = cell();
        set_evolution(&c, false).unwrap();
        assert!(!load(&c).unwrap().evolution);
        set_evolution(&c, true).unwrap();
        assert!(load(&c).unwrap().evolution);
    }

    /// `is_default` decides whether a reply needs a model to shape it, so a
    /// dial nobody has touched must never trigger one.
    #[test]
    fn a_moved_dial_stops_reading_as_default() {
        let c = cell();
        assert!(load(&c).unwrap().is_default());
        set_value(&c, Dimension::Brevity, 71).unwrap();
        assert!(!load(&c).unwrap().is_default());
    }
}
