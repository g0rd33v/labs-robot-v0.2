//! The deterministic floor (decisions Q17): runs first and wins
//! unconditionally -- a deterministic match never yields to a model verdict.
//! The cheapest call is no call; this is also the offline floor.
//!
//! LANGUAGE: this file contains no phrase in any human language, including
//! English. Every surface form lives in a language pack (see `lexicon`), and
//! the floor works in terms of command IDs. That is arch sec 2d made
//! structural rather than aspirational: the kernel is English-only because
//! it holds no language at all, and any language with a pack gets the same
//! deterministic, offline treatment. A language with no pack is not an
//! error -- the floor declines, and the multilingual verdict path takes it.

use crate::lexicon::{self, Pack};
use chrono::{DateTime, Duration, Local, TimeZone};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "match", rename_all = "snake_case")]
pub enum FloorMatch {
    TimeNow,
    SelfMeta,
    Help,
    Remind { fire_at_ms: i64, about: String },
    ListReminders,
    CancelReminder,
    Remember { content: String },
    Recall { query: String },
    RegistryList,
    ForgetFact { index: usize },
    CorrectFact { index: usize, content: String },
    Invite,
    TelegramCode,
    WebSearch { query: String },
}

/// What the floor decided, plus the language it decided in -- so the
/// expression side can answer in the same one without guessing again.
#[derive(Debug, Clone, PartialEq)]
pub struct FloorHit {
    pub matched: FloorMatch,
    pub lang: String,
}

/// Scan one utterance in every candidate language. `now` is injected for
/// testability.
pub fn scan_lang(text: &str, now: DateTime<Local>) -> Option<FloorHit> {
    for pack in lexicon::candidates(text) {
        if let Some(m) = scan_with(pack, text, now) {
            return Some(FloorHit {
                matched: m,
                lang: pack.code.clone(),
            });
        }
    }
    None
}

/// Backwards-compatible entry point for callers that do not need the
/// language back.
pub fn scan(text: &str, now: DateTime<Local>) -> Option<FloorMatch> {
    scan_lang(text, now).map(|h| h.matched)
}

fn normalize(text: &str) -> (Vec<&str>, Vec<String>, String) {
    let original: Vec<&str> = text.split_whitespace().collect();
    let lower: Vec<String> = original
        .iter()
        .map(|t| {
            t.to_lowercase()
                .trim_matches(|c: char| ",.!?:;«»\"".contains(c))
                .to_string()
        })
        .collect();
    let joined = lower.join(" ");
    (original, lower, joined)
}

/// Try one language pack against one utterance.
pub fn scan_with(pack: &Pack, text: &str, now: DateTime<Local>) -> Option<FloorMatch> {
    let (original, lower, joined) = normalize(text);
    if original.is_empty() {
        return None;
    }

    // ---- 1. structured commands: an explicit head, then an argument ----
    // These run FIRST so a reminder whose subject mentions "reminders" is
    // still a reminder.

    if let Some(h) = pack.head_for("forget_fact", &joined) {
        return lower
            .get(h.skip)
            .and_then(|t| t.parse::<usize>().ok())
            .filter(|i| *i >= 1)
            .map(|index| FloorMatch::ForgetFact { index });
    }

    if let Some(h) = pack.head_for("correct_fact", &joined) {
        let index: usize = lower.get(h.skip)?.parse().ok()?;
        if index < 1 {
            return None;
        }
        let content = rest_of(&original, h.skip + 1, pack);
        return (!content.is_empty()).then_some(FloorMatch::CorrectFact { index, content });
    }

    if let Some(h) = pack.head_for("recall", &joined) {
        return Some(FloorMatch::Recall {
            query: rest_of(&original, h.skip, pack),
        });
    }

    if let Some(h) = pack.head_for("remember", &joined) {
        // "remember to X" is a timeless wish, ambiguous with a reminder --
        // the floor declines and the verdict decides.
        if pack
            .particles
            .filler
            .iter()
            .any(|f| lower.get(h.skip).map(String::as_str) == Some(f.as_str()))
            && pack.head_for("remind", &joined).is_none()
        {
            // only bail for the to-style filler, not for "that"-style
            if lower.get(h.skip).map(String::as_str) == Some("to") {
                return None;
            }
        }
        let content = rest_of(&original, h.skip, pack);
        return (!content.is_empty()).then_some(FloorMatch::Remember { content });
    }

    if let Some(m) = parse_reminder(pack, &lower, &original, &joined, now) {
        return Some(m);
    }

    if let Some(h) = pack.head_for("web_search", &joined) {
        let query = rest_of(&original, h.skip, pack);
        return (!query.is_empty()).then_some(FloorMatch::WebSearch { query });
    }

    // ---- 2. whole-utterance commands, matched exactly ----
    match pack.command_for(&joined)? {
        "time_now" => Some(FloorMatch::TimeNow),
        "self_meta" => Some(FloorMatch::SelfMeta),
        "help" => Some(FloorMatch::Help),
        "list_reminders" => Some(FloorMatch::ListReminders),
        "cancel_reminder" => Some(FloorMatch::CancelReminder),
        "registry_list" => Some(FloorMatch::RegistryList),
        "invite" => Some(FloorMatch::Invite),
        "telegram_code" => Some(FloorMatch::TelegramCode),
        _ => None,
    }
}

