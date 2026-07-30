//! The deterministic floor (decisions Q17): runs first and wins
//! unconditionally -- a deterministic match never yields to a model verdict.
//! The cheapest call is no call; this is also the offline floor.
//!
//! V1 floor contents per Q17 + the M3 memory commands (Q28 memory.* set):
//! time/date, self/meta, help, explicit reminders with parseable time,
//! list/cancel, remember/recall, registry list, forget/correct fact.
//! English + Russian surface patterns (the floor scans surface text;
//! everything it *produces* is English-internal per arch sec 2d).

use chrono::{DateTime, Duration, Local, TimeZone};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "match", rename_all = "snake_case")]
pub enum FloorMatch {
    /// current time and date
    TimeNow,
    /// who/what are you
    SelfMeta,
    /// help / commands
    Help,
    /// explicit reminder with a parseable time
    Remind { fire_at_ms: i64, about: String },
    /// list active reminders
    ListReminders,
    /// cancel the most recent active reminder
    CancelReminder,
    /// store an owner-stated fact
    Remember { content: String },
    /// hybrid recall; empty query = recent facts
    Recall { query: String },
    /// registry-lite: every fact with its source
    RegistryList,
    /// erase fact N (1-based registry position) for real
    ForgetFact { index: usize },
    /// supersede fact N with new content
    CorrectFact { index: usize, content: String },
    /// mint a one-time member invite link (owner only)
    Invite,
    /// mint a telegram bind code (owner only)
    TelegramCode,
    /// an explicit request to look something up on the web
    WebSearch { query: String },
}

/// Whole-utterance commands. Matching is EXACT, never substring: an
/// utterance that merely *contains* one of these is not that command.
/// (Regression: "remind me at 18:00 to show reminders" used to list
/// reminders and never create the reminder, with a Verified receipt for the
/// wrong action.) Anything less than exact belongs to the verdict path --
/// Q17 makes the floor the high-signal deterministic set, not an NLU.
const TIME_FORMS: [&str; 12] = [
    "time",
    "what time is it",
    "what time is it now",
    "what's the time",
    "whats the time",
    "current time",
    "what day is it",
    "what day is it today",
    "what date is it",
    "который час",
    "сколько времени",
    "какое сегодня число",
];
const SELF_FORMS: [&str; 8] = [
    "who are you",
    "who are you?",
    "what are you",
    "tell me about yourself",
    "кто ты",
    "кто ты такой",
    "что ты такое",
    "расскажи о себе",
];
const LIST_REMINDER_FORMS: [&str; 8] = [
    "reminders",
    "my reminders",
    "list reminders",
    "show reminders",
    "show my reminders",
    "мои напоминания",
    "напоминания",
    "список напоминаний",
];
const CANCEL_REMINDER_FORMS: [&str; 6] = [
    "cancel reminder",
    "cancel the reminder",
    "cancel last reminder",
    "cancel the last reminder",
    "отмени напоминание",
    "отмена напоминания",
];
const REGISTRY_FORMS: [&str; 8] = [
    "registry",
    "my facts",
    "show facts",
    "show my facts",
    "list facts",
    "мои факты",
    "факты",
    "список фактов",
];
const HELP_FORMS: [&str; 6] = ["help", "/help", "commands", "/start", "помощь", "команды"];
const INVITE_FORMS: [&str; 5] = [
    "invite",
    "new invite",
    "invite someone",
    "пригласи",
    "новый инвайт",
];
const TELEGRAM_FORMS: [&str; 4] = ["telegram code", "telegram", "код телеграм", "телеграм код"];

fn is(joined: &str, forms: &[&str]) -> bool {
    forms.contains(&joined)
}

