//! Language packs: the one place where any human language appears.
//!
//! Arch sec 2d is a law: inside the binary everything is English; at the
//! surface everything is the person's language, and the boundary is crossed
//! exactly once. The kernel used to violate it in both directions -- Russian
//! phrases were hard-coded inside `prism`, while every reply was an English
//! constant delivered straight to the user. A Russian speaker got Russian
//! parsing and English answers: precisely inverted.
//!
//! Here the kernel knows only English command IDENTIFIERS (`remind`,
//! `registry_list`). Every surface phrase and every deterministic reply
//! lives in a pack. Consequences:
//!
//! * Adding Japanese is adding `lang/ja.toml`. No kernel change, ever.
//! * A language with no pack is not an error -- the floor simply does not
//!   match, the turn goes to the verdict (which is multilingual), and the
//!   model answers in that language. Coverage degrades from "instant and
//!   free" to "one model call"; nothing breaks.
//! * Deterministic replies stay deterministic in every packed language, so
//!   the offline floor is offline in Russian too, not just English.
//!
//! Packs are embedded in the binary (arch sec 2: one file that runs
//! anywhere), so a pack cannot go missing at runtime.

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// One language's surface vocabulary.
#[derive(Debug, Deserialize)]
pub struct Pack {
    pub code: String,
    #[allow(dead_code)]
    pub name: String,
    #[serde(default)]
    pub scripts: Vec<String>,
    /// command id -> whole-utterance phrases, matched exactly
    #[serde(default)]
    pub commands: HashMap<String, Vec<String>>,
    /// command id -> heads that introduce an argument
    #[serde(default)]
    pub heads: HashMap<String, Vec<Head>>,
    /// Openers that mean "hello" and nothing else. Not floor commands --
    /// they route to chitchat, not to a capability.
    #[serde(default)]
    pub greetings: Vec<String>,
    /// Free-form phrase lists keyed by signal id (`escalate_super`,
    /// `escalate_ultra`...). Anything that is "a set of phrases meaning X"
    /// and is not a command belongs here rather than in code.
    #[serde(default)]
    pub signals: HashMap<String, Vec<String>>,
    /// Phrases in which the Robot asserts it changed the world. Feeds the
    /// sec 5 / Q26 claim-vs-receipt check -- the one list where a missing
    /// translation is a SAFETY gap, not a cosmetic one, so every pack owns
    /// its own and the check scans all of them.
    #[serde(default)]
    pub effect_claims: Vec<String>,
    #[serde(default)]
    pub particles: Particles,
    #[serde(default)]
    pub replies: HashMap<String, String>,
    /// Weekday and month names, so a date is spoken in the person's
    /// language without pulling in a locale library.
    #[serde(default)]
    pub calendar: Calendar,
    /// Datetime layouts, keyed by id: `{hh} {mm} {weekday} {weekday_short}
    /// {day} {month} {month_short} {year}`.
    #[serde(default)]
    pub formats: HashMap<String, String>,
}

/// Calendar vocabulary. Monday first (ISO), January first.
#[derive(Debug, Deserialize, Default)]
pub struct Calendar {
    #[serde(default)]
    pub weekdays: Vec<String>,
    #[serde(default)]
    pub weekdays_short: Vec<String>,
    #[serde(default)]
    pub months: Vec<String>,
    #[serde(default)]
    pub months_short: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Head {
    pub phrase: String,
    pub skip: usize,
}

#[derive(Debug, Deserialize, Default)]
pub struct Particles {
    #[serde(default)]
    pub filler: Vec<String>,
    #[serde(default)]
    pub relative: Vec<String>,
    #[serde(default)]
    pub absolute: Vec<String>,
    #[serde(default)]
    pub tomorrow: Vec<String>,
    #[serde(default)]
    pub minutes: Vec<String>,
    #[serde(default)]
    pub hours: Vec<String>,
}

impl Pack {
    /// Exact whole-utterance match -> command id.
    pub fn command_for(&self, joined: &str) -> Option<&str> {
        self.commands
            .iter()
            .find(|(_, forms)| forms.iter().any(|f| f == joined))
            .map(|(id, _)| id.as_str())
    }