/// Remainder of the utterance in original casing, minus a leading filler
/// particle -- "to", "that" and their equivalents, listed per language in
/// each pack's `[particles] filler`.
fn rest_of(original: &[&str], mut idx: usize, pack: &Pack) -> String {
    if let Some(tok) = original.get(idx) {
        let t = tok.to_lowercase();
        let t = t.trim_matches(|c: char| ",.!?:;".contains(c));
        if pack.particles.filler.iter().any(|f| f == t) {
            idx += 1;
        }
    }
    original[idx.min(original.len())..]
        .join(" ")
        .trim_start_matches(':')
        .trim()
        .to_string()
}

fn parse_reminder(
    pack: &Pack,
    lower: &[String],
    original: &[&str],
    joined: &str,
    now: DateTime<Local>,
) -> Option<FloorMatch> {
    let head = pack.head_for("remind", joined)?;
    let mut i = head.skip;
    let p = &pack.particles;

    let mut tomorrow = false;
    if lower.get(i).map(|t| p.tomorrow.contains(t)) == Some(true) {
        tomorrow = true;
        i += 1;
    }

    let marker = lower.get(i)?.clone();

    // relative: "in 10 minutes X"
    if !tomorrow && p.relative.contains(&marker) {
        let n: i64 = lower.get(i + 1)?.parse().ok()?;
        if n < 1 {
            return None;
        }
        let unit = lower.get(i + 2)?;
        let minutes = if p.minutes.contains(unit) {
            n
        } else if p.hours.contains(unit) {
            n.checked_mul(60)?
        } else {
            return None;
        };
        // the horizon guard belongs on the RESOLVED duration, not the raw
        // number: "in 5000 hours" is 208 days, not 5000 minutes
        if minutes > 10080 {
            return None;
        }
        let about = rest_of(original, i + 3, pack);
        return (!about.is_empty()).then(|| FloorMatch::Remind {
            fire_at_ms: (now + Duration::minutes(minutes)).timestamp_millis(),
            about,
        });
    }

    // absolute: "at 18:30 X", optionally "tomorrow at 9 X"
    if p.absolute.contains(&marker) {
        let (h, m) = parse_hhmm(lower.get(i + 1)?)?;
        // calendar arithmetic, never +24h: adding an absolute day across a
        // DST boundary lands an hour off the wall-clock time asked for
        let mut day = now.date_naive();
        if tomorrow {
            day = day.succ_opt()?;
        }
        let mut target = local_at(day, h, m)?;
        if target <= now && !tomorrow {
            target = local_at(day.succ_opt()?, h, m)?;
        }
        if target <= now {
            return None;
        }
        let about = rest_of(original, i + 2, pack);
        return (!about.is_empty()).then(|| FloorMatch::Remind {
            fire_at_ms: target.timestamp_millis(),
            about,
        });
    }
    None
}

/// Local wall-clock time on a given day. During a spring-forward gap the
/// requested time does not exist; take the first instant after the gap
/// rather than dropping the reminder.
fn local_at(day: chrono::NaiveDate, h: u32, m: u32) -> Option<DateTime<Local>> {
    let naive = day.and_hms_opt(h, m, 0)?;
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) => Some(t),
        chrono::LocalResult::Ambiguous(earliest, _) => Some(earliest),
        chrono::LocalResult::None => (1..=8).find_map(|step| {
            Local
                .from_local_datetime(&(naive + Duration::minutes(15 * step)))
                .earliest()
        }),
    }
}

