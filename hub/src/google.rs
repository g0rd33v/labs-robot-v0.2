//! Google Calendar and Gmail (Q29), native rather than via MCP — *"native
//! gives us token scoping and receipts we fully control"*.
//!
//! Two halves, deliberately separated:
//!
//! * **Shaping** — pure functions that build a URL, a body, an RFC 2822
//!   message. These carry every mistake that is cheap to make and expensive
//!   to diagnose: a wrong field name comes back as a 400 that looks exactly
//!   like a credential problem. They are testable with no network and no
//!   account, and they are where this module's tests live.
//! * **Transport** — one struct that adds the bearer token, logs the
//!   crossing, and parses the reply.
//!
//! Everything inbound is **untrusted-by-origin** (§7a). An email body is
//! written by whoever sent it, and it lands in model context; text arriving
//! here is data about a message, never instruction. The boundary log tags
//! it accordingly and the capability layer keeps it in a content field.

use crate::gateway::BoundarySink;
use crate::HubError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use trust::boundary::{self, Crossing, Direction};

pub const CAL_BASE: &str = "https://www.googleapis.com/calendar/v3";
pub const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1";

/// Minimal scopes (Q29). `gmail.send` is absent on purpose: it is requested
/// only when the owner turns sending on, so a robot that has never been
/// asked to send mail cannot send mail even if something convinces it to
/// try.
pub const SCOPE_CALENDAR: &str = "https://www.googleapis.com/auth/calendar.events";
pub const SCOPE_MAIL_READ: &str = "https://www.googleapis.com/auth/gmail.readonly";
pub const SCOPE_MAIL_COMPOSE: &str = "https://www.googleapis.com/auth/gmail.compose";
pub const SCOPE_MAIL_SEND: &str = "https://www.googleapis.com/auth/gmail.send";
pub const SCOPE_EMAIL: &str = "https://www.googleapis.com/auth/userinfo.email";

pub fn base_scopes() -> Vec<String> {
    vec![
        SCOPE_EMAIL.into(),
        SCOPE_CALENDAR.into(),
        SCOPE_MAIL_READ.into(),
        SCOPE_MAIL_COMPOSE.into(),
    ]
}

// ------------------------------------------------------------- shaping

fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// One event, in the shape a person cares about rather than the shape
/// Google returns.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: String,
    pub title: String,
    /// RFC 3339. All-day events carry a date; both are passed through as
    /// given rather than normalised, because normalising an all-day event to
    /// midnight in some timezone is how "your birthday" moves a day.
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub all_day: bool,
}

/// The window query for listing. `singleEvents` expands recurrences, which
/// is what makes "what's on Tuesday" answerable at all.
pub fn events_url(time_min: &str, time_max: &str, max: usize) -> String {
    format!(
        "{CAL_BASE}/calendars/primary/events?timeMin={}&timeMax={}\
         &singleEvents=true&orderBy=startTime&maxResults={max}",
        q(time_min),
        q(time_max)
    )
}

pub fn event_url(id: &str) -> String {
    format!("{CAL_BASE}/calendars/primary/events/{}", q(id))
}

/// Build the body for a create or update.
///
/// `send_updates=none` is NOT set: if an event has attendees, they are
/// people who need to know. But note that this makes creating an event with
/// attendees an outward-facing act -- which is why the capability that
/// calls it declares an irreversible effect.
pub fn event_body(
    title: &str,
    start: &str,
    end: &str,
    tz: &str,
    location: Option<&str>,
    attendees: &[String],
) -> serde_json::Value {
    let all_day = !start.contains('T');
    let when = |v: &str| {
        if all_day {
            serde_json::json!({ "date": v })
        } else {
            serde_json::json!({ "dateTime": v, "timeZone": tz })
        }
    };
    let mut body = serde_json::json!({
        "summary": title,
        "start": when(start),
        "end": when(end),
    });
    if let Some(l) = location {
        body["location"] = serde_json::json!(l);
    }
    if !attendees.is_empty() {
        body["attendees"] =
            serde_json::json!(attendees.iter().map(|e| serde_json::json!({ "email": e })).collect::<Vec<_>>());
    }
    body
}