    /// Longest matching head for a command id: ("phrase", skip).
    /// Longest-first so "remind me" beats "remind".
    pub fn head_for(&self, id: &str, joined: &str) -> Option<&Head> {
        let mut best: Option<&Head> = None;
        for h in self.heads.get(id)?.iter() {
            let hit = joined == h.phrase || joined.starts_with(&format!("{} ", h.phrase));
            if hit && best.map(|b| h.phrase.len() > b.phrase.len()).unwrap_or(true) {
                best = Some(h);
            }
        }
        best
    }

    /// A reply template, or the English one, or the id itself. Never panics
    /// and never returns empty: a missing translation degrades to English
    /// rather than to silence.
    pub fn reply<'a>(&'a self, id: &'a str) -> &'a str {
        self.replies
            .get(id)
            .map(String::as_str)
            .or_else(|| english().replies.get(id).map(String::as_str))
            .unwrap_or(id)
    }

    /// Render a moment through the pack's `[formats]` layout `id`.
    ///
    /// Weekday and month names come from the pack, not from a locale
    /// library: `chrono`'s `%A`/`%B` are English whatever the person
    /// speaks, and shipping a locale crate to print twelve words would be a
    /// dependency for nothing. A pack that omits `[calendar]` falls back to
    /// English names, which is exactly what it had before.
    pub fn datetime(&self, id: &str, dt: &DateTime<Local>) -> String {
        const EN_DAYS: [&str; 7] = [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ];
        const EN_MONTHS: [&str; 12] = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        let d = dt.weekday().num_days_from_monday() as usize;
        let m = dt.month0() as usize;
        let pick = |list: &[String], fallback: &str, i: usize| -> String {
            list.get(i).cloned().unwrap_or_else(|| fallback.to_string())
        };
        let weekday = pick(&self.calendar.weekdays, EN_DAYS[d], d);
        let weekday_short = self
            .calendar
            .weekdays_short
            .get(d)
            .cloned()
            .unwrap_or_else(|| weekday.chars().take(3).collect());
        let month = pick(&self.calendar.months, EN_MONTHS[m], m);
        let month_short = self
            .calendar
            .months_short
            .get(m)
            .cloned()
            .unwrap_or_else(|| month.chars().take(3).collect());

        let template = self
            .formats
            .get(id)
            .or_else(|| english().formats.get(id))
            .map(String::as_str)
            .unwrap_or("{hh}:{mm} {day} {month_short}");
        fill(
            template,
            &[
                ("hh", &format!("{:02}", dt.hour())),
                ("mm", &format!("{:02}", dt.minute())),
                ("weekday", &weekday),
                ("weekday_short", &weekday_short),
                ("day", &dt.day().to_string()),
                ("month", &month),
                ("month_short", &month_short),
                ("year", &dt.year().to_string()),
            ],
        )
    }

    /// As `datetime`, from a millisecond timestamp. Out-of-range instants
    /// are shown raw rather than silently turned into a wrong time.
    pub fn datetime_ms(&self, id: &str, ms: i64) -> String {
        match Local.timestamp_millis_opt(ms).earliest() {
            Some(dt) => self.datetime(id, &dt),
            None => format!("t+{ms}ms"),
        }
    }
}

/// The pack whose greetings this utterance opens with, if any. Used by the
/// offline verdict, which must still know a hello when it sees one without
/// a model to ask.
pub fn greeting_pack(text: &str) -> Option<&'static Pack> {
    let t = text.trim().to_lowercase();
    candidates(text)
        .into_iter()
        .find(|p| p.greetings.iter().any(|g| t == *g || t.starts_with(&format!("{g} "))))
}

/// Does this utterance contain a `signals` phrase, in any packed language?
/// Signals are cues about HOW to handle a turn (escalate, hedge), so they
/// are matched across every language for the same reason effect claims are.
pub fn matches_signal(id: &str, text: &str) -> bool {
    let lower = text.to_lowercase();
    packs().values().any(|p| {
        p.signals
            .get(id)
            .is_some_and(|list| list.iter().any(|s| lower.contains(s.as_str())))
    })
}