fn parse_hhmm(tok: &str) -> Option<(u32, u32)> {
    let (h, m) = match tok.split_once(':') {
        Some((h, m)) => (h.parse().ok()?, m.parse().ok()?),
        None => (tok.parse().ok()?, 0),
    };
    (h <= 23 && m <= 59).then_some((h, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed clock: 15 March 2026, 10:00 local. Tests must not depend on
    /// the wall clock -- absolute-reminder cases behave differently near
    /// midnight and across DST.
    fn now() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 3, 15, 10, 0, 0)
            .earliest()
            .expect("fixed test clock is a real local time")
    }

    /// The law, mechanically: no human-language phrase may appear in the
    /// kernel. If someone hard-codes one again, this fails.
    #[test]
    fn the_kernel_contains_no_surface_vocabulary() {
        let src = include_str!("floor.rs");
        // any Cyrillic, CJK, Arabic or Hebrew letter in kernel source means
        // a surface phrase leaked back in (test bodies excepted below)
        let body = src.split("#[cfg(test)]").next().unwrap();
        let leaked: Vec<char> = body
            .chars()
            .filter(|c| {
                let u = *c as u32;
                (0x0400..=0x04FF).contains(&u)      // Cyrillic
                    || (0x0590..=0x05FF).contains(&u) // Hebrew
                    || (0x0600..=0x06FF).contains(&u) // Arabic
                    || (0x3040..=0x30FF).contains(&u) // Kana
                    || (0x4E00..=0x9FFF).contains(&u) // CJK
            })
            .collect();
        assert!(
            leaked.is_empty(),
            "surface vocabulary leaked into the kernel: {leaked:?}"
        );
    }

    #[test]
    fn the_same_command_works_in_every_packed_language() {
        // english
        assert_eq!(scan("what time is it?", now()), Some(FloorMatch::TimeNow));
        assert_eq!(scan("my facts", now()), Some(FloorMatch::RegistryList));
        // russian -- via its pack, with no russian in this crate's kernel
        assert_eq!(scan("который час", now()), Some(FloorMatch::TimeNow));
        assert_eq!(scan("мои факты", now()), Some(FloorMatch::RegistryList));
        assert_eq!(scan("помощь", now()), Some(FloorMatch::Help));
    }

    /// The floor reports which language it matched, so the reply can be
    /// rendered in the same one without a second guess.
    #[test]
    fn the_floor_reports_the_language_it_matched() {
        assert_eq!(scan_lang("what time is it", now()).unwrap().lang, "en");
        assert_eq!(scan_lang("который час", now()).unwrap().lang, "ru");
    }

    #[test]
    fn structured_commands_work_in_every_packed_language() {
        for (text, expect) in [
            ("remember that i drink green tea", "i drink green tea"),
            ("запомни что я пью зелёный чай", "я пью зелёный чай"),
        ] {
            match scan(text, now()) {
                Some(FloorMatch::Remember { content }) => assert_eq!(content, expect, "{text}"),
                other => panic!("{text} -> {other:?}"),
            }
        }
        for text in [
            "remind me in 10 minutes to stretch",
            "напомни через 10 минут размяться",
            "remind me tomorrow at 9 to stretch",
            "напомни завтра в 10 проверить почту",
        ] {
            assert!(
                matches!(scan(text, now()), Some(FloorMatch::Remind { .. })),
                "{text}"
            );
        }
        for text in ["look up the weather in porto", "найди в интернете погоду"] {
            assert!(
                matches!(scan(text, now()), Some(FloorMatch::WebSearch { .. })),
                "{text}"
            );
        }
    }

    /// Regression, now across languages: a phrase command must not hijack a
    /// structured one.
    #[test]
    fn phrase_commands_do_not_hijack_structured_commands() {
        match scan("remind me at 18:00 to show reminders", now()) {
            Some(FloorMatch::Remind { about, .. }) => assert_eq!(about, "show reminders"),
            other => panic!("hijacked: {other:?}"),
        }
        match scan("напомни в 9 посмотреть мои напоминания", now()) {
            Some(FloorMatch::Remind { about, .. }) => {
                assert_eq!(about, "посмотреть мои напоминания")
            }
            other => panic!("hijacked: {other:?}"),
        }
        match scan("remember that my facts are private", now()) {
            Some(FloorMatch::Remember { content }) => {
                assert_eq!(content, "my facts are private")
            }
            other => panic!("hijacked: {other:?}"),
        }
    }

    /// THE POINT OF THE DESIGN: an unpacked language must not error. It
    /// declines the floor and the verdict path takes it.
    #[test]
    fn unpacked_languages_decline_cleanly() {
        for text in [
            "今何時ですか",              // japanese
            "كم الساعة الآن",            // arabic
            "現在幾點鐘",                 // chinese
            "मुझे 10 मिनट में याद दिलाओ", // hindi
            "지금 몇 시입니까",            // korean
            "τι ώρα είναι",              // greek
            "เวลาเท่าไหร่",                // thai
        ] {
            assert_eq!(scan(text, now()), None, "{text} should decline, not error");
        }
    }

    /// Latin-script languages without a pack also decline rather than being
    /// mis-parsed as English.
    #[test]
    fn unpacked_latin_languages_are_not_mistaken_for_english() {
        for text in [
            "¿qué hora es?",       // spanish
            "quelle heure est-il", // french
            "wie spät ist es",     // german
            "que horas são",       // portuguese
        ] {
            assert_eq!(scan(text, now()), None, "{text}");
        }
    }

    #[test]
    fn near_misses_and_guards_still_hold() {
        for text in [
            "can you show me my reminders please",
            "i wonder what time it is",
            "remind me in 5000 hours to x", // horizon guard on resolved duration
            "remember to buy milk",         // timeless wish, ambiguous
            "tell me a joke",
        ] {
            assert_eq!(scan(text, now()), None, "{text}");
        }
    }

    #[test]
    fn absolute_rollover_uses_calendar_days() {
        let n = now(); // 10:00
        match scan("remind me at 8 to stretch", n) {
            Some(FloorMatch::Remind { fire_at_ms, .. }) => {
                let fired = Local.timestamp_millis_opt(fire_at_ms).earliest().unwrap();
                assert_eq!(fired.date_naive(), n.date_naive().succ_opt().unwrap());
                assert_eq!(fired.format("%H:%M").to_string(), "08:00");
            }
            other => panic!("no match: {other:?}"),
        }
    }
}