/// Google's event JSON -> ours. Tolerant by design: a missing field on one
/// event must not lose the other nineteen.
pub fn parse_event(v: &serde_json::Value) -> Event {
    let pick = |k: &str| -> (String, bool) {
        let node = &v[k];
        match node["dateTime"].as_str() {
            Some(dt) => (dt.to_string(), false),
            None => (node["date"].as_str().unwrap_or_default().to_string(), true),
        }
    };
    let (start, all_day) = pick("start");
    let (end, _) = pick("end");
    Event {
        id: v["id"].as_str().unwrap_or_default().to_string(),
        title: v["summary"].as_str().unwrap_or("(no title)").to_string(),
        start,
        end,
        location: v["location"].as_str().map(String::from),
        attendees: v["attendees"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x["email"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        all_day,
    }
}

/// One message, headers and a plain-text body.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Mail {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    pub snippet: String,
    pub body: String,
}

pub fn search_url(query: &str, max: usize) -> String {
    format!(
        "{GMAIL_BASE}/users/me/messages?q={}&maxResults={max}",
        q(query)
    )
}

pub fn message_url(id: &str) -> String {
    format!("{GMAIL_BASE}/users/me/messages/{}?format=full", q(id))
}

fn header(payload: &serde_json::Value, name: &str) -> String {
    payload["headers"]
        .as_array()
        .and_then(|hs| {
            hs.iter()
                .find(|h| {
                    h["name"]
                        .as_str()
                        .map(|n| n.eq_ignore_ascii_case(name))
                        .unwrap_or(false)
                })
                .and_then(|h| h["value"].as_str())
        })
        .unwrap_or_default()
        .to_string()
}

/// Walk the MIME tree for something a person can read.
///
/// Prefers `text/plain`. Falls back to `text/html` only if there is nothing
/// else, because HTML in model context is mostly markup, and markup is
/// where instructions hide.
fn body_text(payload: &serde_json::Value) -> String {
    fn decode(part: &serde_json::Value) -> Option<String> {
        let data = part["body"]["data"].as_str()?;
        let bytes = URL_SAFE_NO_PAD.decode(data.replace('-', "+").replace('_', "/")).ok()
            .or_else(|| URL_SAFE_NO_PAD.decode(data).ok())?;
        Some(String::from_utf8_lossy(&bytes).to_string())
    }
    fn walk(part: &serde_json::Value, want: &str) -> Option<String> {
        if part["mimeType"].as_str() == Some(want) {
            if let Some(t) = decode(part) {
                return Some(t);
            }
        }
        part["parts"]
            .as_array()?
            .iter()
            .find_map(|p| walk(p, want))
    }
    walk(payload, "text/plain")
        .or_else(|| walk(payload, "text/html"))
        .unwrap_or_default()
}

pub fn parse_message(v: &serde_json::Value) -> Mail {
    let payload = &v["payload"];
    Mail {
        id: v["id"].as_str().unwrap_or_default().to_string(),
        thread_id: v["threadId"].as_str().unwrap_or_default().to_string(),
        from: header(payload, "From"),
        to: header(payload, "To"),
        subject: header(payload, "Subject"),
        date: header(payload, "Date"),
        snippet: v["snippet"].as_str().unwrap_or_default().to_string(),
        body: body_text(payload),
    }
}

/// An outgoing message as RFC 2822, base64url-encoded the way Gmail wants.
///
/// Header injection is the risk: a newline inside a subject or a recipient
/// turns one header into two, and the second can be `Bcc`. Both are
/// stripped of CR and LF before they are placed -- there is no legitimate
/// newline in either field.
pub fn compose_raw(to: &str, subject: &str, body: &str, in_reply_to: Option<&str>) -> String {
    let clean = |s: &str| s.replace(['\r', '\n'], " ");
    let mut msg = format!(
        "To: {}\r\nSubject: {}\r\nContent-Type: text/plain; charset=utf-8\r\n",
        clean(to),
        clean(subject)
    );
    if let Some(rt) = in_reply_to {
        msg.push_str(&format!(
            "In-Reply-To: {}\r\nReferences: {}\r\n",
            clean(rt),
            clean(rt)
        ));
    }
    msg.push_str("\r\n");
    msg.push_str(body);
    URL_SAFE_NO_PAD.encode(msg)
}

// ----------------------------------------------------------- transport

pub struct Google {
    agent: ureq::Agent,
    boundary: Option<BoundarySink>,
}

impl Google {
    pub fn new(boundary: Option<BoundarySink>) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_millis(4000))
                .timeout(Duration::from_millis(20_000))
                .build(),
            boundary,
        }
    }

    /// Law #3: a failed log write aborts the crossing. The payload is
    /// hashed, never stored -- the log proves what crossed and when, and is
    /// not a copy of the person's mail.
    fn log(&self, direction: Direction, purpose: &str, payload: &[u8]) -> Result<(), HubError> {
        let Some(sink) = &self.boundary else {
            return Ok(());
        };
        let conn = sink
            .lock()
            .map_err(|_| HubError::Gateway("boundary log unavailable (poisoned)".into()))?;
        boundary::append(
            &conn,
            &Crossing {
                direction,
                channel: "connector".into(),
                counterparty: "google".into(),
                purpose: purpose.into(),
                categories: "owner-data".into(),
                payload_hash: trust::ids::sha256_hex(payload),
                size: payload.len() as i64,
                trust_tag: if direction == Direction::Out {
                    "granted".into()
                } else {
                    // inbound mail and event text is written by other people
                    "untrusted".into()
                },
            },
        )
        .map_err(|e| HubError::Gateway(format!("boundary log write failed: {e}")))?;
        Ok(())
    }

    /// Redeem a code or a refresh token at the token endpoint.
    ///
    /// The form carries the client secret and the PKCE verifier, so the
    /// boundary log records the crossing by hash only -- as it does for
    /// everything, and here it matters more than anywhere else.
    pub fn exchange(&self, form: &str) -> Result<crate::oauth::TokenResponse, HubError> {
        self.log(Direction::Out, "oauth-token", form.as_bytes())?;
        let resp = self
            .agent
            .post(crate::oauth::GOOGLE_TOKEN)
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(form);
        let text = match resp {
            Ok(r) => r.into_string().map_err(|e| HubError::Gateway(e.to_string()))?,
            Err(ureq::Error::Status(code, r)) => {
                let detail = r.into_string().unwrap_or_default();
                self.log(Direction::In, "oauth-token", detail.as_bytes())?;
                return Err(HubError::Gateway(google_error(code, &detail)));
            }
            Err(e) => return Err(HubError::Gateway(e.to_string())),
        };
        // logged by hash: the body IS a pair of tokens
        self.log(Direction::In, "oauth-token", text.as_bytes())?;
        serde_json::from_str(&text)
            .map_err(|e| HubError::Gateway(format!("google token response: {e}")))
    }

    /// Which account this token belongs to. Asked once at connect time so
    /// the person can see whose mailbox is attached.
    pub fn whoami(&self, token: &str) -> Result<String, HubError> {
        let v = self.call(
            "GET",
            "https://www.googleapis.com/oauth2/v2/userinfo",
            token,
            None,
            "userinfo",
        )?;
        Ok(v["email"].as_str().unwrap_or("unknown").to_string())
    }

    /// One authenticated call. `token` is borrowed for the duration and
    /// never stored, logged, or returned.
    pub fn call(
        &self,
        method: &str,
        url: &str,
        token: &str,
        body: Option<&serde_json::Value>,
        purpose: &str,
    ) -> Result<serde_json::Value, HubError> {
        let sent = body.map(|b| b.to_string()).unwrap_or_default();
        self.log(Direction::Out, purpose, sent.as_bytes())?;

        let req = self
            .agent
            .request(method, url)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/json");
        let resp = match body {
            Some(b) => req.send_json(b.clone()),
            None => req.call(),
        };

        let text = match resp {
            Ok(r) => r.into_string().map_err(|e| HubError::Gateway(e.to_string()))?,
            Err(ureq::Error::Status(code, r)) => {
                let detail = r.into_string().unwrap_or_default();
                self.log(Direction::In, purpose, detail.as_bytes())?;
                return Err(HubError::Gateway(google_error(code, &detail)));
            }
            Err(e) => return Err(HubError::Gateway(e.to_string())),
        };
        self.log(Direction::In, purpose, text.as_bytes())?;
        if text.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| HubError::Gateway(format!("google: {e}")))
    }
}

