//! Expression: turning the dial into instructions a model can follow.
//!
//! A number is not an instruction. `brevity 90` tells a model nothing;
//! "answer in one sentence unless asked for more" tells it exactly what to
//! do. Every dimension therefore maps to imperative lines chosen by band,
//! not to an adjective the model has to interpret.
//!
//! The poles here and the poles in `Dimension::poles` describe the same
//! thing to two audiences -- the model, and the person reading `/soul` --
//! and they must not drift apart.

use crate::dial::{Dial, Dimension};

fn band(v: i64) -> usize {
    match v {
        0..=19 => 0,
        20..=39 => 1,
        40..=59 => 2,
        60..=79 => 3,
        _ => 4,
    }
}

fn line(d: Dimension, v: i64) -> &'static str {
    let b = band(v);
    match d {
        Dimension::Directness => [
            "soften everything; offer possibilities rather than conclusions",
            "hedge where you are unsure, and say so gently",
            "say what you think, with the caveats that matter",
            "be direct; skip the cushioning",
            "be blunt. no softening, no preamble, no apologising for the answer",
        ][b],
        Dimension::Warmth => [
            "stay clinical; no pleasantries",
            "stay matter-of-fact",
            "be pleasant without being effusive",
            "be warm; a little care in the wording is right",
            "be openly warm and personal",
        ][b],
        Dimension::Brevity => [
            "take the space to explain properly",
            "explain, but do not pad",
            "keep it compact",
            "keep it short -- two or three sentences",
            "answer in one sentence unless asked for more",
        ][b],
        Dimension::Initiative => [
            "answer exactly what was asked and stop",
            "answer what was asked; mention a next step only if it is important",
            "answer, and offer an obvious next step when there is one",
            "answer, then suggest what would help next",
            "answer, suggest next steps, and ask what would be most useful",
        ][b],
        Dimension::Formality => [
            "lower-case, casual, contractions welcome",
            "casual but tidy",
            "neutral register",
            "write formally",
            "write formally and precisely; no contractions, no slang",
        ][b],
    }
}

/// The dial as instructions, or `None` when it sits at its defaults.
///
/// `None` is load-bearing rather than an optimisation: at the default the
/// English renderer uses its templates, which are free, instant and work
/// with no network. Asking for a different voice is what buys the model
/// call that shaping needs.
pub fn instructions(dial: &Dial) -> Option<String> {
    if dial.is_default() {
        return None;
    }
    let lines: Vec<String> = Dimension::ALL
        .into_iter()
        .map(|d| format!("- {}", line(d, dial.get(d).value)))
        .collect();
    Some(format!("how to say it:\n{}", lines.join("\n")))
}

/// The dial as instructions, always -- for paths that are already calling a
/// model and can carry the voice for free.
pub fn instructions_always(dial: &Dial) -> String {
    let lines: Vec<String> = Dimension::ALL
        .into_iter()
        .map(|d| format!("- {}", line(d, dial.get(d).value)))
        .collect();
    format!("how to say it:\n{}", lines.join("\n"))
}

/// Everything Soul has to say about how this reply should sound.
///
/// `None` means nothing needs shaping -- the default dial, no stance -- and
/// that is load-bearing: at `None` the English renderer uses its templates,
/// which are free, instant and work with no network.
///
/// The fence travels with the instruction rather than being assumed. The
/// person writing "you are a laconic ship's engineer" is not thinking about
/// receipts, so the model is told explicitly that a character changes how
/// things are said and nothing about what is true.
pub fn shape(dial: &Dial, stance: Option<&crate::stance::Stance>) -> Option<String> {
    let dial_part = instructions(dial);
    if dial_part.is_none() && stance.is_none() {
        return None;
    }
    let mut out = vec![];
    if let Some(s) = stance {
        out.push(s.instruction());
    }
    out.push(dial_part.unwrap_or_else(|| instructions_always(dial)));
    out.push(
        "this changes HOW you say things and nothing about what is true. facts, \
         times, names, and what you did or did not do stay exactly as given. if \
         asked sincerely whether you are a person or whether you actually feel \
         something, answer honestly -- a stance is a costume, not a claim."
            .into(),
    );
    Some(out.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dial::Setting;

    fn dial_at(pairs: &[(Dimension, i64)]) -> Dial {
        Dial {
            settings: Dimension::ALL
                .into_iter()
                .map(|d| Setting {
                    dimension: d,
                    value: pairs
                        .iter()
                        .find(|(x, _)| *x == d)
                        .map(|(_, v)| *v)
                        .unwrap_or(d.default_value()),
                    floor: 0,
                    ceiling: 100,
                })
                .collect(),
            evolution: true,
        }
    }

    /// A dial nobody has touched must not trigger a model call, because the
    /// English templates are the whole reason the floor is free and offline.
    #[test]
    fn a_default_dial_asks_for_no_shaping() {
        assert!(instructions(&dial_at(&[])).is_none());
        assert!(instructions(&dial_at(&[(Dimension::Warmth, 56)])).is_some());
    }

    /// Opposite ends must produce genuinely different instructions -- if
    /// they did not, the dial would be decoration.
    #[test]
    fn opposite_ends_read_differently() {
        let blunt = instructions_always(&dial_at(&[
            (Dimension::Directness, 100),
            (Dimension::Brevity, 100),
            (Dimension::Warmth, 0),
        ]));
        let gentle = instructions_always(&dial_at(&[
            (Dimension::Directness, 0),
            (Dimension::Brevity, 0),
            (Dimension::Warmth, 100),
        ]));
        assert_ne!(blunt, gentle);
        assert!(blunt.contains("blunt"), "{blunt}");
        assert!(gentle.contains("soften"), "{gentle}");
        assert!(blunt.contains("one sentence"), "{blunt}");
    }

    /// A stance alone is enough to need shaping, even at a default dial --
    /// and the fence always travels with it.
    #[test]
    fn a_stance_needs_shaping_even_at_the_default_dial() {
        use crate::stance::Stance;
        let d = dial_at(&[]);
        assert!(shape(&d, None).is_none());

        let mentor = shape(&d, Some(&Stance::Mentor)).unwrap();
        assert!(mentor.contains("mentor"), "{mentor}");
        assert!(mentor.contains("nothing about what is true"));
        assert!(mentor.contains("answer honestly"));

        let char = shape(
            &d,
            Some(&Stance::Character("a laconic ship's engineer".into())),
        )
        .unwrap();
        assert!(char.contains("laconic ship's engineer"));
        assert!(char.contains("costume, not a claim"));
    }

    /// Every band of every dimension has a line. A gap would render as a
    /// panic on some perfectly ordinary setting.
    #[test]
    fn every_dimension_has_a_line_at_every_value() {
        for d in Dimension::ALL {
            for v in 0..=100 {
                assert!(!line(d, v).is_empty(), "{} at {v}", d.as_str());
            }
        }
    }
}