/// Scan one utterance. `now` is injected for testability.
///
/// Order matters: structured commands with an explicit head token
/// (remind/remember/correct/forget/recall) are parsed BEFORE whole-utterance
/// phrase commands, so a reminder whose subject mentions "reminders" is
/// still a reminder.
pub fn scan(text: &str, now: DateTime<Local>) -> Option<FloorMatch> {
    let original: Vec<&str> = text.split_whitespace().collect();
    if original.is_empty() {
        return None;
    }
    let lower: Vec<String> = original
        .iter()
        .map(|t| {
            // ':' and ';' are stripped too, so "search the web:" and
            // "remember:" normalize to the same head as their spaced forms
            t.to_lowercase()
                .trim_matches(|c: char| ",.!?:;".contains(c))
                .to_string()
        })
        .collect();
    let joined = lower.join(" ");

    // ---- 1. structured commands (explicit head token at position 0) ----

    // forget fact N
    if let Some(rest) = joined
        .strip_prefix("forget fact ")
        .or_else(|| joined.strip_prefix("забудь факт "))
    {
        return rest
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|i| *i >= 1)
            .map(|index| FloorMatch::ForgetFact { index });
    }
    if let Some(m) = parse_correct(&lower, &original) {
        return Some(m);
    }
    if let Some(m) = parse_recall(&lower, &original, &joined) {
        return Some(m);
    }
    if let Some(m) = parse_remember(&lower, &original) {
        return Some(m);
    }
    if let Some(m) = parse_reminder(&lower, &original, now) {
        return Some(m);
    }
    if let Some(m) = parse_web_search(&lower, &original, &joined) {
        return Some(m);
    }

    // ---- 2. whole-utterance phrase commands (exact match only) ----

    if is(&joined, &HELP_FORMS) {
        return Some(FloorMatch::Help);
    }
    if is(&joined, &TIME_FORMS) {
        return Some(FloorMatch::TimeNow);
    }
    if is(&joined, &SELF_FORMS) {
        return Some(FloorMatch::SelfMeta);
    }
    if is(&joined, &LIST_REMINDER_FORMS) {
        return Some(FloorMatch::ListReminders);
    }
    if is(&joined, &CANCEL_REMINDER_FORMS) {
        return Some(FloorMatch::CancelReminder);
    }
    if is(&joined, &REGISTRY_FORMS) {
        return Some(FloorMatch::RegistryList);
    }
    if is(&joined, &INVITE_FORMS) {
        return Some(FloorMatch::Invite);
    }
    if is(&joined, &TELEGRAM_FORMS) {
        return Some(FloorMatch::TelegramCode);
    }

    None
}

/// An explicit "look this up on the web" is high-signal and deterministic --
/// exactly what the floor is for (Q17). Leaving it to the verdict made
/// search unreliable in practice: the same class of question routed to
/// research one day and to a plain model answer the next, which replied
/// "I can't search the web" instead of searching.
fn parse_web_search(lower: &[String], original: &[&str], joined: &str) -> Option<FloorMatch> {
    const HEADS: [(&str, usize); 12] = [
        ("search the web for", 4),
        ("search the web", 3),
        ("search online for", 3),
        ("search for", 2),
        ("look up", 2),
        ("google", 1),
        ("найди в интернете", 3),
        ("поищи в интернете", 3),
        ("найди в сети", 3),
        ("погугли", 1),
        ("поищи", 1),
        ("найди", 1),
    ];
    for (head, tokens) in HEADS {
        let hit = joined == head || joined.starts_with(&format!("{head} "));
        if !hit {
            continue;
        }
        let mut idx = tokens;
        // strip a linking word left over from the longer forms
        if matches!(
            lower.get(idx).map(String::as_str),
            Some("for") | Some("about") | Some(":") | Some("про") | Some("о")
        ) {
            idx += 1;
        }
        let query = original[idx.min(original.len())..]
            .join(" ")
            .trim_start_matches(':')
            .trim()
            .to_string();
        if query.is_empty() {
            return None; // "google" alone is not a search request
        }
        return Some(FloorMatch::WebSearch { query });
    }
    None
}

