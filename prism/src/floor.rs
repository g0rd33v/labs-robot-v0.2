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
}

/// Scan one utterance. `now` is injected for testability.
pub fn scan(text: &str, now: DateTime<Local>) -> Option<FloorMatch> {
    let original: Vec<&str> = text.split_whitespace().collect();
    if original.is_empty() {
        return None;
    }
    let lower: Vec<String> = original
        .iter()
        .map(|t| {
            t.to_lowercase()
                .trim_matches(|c: char| ",.!?".contains(c))
                .to_string()
        })
        .collect();
    let joined = lower.join(" ");

    // help
    if matches!(
        joined.as_str(),
        "help" | "/help" | "commands" | "/start" | "помощь" | "команды"
    ) {
        return Some(FloorMatch::Help);
    }

    // time / date
    const TIME_PATTERNS: [&str; 8] = [
        "what time",
        "what's the time",
        "current time",
        "what date",
        "what day is it",
        "который час",
        "сколько времени",
        "какое сегодня число",
    ];
    if joined == "time" || TIME_PATTERNS.iter().any(|p| joined.contains(p)) {
        return Some(FloorMatch::TimeNow);
    }

    // memory: recall (before self/meta -- "что ты помнишь" must not be eaten)
    if let Some(m) = parse_recall(&lower, &original, &joined) {
        return Some(m);
    }

    // self / meta
    const SELF_PATTERNS: [&str; 4] = ["who are you", "what are you", "кто ты", "что ты такое"];
    if SELF_PATTERNS.iter().any(|p| joined.contains(p)) {
        return Some(FloorMatch::SelfMeta);
    }

    // list reminders
    const LIST_PATTERNS: [&str; 5] = [
        "my reminders",
        "list reminders",
        "show reminders",
        "мои напоминания",
        "список напоминаний",
    ];
    if joined == "reminders" || LIST_PATTERNS.iter().any(|p| joined.contains(p)) {
        return Some(FloorMatch::ListReminders);
    }

    // cancel reminder
    if joined.starts_with("cancel reminder")
        || joined.starts_with("cancel the reminder")
        || joined.starts_with("отмени напоминание")
        || joined.starts_with("отмена напоминания")
    {
        return Some(FloorMatch::CancelReminder);
    }

    // registry list
    const REGISTRY_PATTERNS: [&str; 5] = [
        "my facts",
        "show facts",
        "list facts",
        "мои факты",
        "список фактов",
    ];
    if joined == "registry" || REGISTRY_PATTERNS.iter().any(|p| joined.contains(p)) {
        return Some(FloorMatch::RegistryList);
    }

    // forget fact N
    if let Some(rest) = joined
        .strip_prefix("forget fact ")
        .or_else(|| joined.strip_prefix("забудь факт "))
    {
        if let Ok(index) = rest.trim().parse::<usize>() {
            if index >= 1 {
                return Some(FloorMatch::ForgetFact { index });
            }
        }
        return None;
    }

    // correct fact N: new content
    if let Some(m) = parse_correct(&lower, &original) {
        return Some(m);
    }

    // remember: store an owner-stated fact
    if let Some(m) = parse_remember(&lower, &original) {
        return Some(m);
    }

    // explicit reminders
    parse_reminder(&lower, &original, now)
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
            if !(1..=10080).contains(&n) {
                return None; // beyond a week is not high-signal floor territory
            }
            let unit = lower.get(i + 2)?;
            let minutes = match unit.as_str() {
                "minute" | "minutes" | "min" | "mins" | "минуту" | "минуты" | "минут"
                | "мин" => n,
                "hour" | "hours" | "h" | "час" | "часа" | "часов" | "ч" => n * 60,
                _ => return None,
            };
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
            let base = now.date_naive() + Duration::days(if tomorrow { 1 } else { 0 });
            let naive = base.and_hms_opt(h, m, 0)?;
            let mut target = Local.from_local_datetime(&naive).earliest()?;
            if target <= now && !tomorrow {
                target += Duration::days(1); // "at 8:00" said at 9:00 means tomorrow
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

    fn now() -> DateTime<Local> {
        Local::now()
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
