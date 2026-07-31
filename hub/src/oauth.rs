//! OAuth 2.0 authorization code + PKCE (Q29), for native connectors.
//!
//! *"OAuth: standard authorization-code with PKCE; for self-hosted, loopback
//! redirect to the embedded server (the desktop-app pattern)... Tokens →
//! vault; scopes minimal."*
//!
//! Why PKCE matters here specifically: a robot on someone's machine is a
//! **public client**. Its client secret ships inside a binary the owner
//! holds, so it is not a secret, and the authorization code is the only
//! thing standing between an attacker and the person's mailbox. PKCE binds
//! the code to a verifier that never left this process — an intercepted
//! code redeemed without it is worthless.
//!
//! Three properties this module is responsible for, each of which is a real
//! attack when absent:
//!
//! * **The verifier is generated per attempt and never logged.** It is the
//!   proof of possession; a reused or recorded one is a reusable code.
//! * **`state` is single-use and compared in full.** It is the only defence
//!   against a code injected by another site — the callback arrives over
//!   plain loopback HTTP where anything on the machine may reach it.
//! * **The redirect URI is loopback with a fixed path.** Google matches it
//!   exactly; so do we, on the way back in.

use crate::HubError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

pub const GOOGLE_AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";

/// A person has a minute to finish a consent screen, not an afternoon. An
/// attempt that outlives its usefulness is a `state` value still waiting to
/// be matched.
pub const ATTEMPT_TTL_MS: i64 = 10 * 60_000;

/// Refresh this far before the token actually expires, so a call never
/// races the clock it just checked.
pub const REFRESH_MARGIN_MS: i64 = 60_000;

/// The PKCE pair (RFC 7636). `verifier` is the secret; `challenge` is what
/// travels.
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    /// 96 random bytes -> 128 base64url characters, the top of RFC 7636's
    /// permitted range. The verifier's only job is to be unguessable.
    pub fn generate() -> Pkce {
        let mut raw = [0u8; 96];
        trust::ids::fill_random(&mut raw);
        let verifier = URL_SAFE_NO_PAD.encode(raw);
        Pkce::from_verifier(verifier)
    }

    pub fn from_verifier(verifier: String) -> Pkce {
        let challenge = URL_SAFE_NO_PAD.encode(trust::ids::sha256(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }
}

/// One authorization attempt, held until its callback arrives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub state: String,
    pub verifier: String,
    pub provider: String,
    pub scopes: Vec<String>,
    pub principal: i64,
    pub started_at: i64,
}

impl Attempt {
    pub fn is_fresh(&self, now: i64) -> bool {
        now.saturating_sub(self.started_at) < ATTEMPT_TTL_MS
    }
}

/// The client registration for one provider. The secret is a *credential*
/// (§7): it comes from the environment, and nothing here ever puts it in a
/// log line or a model context.
#[derive(Debug, Clone)]
pub struct App {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
}

impl App {
    /// Google's own guidance for installed apps: loopback IP, arbitrary
    /// port, fixed path. `localhost` is deliberately not used -- it can
    /// resolve to something other than the interface we are listening on.
    pub fn loopback(client_id: String, client_secret: Option<String>, port: u16) -> App {
        App {
            client_id,
            client_secret,
            redirect_uri: format!("http://127.0.0.1:{port}/oauth/google/callback"),
        }
    }
}

fn esc(s: &str) -> String {
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

/// Begin an attempt: returns the URL to send the person to, and the state
/// to keep until they come back.
pub fn begin(app: &App, provider: &str, scopes: &[String], principal: i64) -> (String, Attempt) {
    let pkce = Pkce::generate();
    let attempt = Attempt {
        state: trust::ids::random_hex(24),
        verifier: pkce.verifier,
        provider: provider.into(),
        scopes: scopes.to_vec(),
        principal,
        started_at: trust::ids::ts_ms(),
    };
    let url = format!(
        "{GOOGLE_AUTH}?client_id={}&redirect_uri={}&response_type=code\
         &scope={}&code_challenge={}&code_challenge_method=S256&state={}\
         &access_type=offline&prompt=consent",
        esc(&app.client_id),
        esc(&app.redirect_uri),
        esc(&scopes.join(" ")),
        esc(&pkce.challenge),
        esc(&attempt.state),
    );
    (url, attempt)
}

/// What a provider hands back. `refresh_token` is absent on a re-consent
/// that Google decides is redundant, which is why `access_type=offline` and
/// `prompt=consent` are both set above -- without a refresh token the
/// connection dies in an hour and the person has to reconnect.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
}

impl TokenResponse {
    pub fn expires_at(&self, now: i64) -> i64 {
        now + self.expires_in.unwrap_or(3600) * 1000
    }
}

/// The form body for redeeming a code. Separated from the HTTP call so the
/// shape can be tested without a network -- getting `grant_type` or the
/// verifier field name wrong fails identically to having no credentials,
/// and only one of those is findable offline.
pub fn code_exchange_form(app: &App, attempt: &Attempt, code: &str) -> String {
    let mut form = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        esc(code),
        esc(&app.redirect_uri),
        esc(&app.client_id),
        esc(&attempt.verifier),
    );
    if let Some(secret) = &app.client_secret {
        form.push_str(&format!("&client_secret={}", esc(secret)));
    }
    form
}