fn parse_recall(lower: &[String], original: &[&str], joined: &str) -> Option<FloorMatch> {
    // "what do you remember [about X]" / "что ты помнишь [о X]"
    let heads: [(&str, usize); 4] = [
        ("what do you remember", 4),
        ("do you remember", 3),
        ("что ты помнишь", 3),
        ("вспомни", 1),
    ];
    for (head, tokens) in heads {
        if joined == head || joined.starts_with(&format!("{head} ")) {
            let mut idx = tokens;
            // strip a linking word: about / о / про
            if matches!(
                lower.get(idx).map(String::as_str),
                Some("about") | Some("о") | Some("об") | Some("про")
            ) {
                idx += 1;
            }
            let query = original[idx.min(original.len())..].join(" ");
            return Some(FloorMatch::Recall { query });
        }
    }
    None
}

fn parse_remember(lower: &[String], original: &[&str]) -> Option<FloorMatch> {
    let mut i = match lower.first().map(String::as_str) {
        Some("remember") => 1,
        Some("запомни") => 1,
        _ => return None,
    };
    // strip "that"/"что"/":"
    if matches!(
        lower.get(i).map(String::as_str),
        Some("that") | Some("что") | Some(":")
    ) {
        i += 1;
    }
    // "remember to X" reads as a timeless reminder wish, not a fact --
    // ambiguous, so the floor stays silent and the verdict path answers
    if lower.get(i).map(String::as_str) == Some("to") {
        return None;
    }
    let content = original[i.min(original.len())..].join(" ");
    let content = content.trim_start_matches(':').trim().to_string();
    if content.is_empty() {
        return None;
    }
    Some(FloorMatch::Remember { content })
}

fn parse_correct(lower: &[String], original: &[&str]) -> Option<FloorMatch> {
    let head = matches!(
        (
            lower.first().map(String::as_str),
            lower.get(1).map(String::as_str)
        ),
        (Some("correct"), Some("fact")) | (Some("исправь"), Some("факт"))
    );
    if !head {
        return None;
    }
    let num_tok = lower.get(2)?;
    let index: usize = num_tok.trim_end_matches(':').parse().ok()?;
    if index < 1 {
        return None;
    }
    let content = original[3.min(original.len())..].join(" ");
    let content = content.trim_start_matches(':').trim().to_string();
    if content.is_empty() {
        return None;
    }
    Some(FloorMatch::CorrectFact { index, content })
}

/// "remind me in 10 minutes to call mark" / "remind me [tomorrow] at 18:30 to X"
/// "напомни через 10 минут позвонить марку" / "напомни [завтра] в 18:30 X"
fn parse_reminder(
    lower: &[String],
    original: &[&str],
    now: DateTime<Local>,
) -> Option<FloorMatch> {
    // find the head: "remind [me]" (en) or "напомни [мне]" (ru)
    let second = lower.get(1).map(String::as_str);
    let mut i = match lower.first().map(String::as_str) {
        Some("remind") => {
            if second == Some("me") {
                2
            } else {
                1
            }
        }
        Some("напомни") => {
            if second == Some("мне") {
                2
            } else {
                1
            }
        }
        _ => return None,
    };

    let mut tomorrow = false;
    if matches!(
        lower.get(i).map(String::as_str),
        Some("tomorrow") | Some("завтра")
    ) {
        tomorrow = true;
        i += 1;
    }

    match lower.get(i).map(String::as_str) {
        // relative: in/через N unit rest
        Some("in") | Some("через") if !tomorrow => {
            let n: i64 = lower.get(i + 1)?.parse().ok()?;
            if n < 1 {
                return None;
            }
            let unit = lower.get(i + 2)?;
            let minutes = match unit.as_str() {
                "minute" | "minutes" | "min" | "mins" | "минуту" | "минуты" | "минут"
                | "мин" => n,
                "hour" | "hours" | "h" | "час" | "часа" | "часов" | "ч" => n.checked_mul(60)?,
                _ => return None,
            };
            // the horizon guard belongs on the RESOLVED duration, not on the
            // raw number ("in 5000 hours" is 208 days, not 5000 minutes)
            if minutes > 10080 {
                return None; // beyond a week is not high-signal floor territory
            }
            let about = about_from(original, i + 3);
            if about.is_empty() {
                return None;
            }
            let fire_at_ms = (now + Duration::minutes(minutes)).timestamp_millis();
            Some(FloorMatch::Remind { fire_at_ms, about })
        }
        // absolute: at/в HH[:MM] rest  (optionally preceded by tomorrow/завтра)
        Some("at") | Some("в") => {
            let (h, m) = parse_hhmm(lower.get(i + 1)?)?;
            // calendar arithmetic, never +24h: adding an absolute day across a
            // DST boundary lands an hour off the wall-clock time asked for
            let mut day = now.date_naive();
            if tomorrow {
                day = day.succ_opt()?;
            }
            let mut target = local_at(day, h, m)?;
            if target <= now && !tomorrow {
                // "at 8:00" said at 09:00 means tomorrow's 8:00
                target = local_at(day.succ_opt()?, h, m)?;
            }
            if target <= now {
                return None;
            }
            let about = about_from(original, i + 2);
            if about.is_empty() {
                return None;
            }
            Some(FloorMatch::Remind {
                fire_at_ms: target.timestamp_millis(),
                about,
            })
        }
        // "remind me tomorrow to X" (no time) is ambiguous -> not floor
        _ => None,
    }
}

