//! The deterministic floor (decisions Q17): runs first and wins
//! unconditionally -- a deterministic match never yields to a model verdict.
//! The cheapest call is no call; this is also the offline floor.
//!
//! LANGUAGE. This file is English, and only English, on purpose. It is not
//! a language feature -- it is the fast path for the kernel's own language
//! and the only path that works with no network at all. Every other
//! language is understood by the routing call, which maps any phrasing onto
//! the same tools through their English descriptions (see
//! `robotd::caps`). That is why there are no phrase tables here for anyone
//! else: a table per language is a table to maintain per capability,
//! forever, and the count of supported languages should appear nowhere in
//! this codebase.
//!
//! The floor emits COMMAND STRUCTURE, never words. Nothing below produces a
//! sentence.

use chrono::{DateTime, Duration, Local, TimeZone};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "match", rename_all = "snake_case")]
pub enum FloorMatch {
    TimeNow,
    SelfMeta,
    Help,
    Remind { fire_at_ms: i64, about: String },
    /// R4.3.1: the time was vague, so the robot asks rather than guesses.
    /// The options are resolved instants; the person answers with a number.
    ClarifyTime {
        about: String,
        /// (label a person reads, the instant it means)
        options: Vec<(String, i64)>,
    },
    ListReminders,
    CancelReminder,
    Remember { content: String },
    Recall { query: String },
    RegistryList,
    ForgetFact { index: usize },
    CorrectFact { index: usize, content: String },
    Invite,
    TelegramCode,
    SoulShow,
    /// Begin linking an outside account. Deliberately floor-only: it mints
    /// a URL that grants standing access to someone's mail, and nothing a
    /// model can be talked into should be able to produce one.
    ConnectStart,
    ConnectStatus,
    RegistryShow,
    CommitmentList,
    InstructionList,
    WebSearch { query: String },
}

// ---------------------------------------------------------------------
// Whole-utterance commands. Matched EXACTLY, never as substrings: an
// utterance that merely contains one of these is not that command.
// ---------------------------------------------------------------------
const EXACT: [(&str, &str); 49] = [
    ("time", "time_now"),
    ("what time is it", "time_now"),
    ("what time is it now", "time_now"),
    ("what's the time", "time_now"),
    ("whats the time", "time_now"),
    ("current time", "time_now"),
    ("what day is it", "time_now"),
    ("what day is it today", "time_now"),
    ("what date is it", "time_now"),
    ("who are you", "self_meta"),
    ("what are you", "self_meta"),
    ("tell me about yourself", "self_meta"),
    ("help", "help"),
    ("/help", "help"),
    ("/start", "help"),
    ("commands", "help"),
    ("reminders", "list_reminders"),
    ("my reminders", "list_reminders"),
    ("list reminders", "list_reminders"),
    ("show reminders", "list_reminders"),
    ("show my reminders", "list_reminders"),
    ("cancel reminder", "cancel_reminder"),
    ("cancel the reminder", "cancel_reminder"),
    ("cancel last reminder", "cancel_reminder"),
    ("cancel the last reminder", "cancel_reminder"),
    ("registry", "registry_list"),
    ("my facts", "registry_list"),
    ("show my facts", "registry_list"),
    ("list facts", "registry_list"),
    ("invite", "invite"),
    ("new invite", "invite"),
    ("invite someone", "invite"),
    ("show facts", "registry_list"),
    ("telegram code", "telegram_code"),
    ("telegram", "telegram_code"),
    ("/registry", "registry_show"),
    ("/commitments", "commitment_list"),
    ("what are you waiting on", "commitment_list"),
    ("what did i ask you", "commitment_list"),
    ("my commitments", "commitment_list"),
    ("my rules", "instruction_list"),
    ("/rules", "instruction_list"),
    ("/connect", "connect_start"),
    ("/connections", "connect_status"),
    ("connected accounts", "connect_status"),
    ("what is connected", "connect_status"),
    ("/soul", "soul_show"),
    ("soul", "soul_show"),
    ("how are you set to speak", "soul_show"),
];