pub fn refresh_form(app: &App, refresh_token: &str) -> String {
    let mut form = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}",
        esc(refresh_token),
        esc(&app.client_id),
    );
    if let Some(secret) = &app.client_secret {
        form.push_str(&format!("&client_secret={}", esc(secret)));
    }
    form
}

/// Match a callback against the attempt that started it.
///
/// Full comparison, not a prefix: a `state` check that accepts a prefix
/// accepts an attacker who can guess one character at a time.
pub fn check_callback(attempt: &Attempt, state: &str, now: i64) -> Result<(), HubError> {
    if !attempt.is_fresh(now) {
        return Err(HubError::Gateway(
            "that sign-in attempt expired -- start it again".into(),
        ));
    }
    if state.len() != attempt.state.len() || state != attempt.state {
        return Err(HubError::Gateway(
            "sign-in state did not match; refusing the callback".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636's own worked example. If this drifts, every real exchange
    /// fails with an opaque `invalid_grant` and no local test would say why.
    #[test]
    fn the_pkce_challenge_matches_the_rfc_vector() {
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    /// The verifier is the whole proof of possession. Two attempts sharing
    /// one would make an intercepted code replayable.
    #[test]
    fn every_attempt_gets_its_own_verifier_and_state() {
        let app = App::loopback("cid".into(), None, 7777);
        let (_, a) = begin(&app, "google", &["s".into()], 1);
        let (_, b) = begin(&app, "google", &["s".into()], 1);
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.state, b.state);
        assert!(a.verifier.len() >= 43 && a.verifier.len() <= 128, "RFC 7636 range");
    }

    #[test]
    fn the_authorization_url_carries_what_google_requires() {
        let app = App::loopback("cid.apps.googleusercontent.com".into(), None, 7777);
        let scopes = vec![
            "https://www.googleapis.com/auth/calendar.events".to_string(),
            "https://www.googleapis.com/auth/gmail.readonly".to_string(),
        ];
        let (url, attempt) = begin(&app, "google", &scopes, 1);
        assert!(url.starts_with(GOOGLE_AUTH));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("access_type=offline"), "or there is no refresh token");
        assert!(url.contains(&format!("state={}", attempt.state)));
        // scopes are space-joined and escaped, not comma-joined
        assert!(url.contains("calendar.events%20https"), "{url}");
        // the verifier must never appear in the URL -- only its hash does
        assert!(!url.contains(&attempt.verifier));
        // loopback IP, not the name
        assert!(url.contains("127.0.0.1"));
    }

    #[test]
    fn the_exchange_form_sends_the_verifier_and_never_the_challenge() {
        let app = App::loopback("cid".into(), Some("sec".into()), 7777);
        let (_, attempt) = begin(&app, "google", &[], 1);
        let form = code_exchange_form(&app, &attempt, "4/code with space");
        assert!(form.contains("grant_type=authorization_code"));
        assert!(form.contains(&format!("code_verifier={}", esc(&attempt.verifier))));
        assert!(form.contains("code=4%2Fcode%20with%20space"), "escaped: {form}");
        assert!(form.contains("client_secret=sec"));
        assert!(!form.contains("code_challenge"));
    }

    /// A callback is the one moment an outsider can speak to us. Both the
    /// window and the full-value comparison are load-bearing.
    #[test]
    fn a_callback_is_refused_unless_it_matches_exactly_and_in_time() {
        let app = App::loopback("cid".into(), None, 7777);
        let (_, a) = begin(&app, "google", &[], 1);
        let now = a.started_at;

        assert!(check_callback(&a, &a.state, now).is_ok());
        assert!(check_callback(&a, "", now).is_err(), "empty state");
        assert!(
            check_callback(&a, &a.state[..a.state.len() - 1], now).is_err(),
            "a prefix is not a match"
        );
        assert!(
            check_callback(&a, &format!("{}x", a.state), now).is_err(),
            "nor is an extension"
        );
        assert!(
            check_callback(&a, &a.state, now + ATTEMPT_TTL_MS + 1).is_err(),
            "an attempt does not wait forever"
        );
    }

    #[test]
    fn an_expiry_is_computed_from_the_response_with_a_sane_default() {
        let t: TokenResponse = serde_json::from_str(
            r#"{"access_token":"a","expires_in":3599,"token_type":"Bearer"}"#,
        )
        .unwrap();
        assert_eq!(t.expires_at(1_000), 1_000 + 3_599_000);
        assert!(t.refresh_token.is_none());

        // a response with no expiry still gets one -- "never expires" is the
        // assumption that leaves a dead token in place forever
        let t: TokenResponse = serde_json::from_str(r#"{"access_token":"a"}"#).unwrap();
        assert_eq!(t.expires_at(0), 3_600_000);
    }
}