/// Turn Google's error shape into something a person can act on. A raw 403
/// is indistinguishable from a bug; "the calendar scope was not granted" is
/// a thing they can fix.
pub fn google_error(code: u16, detail: &str) -> String {
    let parsed: serde_json::Value = serde_json::from_str(detail).unwrap_or_default();
    let msg = parsed["error"]["message"]
        .as_str()
        .or_else(|| parsed["error_description"].as_str())
        .unwrap_or("")
        .to_string();
    match code {
        401 => "google rejected the credentials -- the connection needs renewing".into(),
        403 if msg.contains("insufficient") || msg.contains("scope") => {
            "google refused: this account did not grant that permission -- reconnect \
             and approve it"
                .into()
        }
        403 => format!("google refused: {msg}"),
        404 => "google could not find that -- it may already be gone".into(),
        429 => "google is rate-limiting this account; try again shortly".into(),
        _ if msg.is_empty() => format!("google returned {code}"),
        _ => format!("google returned {code}: {msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_window_expands_recurrences_and_escapes_its_bounds() {
        let url = events_url("2026-08-01T00:00:00+02:00", "2026-08-02T00:00:00+02:00", 20);
        assert!(url.starts_with(CAL_BASE));
        assert!(url.contains("singleEvents=true"), "or recurring events are invisible");
        assert!(url.contains("orderBy=startTime"));
        // the `+` in an offset must not arrive as a space
        assert!(url.contains("%2B02%3A00"), "{url}");
        assert!(!url.contains("+02:00"));
    }

    /// A timed event and an all-day event are different shapes to Google,
    /// and sending the wrong one moves the event by hours.
    #[test]
    fn an_all_day_event_is_a_date_and_a_timed_one_is_a_datetime() {
        let timed = event_body(
            "standup",
            "2026-08-01T09:00:00+02:00",
            "2026-08-01T09:15:00+02:00",
            "Europe/Berlin",
            None,
            &[],
        );
        assert_eq!(timed["start"]["dateTime"], "2026-08-01T09:00:00+02:00");
        assert_eq!(timed["start"]["timeZone"], "Europe/Berlin");
        assert!(timed["start"]["date"].is_null());

        let allday = event_body("birthday", "2026-08-01", "2026-08-02", "Europe/Berlin", None, &[]);
        assert_eq!(allday["start"]["date"], "2026-08-01");
        assert!(allday["start"]["dateTime"].is_null(), "no timezone to drift in");

        let with_people = event_body(
            "lunch",
            "2026-08-01T12:00:00Z",
            "2026-08-01T13:00:00Z",
            "UTC",
            Some("the usual place"),
            &["a@b.com".into()],
        );
        assert_eq!(with_people["attendees"][0]["email"], "a@b.com");
        assert_eq!(with_people["location"], "the usual place");
    }

    #[test]
    fn google_events_parse_back_including_the_all_day_case() {
        let v = serde_json::json!({
            "id": "abc",
            "summary": "standup",
            "start": {"dateTime": "2026-08-01T09:00:00+02:00"},
            "end": {"dateTime": "2026-08-01T09:15:00+02:00"},
            "attendees": [{"email": "a@b.com"}, {"displayName": "no email"}]
        });
        let e = parse_event(&v);
        assert_eq!(e.title, "standup");
        assert_eq!(e.attendees, vec!["a@b.com"]);
        assert!(!e.all_day);

        let v = serde_json::json!({
            "id": "d", "start": {"date": "2026-08-01"}, "end": {"date": "2026-08-02"}
        });
        let e = parse_event(&v);
        assert!(e.all_day);
        assert_eq!(e.title, "(no title)", "a missing field loses one field, not the event");
    }

    /// Header injection: a newline in a recipient or subject turns one
    /// header into two, and the second one can be `Bcc`.
    #[test]
    fn a_newline_can_never_smuggle_a_header_into_an_outgoing_message() {
        let raw = compose_raw(
            "a@b.com\r\nBcc: attacker@evil.com",
            "hello\r\nX-Injected: yes",
            "the body",
            None,
        );
        let decoded = String::from_utf8(URL_SAFE_NO_PAD.decode(&raw).unwrap()).unwrap();
        // the text survives -- flattened onto its own header's line, where
        // it is inert. What must not happen is a NEW header line.
        let head = decoded.split("\r\n\r\n").next().unwrap();
        for line in head.split("\r\n") {
            assert!(
                !line.starts_with("Bcc:") && !line.starts_with("X-Injected:"),
                "a smuggled header line: {line:?}"
            );
        }
        assert_eq!(head.split("\r\n").count(), 3, "exactly the headers we wrote");
        // the body is the person's words and keeps its own newlines
        let raw = compose_raw("a@b.com", "hi", "line one\nline two", None);
        let decoded = String::from_utf8(URL_SAFE_NO_PAD.decode(&raw).unwrap()).unwrap();
        assert!(decoded.ends_with("line one\nline two"));
        assert!(decoded.contains("\r\n\r\n"), "headers end before the body");
    }

    #[test]
    fn a_reply_carries_the_threading_headers() {
        let raw = compose_raw("a@b.com", "Re: x", "sure", Some("<msg-1@mail>"));
        let decoded = String::from_utf8(URL_SAFE_NO_PAD.decode(&raw).unwrap()).unwrap();
        assert!(decoded.contains("In-Reply-To: <msg-1@mail>"));
        assert!(decoded.contains("References: <msg-1@mail>"));
    }

    #[test]
    fn a_message_parses_headers_and_prefers_plain_text_over_html() {
        let enc = |s: &str| URL_SAFE_NO_PAD.encode(s);
        let v = serde_json::json!({
            "id": "m1", "threadId": "t1", "snippet": "a preview",
            "payload": {
                "headers": [
                    {"name": "from", "value": "Sender <s@x.com>"},
                    {"name": "Subject", "value": "the subject"},
                    {"name": "Date", "value": "Fri, 31 Jul 2026 10:00:00 +0200"}
                ],
                "parts": [
                    {"mimeType": "text/html", "body": {"data": enc("<p>markup</p>")}},
                    {"mimeType": "text/plain", "body": {"data": enc("the readable body")}}
                ]
            }
        });
        let m = parse_message(&v);
        assert_eq!(m.from, "Sender <s@x.com>", "header lookup is case-insensitive");
        assert_eq!(m.subject, "the subject");
        assert_eq!(m.body, "the readable body", "plain text wins over markup");

        // html only: taken, because some body beats none
        let v = serde_json::json!({
            "id": "m2",
            "payload": {"mimeType": "text/html", "body": {"data": enc("<p>only this</p>")}}
        });
        assert!(parse_message(&v).body.contains("only this"));
    }

    /// A bare status code is indistinguishable from a bug. These are the
    /// three a person actually hits, and each has a different fix.
    #[test]
    fn google_errors_say_what_to_do_about_them() {
        let scope = r#"{"error":{"message":"Request had insufficient authentication scopes"}}"#;
        assert!(google_error(403, scope).contains("reconnect"));
        assert!(google_error(401, "{}").contains("renewing"));
        assert!(google_error(404, "{}").contains("already be gone"));
        assert!(google_error(429, "{}").contains("rate-limiting"));
        assert!(google_error(500, "{}").contains("500"));
    }
}