/// Heads that introduce an argument, longest first within each command so
/// "remind me" beats "remind". `usize` is how many whitespace tokens the
/// head occupies.
const REMEMBER: [(&str, usize); 2] = [("remember that", 2), ("remember", 1)];
const RECALL: [(&str, usize); 3] = [
    ("what do you remember about", 5),
    ("what do you remember", 4),
    ("do you remember", 3),
];
const REMIND: [(&str, usize); 2] = [("remind me", 2), ("remind", 1)];
const FORGET: [(&str, usize); 1] = [("forget fact", 2)];
const CORRECT: [(&str, usize); 1] = [("correct fact", 2)];
const SEARCH: [(&str, usize); 6] = [
    ("search the web for", 4),
    ("search the web", 3),
    ("search online for", 3),
    ("search for", 2),
    ("look up", 2),
    ("google", 1),
];

/// Dropped right after a head: "remind me to call mark" -> "call mark".
const FILLER: [&str; 4] = ["to", "that", "about", "for"];
const MINUTES: [&str; 4] = ["minute", "minutes", "min", "mins"];
const HOURS: [&str; 3] = ["hour", "hours", "h"];

/// Vague times, and the two or three hours a person actually means by them
/// (R4.3.1: *never guess silently*). English only, like the rest of the
/// floor -- other languages reach the same clarify through the model, and
/// the ANSWER is a number, which needs no language at all.
///
/// The windows are deliberately narrow and conventional. The point is not
/// to be right about what "evening" means to everyone; it is to make the
/// robot ask instead of picking, and to make answering one tap of work.
const VAGUE: [(&str, [u32; 3]); 8] = [
    ("morning", [8, 9, 10]),
    ("afternoon", [14, 15, 16]),
    ("evening", [18, 19, 20]),
    ("tonight", [20, 21, 22]),
    ("night", [20, 21, 22]),
    ("lunch", [12, 13, 13]),
    ("lunchtime", [12, 13, 13]),
    ("breakfast", [7, 8, 9]),
];