/// Does this utterance assert, in ANY packed language, that the Robot
/// performed an effect? Deliberately narrow: first-person completions only,
/// never descriptions of what it could do.
pub fn asserts_an_effect(text: &str) -> bool {
    let lower = text.to_lowercase();
    packs()
        .values()
        .any(|p| p.effect_claims.iter().any(|c| lower.contains(c.as_str())))
}

/// Fill `{name}` placeholders. Unknown placeholders are left alone so a
/// malformed pack shows the gap instead of dropping content silently.
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

// ------------------------------------------------------------- registry

const EMBEDDED: [(&str, &str); 2] = [
    ("en", include_str!("lang/en.toml")),
    ("ru", include_str!("lang/ru.toml")),
];

fn packs() -> &'static HashMap<String, Pack> {
    static PACKS: OnceLock<HashMap<String, Pack>> = OnceLock::new();
    PACKS.get_or_init(|| {
        let mut m = HashMap::new();
        for (code, body) in EMBEDDED {
            match toml::from_str::<Pack>(body) {
                Ok(p) => {
                    m.insert(p.code.clone(), p);
                }
                // A broken pack must not take the Robot down: log it and
                // carry on without that language.
                Err(e) => tracing_pack_error(code, &e.to_string()),
            }
        }
        m
    })
}

fn tracing_pack_error(code: &str, err: &str) {
    // prism deliberately has no logging dependency; the kernel stays quiet.
    // A malformed pack is a build-time bug and the pack test catches it.
    debug_assert!(false, "language pack {code} failed to parse: {err}");
}

pub fn english() -> &'static Pack {
    packs().get("en").expect("the english pack is canonical")
}

pub fn pack(code: &str) -> Option<&'static Pack> {
    packs().get(code)
}

pub fn codes() -> Vec<&'static str> {
    let mut c: Vec<&str> = packs().keys().map(String::as_str).collect();
    c.sort();
    c
}

/// Detect the language of an utterance from its script, deterministically
/// and without a model call.
///
/// Script is a strong, cheap signal for non-Latin languages (Cyrillic,
/// CJK, Arabic, Hebrew, Greek, Thai, Devanagari...). It cannot separate
/// languages that share the Latin alphabet -- Spanish from Portuguese --
/// so for Latin text this returns `None` and the caller falls back to
/// phrase matching first and the verdict's `lang` field second. That is the
/// honest division of labour: deterministic where determinism is real,
/// model where it is not.
pub fn detect_script(text: &str) -> Option<&'static str> {
    let mut cyr = 0usize;
    let mut latin = 0usize;
    let mut other = 0usize;
    for ch in text.chars().filter(|c| c.is_alphabetic()) {
        let c = ch as u32;
        if (0x0400..=0x04FF).contains(&c) {
            cyr += 1;
        } else if ch.is_ascii_alphabetic() || (0x00C0..=0x024F).contains(&c) {
            latin += 1;
        } else {
            other += 1;
        }
    }
    let total = cyr + latin + other;
    if total == 0 {
        return None;
    }
    if cyr * 2 > total {
        return Some("Cyrillic");
    }
    if latin * 2 > total {
        return Some("Latin");
    }
    None
}