/// Remainder of the utterance in its original casing; strips a leading
/// "to"/"that" (en) so "to call mark" stores as "call mark".
fn about_from(original: &[&str], mut idx: usize) -> String {
    if matches!(
        original.get(idx).map(|t| t.to_lowercase()),
        Some(ref t) if t == "to" || t == "that"
    ) {
        idx += 1;
    }
    original[idx.min(original.len())..].join(" ")
}

/// Local wall-clock time on a given day. During a spring-forward gap the
/// requested time does not exist; we take the first instant after the gap
/// rather than dropping the reminder on the floor.
fn local_at(day: chrono::NaiveDate, h: u32, m: u32) -> Option<DateTime<Local>> {
    let naive = day.and_hms_opt(h, m, 0)?;
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) => Some(t),
        chrono::LocalResult::Ambiguous(earliest, _) => Some(earliest),
        chrono::LocalResult::None => {
            // gap hour: walk forward in 15-minute steps to the first real instant
            (1..=8).find_map(|step| {
                let bumped = naive + Duration::minutes(15 * step);
                Local.from_local_datetime(&bumped).earliest()
            })
        }
    }
}

fn parse_hhmm(tok: &str) -> Option<(u32, u32)> {
    let (h, m) = match tok.split_once(':') {
        Some((h, m)) => (h.parse().ok()?, m.parse().ok()?),
        None => (tok.parse().ok()?, 0),
    };
    if h <= 23 && m <= 59 {
        Some((h, m))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed, unremarkable clock: 15 March 2026, 10:00 local. Tests must
    /// not depend on the wall clock -- the absolute-reminder cases behave
    /// differently near midnight and across DST transitions.
    fn now() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 3, 15, 10, 0, 0)
            .earliest()
            .expect("fixed test clock is a real local time")
    }

    /// Regression: phrase commands were matched with `contains`, so any
    /// utterance mentioning them was hijacked -- the real command was
    /// silently dropped and the receipt honestly recorded the WRONG action.
    #[test]
    fn phrase_commands_do_not_hijack_structured_commands() {
        // a reminder whose subject mentions reminders is still a reminder
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
        // a fact that mentions facts is still a fact
        match scan("remember that my facts are private", now()) {
            Some(FloorMatch::Remember { content }) => {
                assert_eq!(content, "my facts are private")
            }
            other => panic!("hijacked: {other:?}"),
        }
        // "what date" inside a reminder subject must not become TimeNow
        match scan("remind me at 9 to check what date the demo is", now()) {
            Some(FloorMatch::Remind { about, .. }) => {
                assert_eq!(about, "check what date the demo is")
            }
            other => panic!("hijacked: {other:?}"),
        }
        // and a fact mentioning the time question stays a fact
        match scan("remember that i always ask what time it is", now()) {
            Some(FloorMatch::Remember { .. }) => {}
            other => panic!("hijacked: {other:?}"),
        }
    }

    /// Conversational phrasings are NOT floor commands -- they belong to the
    /// verdict path (Q17: the floor is the high-signal set, not an NLU).
    /// Explicit search requests are deterministic, not left to the verdict.
    #[test]
    fn explicit_web_search_is_a_floor_command() {
        for (text, expect) in [
            ("search the web for rust 1.97 features", "rust 1.97 features"),
            ("search the web: rust 1.97 features", "rust 1.97 features"),
            ("search for the best coffee in lisbon", "the best coffee in lisbon"),
            ("look up the weather in porto", "the weather in porto"),
            ("google rust release notes", "rust release notes"),
            ("найди в интернете новости про rust", "новости про rust"),
            ("погугли погоду в лиссабоне", "погоду в лиссабоне"),
        ] {
            match scan(text, now()) {
                Some(FloorMatch::WebSearch { query }) => assert_eq!(query, expect, "{text}"),
                other => panic!("{text} -> {other:?}"),
            }
        }
        // a bare verb is not a search request
        assert_eq!(scan("google", now()), None);
        // and a reminder that mentions searching is still a reminder
        assert!(matches!(
            scan("remind me at 9 to google the answer", now()),
            Some(FloorMatch::Remind { .. })
        ));
    }

    #[test]
    fn near_miss_phrasings_fall_through_to_the_verdict() {
        for text in [
            "can you show me my reminders please",
            "i wonder what time it is",
            "tell me about my facts and where they came from",
        ] {
            assert_eq!(scan(text, now()), None, "{text}");
        }
    }

    #[test]
    fn relative_horizon_guard_applies_to_resolved_duration() {
        // 5000 hours is 208 days -- must not pass as "within a week"
        assert_eq!(scan("remind me in 5000 hours to x", now()), None);
        // but a legitimate hour-scale reminder still works
        assert!(matches!(
            scan("remind me in 48 hours to x", now()),
            Some(FloorMatch::Remind { .. })
        ));
        // exactly a week is the boundary, just inside
        assert!(matches!(
            scan("remind me in 168 hours to x", now()),
            Some(FloorMatch::Remind { .. })
        ));
        assert_eq!(scan("remind me in 169 hours to x", now()), None);
    }

    /// "at 8:00" said at 10:00 rolls to tomorrow's 8:00 by CALENDAR, not by
    /// adding 24 absolute hours (which lands an hour off across DST).
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
        // and "tomorrow at 9" is tomorrow's 09:00 on the wall clock
        match scan("remind me tomorrow at 9 to stretch", n) {
            Some(FloorMatch::Remind { fire_at_ms, .. }) => {
                let fired = Local.timestamp_millis_opt(fire_at_ms).earliest().unwrap();
                assert_eq!(fired.format("%H:%M").to_string(), "09:00");
            }
            other => panic!("no match: {other:?}"),
        }
    }

    #[test]
    fn floor_matches_time_self_help_in_both_languages() {
        assert_eq!(scan("What time is it?", now()), Some(FloorMatch::TimeNow));
        assert_eq!(scan("который час", now()), Some(FloorMatch::TimeNow));
        assert_eq!(scan("who are you?", now()), Some(FloorMatch::SelfMeta));
        assert_eq!(scan("кто ты", now()), Some(FloorMatch::SelfMeta));
        assert_eq!(scan("help", now()), Some(FloorMatch::Help));
        assert_eq!(scan("помощь", now()), Some(FloorMatch::Help));
        assert_eq!(scan("my reminders", now()), Some(FloorMatch::ListReminders));
        assert_eq!(
            scan("cancel reminder", now()),
            Some(FloorMatch::CancelReminder)
        );
    }

    #[test]
    fn floor_parses_relative_reminders() {
        let n = now();
        match scan("remind me in 10 minutes to call Mark", n) {
            Some(FloorMatch::Remind { fire_at_ms, about }) => {
                assert_eq!(about, "call Mark");
                let delta = fire_at_ms - n.timestamp_millis();
                assert!((9 * 60_000..=11 * 60_000).contains(&delta));
            }
            other => panic!("no match: {other:?}"),
        }
        match scan("напомни через 2 часа позвонить маме", n) {
            Some(FloorMatch::Remind { fire_at_ms, about }) => {
                assert_eq!(about, "позвонить маме");
                let delta = fire_at_ms - n.timestamp_millis();
                assert!((119 * 60_000..=121 * 60_000).contains(&delta));
            }
            other => panic!("no match: {other:?}"),
        }
    }

    #[test]
    fn floor_parses_absolute_reminders_in_the_future() {
        let n = now();
        for text in [
            "remind me at 23:59 to wrap up",
            "remind me tomorrow at 9 to stretch",
            "напомни завтра в 10 проверить почту",
        ] {
            match scan(text, n) {
                Some(FloorMatch::Remind { fire_at_ms, about }) => {
                    assert!(fire_at_ms > n.timestamp_millis(), "{text}");
                    assert!(!about.is_empty(), "{text}");
                }
                other => panic!("no match for {text}: {other:?}"),
            }
        }
    }

    #[test]
    fn floor_parses_memory_commands() {
        assert_eq!(
            scan("remember that I drink green tea", now()),
            Some(FloorMatch::Remember {
                content: "I drink green tea".into()
            })
        );
        assert_eq!(
            scan("запомни что я пью зелёный чай", now()),
            Some(FloorMatch::Remember {
                content: "я пью зелёный чай".into()
            })
        );
        assert_eq!(
            scan("what do you remember about tea?", now()),
            Some(FloorMatch::Recall {
                query: "tea?".into()
            })
        );
        assert_eq!(
            scan("what do you remember", now()),
            Some(FloorMatch::Recall { query: "".into() })
        );
        assert_eq!(
            scan("что ты помнишь о чае", now()),
            Some(FloorMatch::Recall {
                query: "чае".into()
            })
        );
        assert_eq!(scan("my facts", now()), Some(FloorMatch::RegistryList));
        assert_eq!(scan("мои факты", now()), Some(FloorMatch::RegistryList));
        assert_eq!(
            scan("forget fact 2", now()),
            Some(FloorMatch::ForgetFact { index: 2 })
        );
        assert_eq!(
            scan("забудь факт 1", now()),
            Some(FloorMatch::ForgetFact { index: 1 })
        );
        assert_eq!(
            scan("correct fact 1: I live in Lisbon", now()),
            Some(FloorMatch::CorrectFact {
                index: 1,
                content: "I live in Lisbon".into()
            })
        );
        assert_eq!(
            scan("исправь факт 2: я живу в Лиссабоне", now()),
            Some(FloorMatch::CorrectFact {
                index: 2,
                content: "я живу в Лиссабоне".into()
            })
        );
    }

    #[test]
    fn floor_stays_silent_on_everything_else() {
        for text in [
            "tell me a joke",
            "remind me to call mark",           // no parseable time -> not floor
            "remind me in 99999 minutes to x",  // beyond the week guard
            "what is the meaning of life",
            "напомни позвонить маме",           // no time -> not floor
            "remember to buy milk",             // timeless wish, ambiguous -> not floor
            "forget fact zero",                 // unparseable index
        ] {
            assert_eq!(scan(text, now()), None, "{text}");
        }
    }
}