/// Longest matching head, or none.
fn head(table: &[(&str, usize)], joined: &str) -> Option<usize> {
    table
        .iter()
        .filter(|(phrase, _)| joined == *phrase || joined.starts_with(&format!("{phrase} ")))
        .max_by_key(|(phrase, _)| phrase.len())
        .map(|(_, skip)| *skip)
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

/// Scan one utterance. `now` is injected for testability.
pub fn scan(text: &str, now: DateTime<Local>) -> Option<FloorMatch> {
    let (original, lower, joined) = normalize(text);
    if original.is_empty() {
        return None;
    }

    // ---- 1. structured commands: an explicit head, then an argument ----
    // These run FIRST so a reminder whose subject mentions "reminders" is
    // still a reminder.

    if let Some(skip) = head(&FORGET, &joined) {
        return lower
            .get(skip)
            .and_then(|t| t.parse::<usize>().ok())
            .filter(|i| *i >= 1)
            .map(|index| FloorMatch::ForgetFact { index });
    }

    if let Some(skip) = head(&CORRECT, &joined) {
        let index: usize = lower.get(skip)?.parse().ok()?;
        if index < 1 {
            return None;
        }
        let content = rest_of(&original, skip + 1);
        return (!content.is_empty()).then_some(FloorMatch::CorrectFact { index, content });
    }

    if let Some(skip) = head(&RECALL, &joined) {
        return Some(FloorMatch::Recall {
            query: rest_of(&original, skip),
        });
    }

    if let Some(skip) = head(&REMEMBER, &joined) {
        // "remember to X" is a timeless wish, ambiguous with a reminder --
        // the floor declines and the routing call decides.
        if lower.get(skip).map(String::as_str) == Some("to") {
            return None;
        }
        let content = rest_of(&original, skip);
        return (!content.is_empty()).then_some(FloorMatch::Remember { content });
    }

    if let Some(m) = parse_reminder(&lower, &original, &joined, now) {
        return Some(m);
    }

    if let Some(skip) = head(&SEARCH, &joined) {
        let query = rest_of(&original, skip);
        return (!query.is_empty()).then_some(FloorMatch::WebSearch { query });
    }

    // ---- 2. whole-utterance commands, matched exactly ----
    let id = EXACT
        .iter()
        .find(|(phrase, _)| *phrase == joined)
        .map(|(_, id)| *id)?;
    match id {
        "time_now" => Some(FloorMatch::TimeNow),
        "self_meta" => Some(FloorMatch::SelfMeta),
        "help" => Some(FloorMatch::Help),
        "list_reminders" => Some(FloorMatch::ListReminders),
        "cancel_reminder" => Some(FloorMatch::CancelReminder),
        "registry_list" => Some(FloorMatch::RegistryList),
        "invite" => Some(FloorMatch::Invite),
        "telegram_code" => Some(FloorMatch::TelegramCode),
        "soul_show" => Some(FloorMatch::SoulShow),
        "connect_start" => Some(FloorMatch::ConnectStart),
        "connect_status" => Some(FloorMatch::ConnectStatus),
        "registry_show" => Some(FloorMatch::RegistryShow),
        "commitment_list" => Some(FloorMatch::CommitmentList),
        "instruction_list" => Some(FloorMatch::InstructionList),
        _ => None,
    }
}

/// Remainder of the utterance in original casing, minus a leading filler.
fn rest_of(original: &[&str], mut idx: usize) -> String {
    if let Some(tok) = original.get(idx) {
        let t = tok.to_lowercase();
        let t = t.trim_matches(|c: char| ",.!?:;".contains(c));
        if FILLER.contains(&t) {
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
    lower: &[String],
    original: &[&str],
    joined: &str,
    now: DateTime<Local>,
) -> Option<FloorMatch> {
    let skip = head(&REMIND, joined)?;
    let mut i = skip;

    let mut tomorrow = false;
    if lower.get(i).map(String::as_str) == Some("tomorrow") {
        tomorrow = true;
        i += 1;
    }

    // R4.3.1 FIRST: "in the morning" would otherwise die trying to parse
    // "the" as a number and return None through `?`, never reaching the
    // clarify below. A vague marker is unambiguous -- no exact form
    // contains one -- so testing it first costs nothing and misses nothing.
    if let Some(clarify) = vague_time(lower, original, i, now) {
        return Some(clarify);
    }

    let marker = lower.get(i)?.as_str();

    // relative: "in 10 minutes X"
    if !tomorrow && marker == "in" {
        let n: i64 = lower.get(i + 1)?.parse().ok()?;
        if n < 1 {
            return None;
        }
        let unit = lower.get(i + 2)?.as_str();
        let minutes = if MINUTES.contains(&unit) {
            n
        } else if HOURS.contains(&unit) {
            n.checked_mul(60)?
        } else {
            return None;
        };
        // the horizon guard belongs on the RESOLVED duration, not the raw
        // number: "in 5000 hours" is 208 days, not 5000 minutes
        if minutes > 10080 {
            return None;
        }
        let about = rest_of(original, i + 3);
        return (!about.is_empty()).then(|| FloorMatch::Remind {
            fire_at_ms: (now + Duration::minutes(minutes)).timestamp_millis(),
            about,
        });
    }

    // absolute: "at 18:30 X", optionally "tomorrow at 9 X"
    if marker == "at" {
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
        let about = rest_of(original, i + 2);
        return (!about.is_empty()).then(|| FloorMatch::Remind {
            fire_at_ms: target.timestamp_millis(),
            about,
        });
    }

    None
}

/// "in the morning", "this evening", "at lunch" -> a clarify with options.
fn vague_time(
    lower: &[String],
    original: &[&str],
    i: usize,
    now: DateTime<Local>,
) -> Option<FloorMatch> {
    // scan the remainder for a vague marker; the subject is everything else
    let (at, hours) = lower
        .iter()
        .enumerate()
        .skip(i)
        .find_map(|(k, w)| VAGUE.iter().find(|(v, _)| v == w).map(|(_, h)| (k, *h)))?;

    // The subject is THEIR WORDS (law 5), so only the time PHRASE comes
    // out -- the marker plus the contiguous run of filler immediately
    // before it. Filtering every occurrence of "the" turned "call the
    // bank" into "call bank", which is a different sentence and not one
    // they said.
    let lead: [&str; 6] = ["in", "the", "this", "at", "tomorrow", "later"];
    let mut cut_from = at;
    while cut_from > i {
        let prev = lower[cut_from - 1].trim_matches(',');
        if lead.contains(&prev) {
            cut_from -= 1;
        } else {
            break;
        }
    }
    let about: String = original
        .iter()
        .enumerate()
        .skip(i)
        .filter(|(k, _)| *k < cut_from || *k > at)
        .map(|(_, w)| *w)
        .collect::<Vec<_>>()
        .join(" ");
    let about = about
        .trim()
        .trim_start_matches("to ")
        .trim_end_matches(',')
        .trim()
        .to_string();
    if about.is_empty() {
        return None;
    }

    let tomorrow = lower.iter().skip(i).any(|w| w == "tomorrow");
    let mut options = vec![];
    for h in hours {
        let mut day = now.date_naive();
        if tomorrow {
            day = day.succ_opt()?;
        }
        let mut t = local_at(day, h, 0)?;
        // a window that has already passed today means tomorrow, not
        // "never" -- asking about a time in the past helps nobody
        if t <= now && !tomorrow {
            t = local_at(day.succ_opt()?, h, 0)?;
        }
        let label = t.format("%H:%M").to_string();
        if !options.iter().any(|(l, _)| *l == label) {
            options.push((label, t.timestamp_millis()));
        }
    }
    (!options.is_empty()).then_some(FloorMatch::ClarifyTime { about, options })
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

    fn now() -> DateTime<Local> {
        Local
            .from_local_datetime(
                &chrono::NaiveDate::from_ymd_opt(2026, 3, 10)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            )
            .earliest()
            .unwrap()
    }

    #[test]
    fn exact_commands_match_and_only_exactly() {
        assert_eq!(scan("what time is it", now()), Some(FloorMatch::TimeNow));
        assert_eq!(scan("What time is it?", now()), Some(FloorMatch::TimeNow));
        assert_eq!(scan("my facts", now()), Some(FloorMatch::RegistryList));
        assert_eq!(scan("help", now()), Some(FloorMatch::Help));
        // a sentence that merely CONTAINS a command is not that command
        assert_eq!(scan("i need help with my taxes", now()), None);
        assert_eq!(scan("what time is it in tokyo", now()), None);
    }

    #[test]
    fn structured_commands_take_their_argument() {
        assert_eq!(
            scan("remember that i drink green tea", now()),
            Some(FloorMatch::Remember {
                content: "i drink green tea".into()
            })
        );
        assert_eq!(
            scan("forget fact 2", now()),
            Some(FloorMatch::ForgetFact { index: 2 })
        );
        assert_eq!(
            scan("look up the weather in porto", now()),
            Some(FloorMatch::WebSearch {
                query: "the weather in porto".into()
            })
        );
    }

    /// Structured commands run before exact ones, so a reminder whose
    /// subject mentions "my reminders" is still a reminder.
    #[test]
    fn phrase_commands_do_not_hijack_structured_commands() {
        match scan("remind me at 9 to check my reminders", now()) {
            Some(FloorMatch::Remind { about, .. }) => assert_eq!(about, "check my reminders"),
            other => panic!("{other:?}"),
        }
    }

    /// R4.3.1: a vague time must ASK, with options -- never resolve to an
    /// hour the person did not choose.
    #[test]
    fn a_vague_time_asks_instead_of_guessing() {
        let now = Local.with_ymd_and_hms(2026, 8, 5, 6, 0, 0).unwrap();
        match scan("remind me in the morning to call the bank", now) {
            Some(FloorMatch::ClarifyTime { about, options }) => {
                assert_eq!(about, "call the bank", "their words, minus the time");
                assert_eq!(options.len(), 3, "two or three choices, not a lecture");
                assert_eq!(options[0].0, "08:00");
                assert!(options.iter().all(|(_, t)| *t > now.timestamp_millis()));
            }
            other => panic!("expected a clarify, got {other:?}"),
        }

        // an EXACT time is not vague and must not be interrupted
        assert!(matches!(
            scan("remind me at 18:30 to call the bank", now),
            Some(FloorMatch::Remind { .. })
        ));
        assert!(matches!(
            scan("remind me in 10 minutes to stretch", now),
            Some(FloorMatch::Remind { .. })
        ));
    }

    /// A window already past today means tomorrow -- asking about a time
    /// in the past helps nobody.
    #[test]
    fn a_passed_window_offers_tomorrow() {
        let evening = Local.with_ymd_and_hms(2026, 8, 5, 23, 0, 0).unwrap();
        match scan("remind me in the morning to take the bins out", evening) {
            Some(FloorMatch::ClarifyTime { options, .. }) => {
                assert!(
                    options.iter().all(|(_, t)| *t > evening.timestamp_millis()),
                    "every option must be in the future"
                );
            }
            other => panic!("expected a clarify, got {other:?}"),
        }
    }

    #[test]
    fn reminders_resolve_relative_and_absolute_times() {
        for text in [
            "remind me in 10 minutes to stretch",
            "remind me at 18:30 to call mark",
            "remind me tomorrow at 10 to check the post",
        ] {
            assert!(
                matches!(scan(text, now()), Some(FloorMatch::Remind { .. })),
                "{text}"
            );
        }
    }

    #[test]
    fn near_misses_and_guards_still_hold() {
        // horizon guard applies to the RESOLVED duration
        assert_eq!(scan("remind me in 5000 hours to blink", now()), None);
        assert_eq!(scan("remind me in 0 minutes to blink", now()), None);
        assert_eq!(scan("remind me at 99:99 to blink", now()), None);
        assert_eq!(scan("remind me in 10 minutes", now()), None);
        // a timeless wish is not a reminder; the routing call decides
        assert_eq!(scan("remember to call mum", now()), None);
    }

    /// The floor is English and declines everything else rather than
    /// guessing. Declining is not failing: the turn goes to the routing
    /// call, which reaches the same tools through their descriptions.
    #[test]
    fn other_languages_are_declined_not_guessed() {
        for text in [
            "который час",
            "напомни через 10 минут размяться",
            "今何時ですか",
            "كم الساعة",
            "¿qué hora es?",
            "wie spät ist es",
            "지금 몇 시야",
            "现在几点",
        ] {
            assert_eq!(scan(text, now()), None, "{text}");
        }
    }

    /// Law 4, mechanically: this file may contain no script but Latin, and
    /// the test module is the only place a foreign word may appear at all.
    #[test]
    fn the_kernel_holds_no_foreign_vocabulary() {
        let src = include_str!("floor.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap()];
        let stray: Vec<char> = code
            .chars()
            .filter(|c| {
                let u = *c as u32;
                (0x0400..=0x04FF).contains(&u)
                    || (0x0590..=0x08FF).contains(&u)
                    || (0x3040..=0x30FF).contains(&u)
                    || (0x4E00..=0x9FFF).contains(&u)
                    || (0xAC00..=0xD7AF).contains(&u)
            })
            .collect();
        assert!(stray.is_empty(), "foreign vocabulary in the floor: {stray:?}");
    }
}