/// Packs whose declared scripts match this text, most specific first.
/// Latin-script packs are always considered, since script cannot pick
/// between them.
pub fn candidates(text: &str) -> Vec<&'static Pack> {
    let script = detect_script(text);
    let mut out: Vec<&'static Pack> = vec![];
    if let Some(s) = script {
        for p in packs().values() {
            if p.scripts.iter().any(|x| x == s) && p.code != "en" {
                out.push(p);
            }
        }
    }
    // english is the canonical fallback and always tried last
    out.push(english());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pack must parse, and every pack must speak about the same
    /// command ids the canonical pack defines. A typo in a pack is a
    /// silently missing command otherwise.
    #[test]
    fn every_pack_parses_and_uses_canonical_command_ids() {
        let en = english();
        let known: Vec<&str> = en
            .commands
            .keys()
            .chain(en.heads.keys())
            .map(String::as_str)
            .collect();
        assert!(!known.is_empty());

        for code in codes() {
            let p = pack(code).unwrap();
            assert_eq!(p.code, code);
            for id in p.commands.keys().chain(p.heads.keys()) {
                assert!(
                    known.contains(&id.as_str()),
                    "pack '{code}' defines '{id}', which the canonical pack does not know"
                );
            }
            // no pack may leave a reply id dangling with an empty string
            for (id, text) in &p.replies {
                assert!(!text.trim().is_empty(), "pack '{code}' reply '{id}' is empty");
            }
        }
    }

    /// A pack that ships must be complete. Falling back to English is the
    /// safety net for a pack written by someone else mid-flight, not a
    /// licence for the packs in this repo to be half-translated: a person
    /// reading their own language and hitting an English sentence learns
    /// the Robot is lying about speaking it.
    #[test]
    fn every_shipped_pack_translates_every_reply() {
        let en = english();
        for code in codes() {
            if code == "en" {
                continue;
            }
            let p = pack(code).unwrap();
            let mut missing: Vec<&str> = en
                .replies
                .keys()
                .filter(|id| !p.replies.contains_key(*id))
                .map(String::as_str)
                .collect();
            missing.sort();
            assert!(
                missing.is_empty(),
                "pack '{code}' is missing replies: {missing:?}"
            );
            // and the same for the formats a reply can reference
            for id in en.formats.keys() {
                assert!(
                    p.formats.contains_key(id),
                    "pack '{code}' is missing the '{id}' datetime layout"
                );
            }
        }
    }

    /// The claim-vs-receipt check is a safety property, not a nicety: a
    /// pack with no effect-claim phrases is a language in which the Robot
    /// could say "i saved it" with no receipt and nothing would notice.
    #[test]
    fn every_pack_can_catch_an_unsupported_effect_claim() {
        for code in codes() {
            let p = pack(code).unwrap();
            assert!(
                !p.effect_claims.is_empty(),
                "pack '{code}' declares no effect-claim phrases"
            );
        }
        assert!(asserts_an_effect("sure, i've saved that for you"));
        assert!(asserts_an_effect("конечно, я запомнил это"));
        assert!(!asserts_an_effect("i can save things if you ask"));
    }

    /// A missing translation falls back to English rather than to nothing.
    #[test]
    fn missing_replies_fall_back_to_english() {
        let ru = pack("ru").unwrap();
        assert!(!ru.reply("time_now").is_empty());
        // an id no pack defines still returns something usable
        assert_eq!(ru.reply("no_such_reply_id"), "no_such_reply_id");
    }

    #[test]
    fn script_detection_is_deterministic() {
        assert_eq!(detect_script("который час"), Some("Cyrillic"));
        assert_eq!(detect_script("what time is it"), Some("Latin"));
        assert_eq!(detect_script("¿qué hora es?"), Some("Latin"));
        assert_eq!(detect_script("今何時ですか"), None); // no latin/cyrillic majority
        assert_eq!(detect_script("12:30"), None); // no letters at all
        assert_eq!(detect_script(""), None);
    }

    /// The point of the whole design: a language with no pack must not be
    /// an error. It simply has no floor coverage.
    #[test]
    fn an_unpacked_language_is_not_an_error() {
        for text in ["今何時ですか", "كم الساعة", "現在幾點", "मुझे याद दिलाओ"] {
            let c = candidates(text);
            assert!(!c.is_empty(), "candidates must never be empty");
            // english is always the last resort, and it will simply not match
            assert_eq!(c.last().unwrap().code, "en");
            assert!(c.last().unwrap().command_for(text).is_none());
        }
    }

    #[test]
    fn fill_leaves_unknown_placeholders_visible() {
        assert_eq!(fill("it's {time}", &[("time", "10:00")]), "it's 10:00");
        assert_eq!(fill("hi {who}", &[("x", "y")]), "hi {who}");
    }
}
