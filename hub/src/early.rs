//! Early decision from a streamed router (§2c #2, done without guessing).
//!
//! The routing call is the wall in front of every non-floor reply:
//! measured p50 ~3.0 s, against an answer whose own time-to-first-token is
//! ~350 ms. Waiting for the whole verdict object before starting the
//! answer costs the person three seconds of nothing.
//!
//! But the answer path needs exactly one field — *which tool, if any* —
//! and `output_shape` now emits `call.tool` first. So this watches the
//! stream as it arrives and reports the decision the moment it is
//! unambiguous, while `mood`, `confidence` and the rest are still being
//! written. The answer starts then.
//!
//! **Nothing here is speculative.** The decision is read, not predicted:
//! if the model has said `"tool": "none"`, no later token can change that,
//! because the field is written once. That is the whole reason the shape
//! change was worth making — a guess would have to be retracted, and a
//! robot that retracts answers is worse than a slow one.

/// What the router has committed to so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Early {
    /// `"tool": "none"` — the answer path, and it can start now.
    NoTool,
    /// A tool was named. Its arguments are still arriving, so the caller
    /// must wait for the full object; knowing the name early is still
    /// worth reporting for tracing and for future per-tool prefetch.
    Tool(String),
}

/// Scan a partial routing response for the tool decision.
///
/// Deliberately a string scan rather than an incremental JSON parser: the
/// prefix of a JSON document is not a JSON document, and every tolerant
/// parser in this file exists because models emit shapes that are *nearly*
/// right. What we need is narrower than parsing — one field, whose value
/// is a bare string — and a scan for it is honest about that.
///
/// Returns `None` while the field has not yet arrived complete. A value is
/// only reported once its CLOSING quote is present, so a truncated
/// `"tool": "calendar.li` is never read as a tool named `calendar.li`.
pub fn decision(partial: &str) -> Option<Early> {
    let key = partial.find("\"tool\"")?;
    let after = &partial[key + 6..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let mut chars = rest.char_indices();
    // the value must be a string; anything else is a shape we do not read
    if chars.next()?.1 != '"' {
        return None;
    }
    let start = 1;
    let mut end = None;
    let mut escaped = false;
    for (i, ch) in rest[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => {
                end = Some(start + i);
                break;
            }
            _ => {}
        }
    }
    let value = &rest[start..end?];
    if value == crate::verdicts::NO_TOOL {
        Some(Early::NoTool)
    } else if value.is_empty() {
        None
    } else {
        Some(Early::Tool(value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision is readable from a prefix, and only once it is whole.
    #[test]
    fn a_decision_is_read_only_when_it_is_complete() {
        // nothing yet
        assert_eq!(decision(""), None);
        assert_eq!(decision("{\"call\": {"), None);
        assert_eq!(decision("{\"call\": {\"tool\""), None);
        assert_eq!(decision("{\"call\": {\"tool\": "), None);
        assert_eq!(decision("{\"call\": {\"tool\": \""), None);
        // TRUNCATED mid-value must not be read as a shorter tool name --
        // starting an answer for `calendar.li` would be acting on a
        // decision nobody made
        assert_eq!(decision("{\"call\": {\"tool\": \"calendar.li"), None);

        // complete: the answer path can start here, with the rest of the
        // object still unwritten
        assert_eq!(
            decision("{\"call\": {\"tool\": \"none\", \"args\": {"),
            Some(Early::NoTool)
        );
        assert_eq!(
            decision("{\"call\": {\"tool\": \"calendar.list\", \"args\": {\"from\""),
            Some(Early::Tool("calendar.list".into()))
        );
    }

    /// Whitespace and formatting vary by provider; the scan must not.
    #[test]
    fn formatting_does_not_change_the_decision() {
        for shape in [
            "{\"call\":{\"tool\":\"none\"}}",
            "{\n  \"call\": {\n    \"tool\"  :   \"none\",\n",
            "{\"call\": {\"args\": {}, \"tool\": \"none\"}}",
        ] {
            assert_eq!(decision(shape), Some(Early::NoTool), "{shape}");
        }
    }

    /// An escaped quote inside a value must not end it early.
    #[test]
    fn an_escaped_quote_does_not_truncate_the_value() {
        assert_eq!(
            decision(r#"{"call": {"tool": "we\"ird", "args": {}"#),
            Some(Early::Tool("we\\\"ird".into()))
        );
    }

    /// A non-string value is a shape we do not read, not a guess.
    #[test]
    fn a_non_string_value_is_declined() {
        assert_eq!(decision("{\"call\": {\"tool\": null"), None);
        assert_eq!(decision("{\"call\": {\"tool\": 42"), None);
    }
}
