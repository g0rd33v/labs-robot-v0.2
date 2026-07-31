//! The bridge between a stored connection and a call to a provider.
//!
//! Its whole job is the order of operations. Both refreshing a token and
//! using it are network operations, and the cell's lock must be held across
//! neither — a mutex held for the length of a Google round trip stalls
//! every other turn for that person behind someone else's server. So:
//!
//! 1. under the lock: read the token, decide whether it is spent
//! 2. lock released: refresh over the network, if it was
//! 3. under the lock: write the fresh token back
//! 4. lock released: make the actual call
//!
//! The token exists as a value between those steps, which is why it is a
//! [`mind::connections::Secret`] rather than a `String`.

use hub::google::Google;
use hub::oauth::{self, App};
use mind::connections::{self, Access, Secret};
use prism::{Cell, PrismError};

pub const GOOGLE: &str = "google";

fn err(e: impl std::fmt::Display) -> PrismError {
    PrismError::Capability(e.to_string())
}

/// A usable access token, refreshed first if it was spent.
pub fn token(cell: &Cell, app: &App, g: &Google) -> Result<Secret, PrismError> {
    let now = trust::ids::ts_ms();
    let state = cell.with(|c| {
        connections::access(c, GOOGLE, now, oauth::REFRESH_MARGIN_MS).map_err(err)
    })?;
    match state {
        Access::Ready(t) => Ok(t),
        Access::NeedsRefresh(refresh_token) => {
            // no lock held here
            let form = oauth::refresh_form(app, refresh_token.expose());
            let fresh = g.exchange(&form).map_err(err)?;
            let token = Secret::new(fresh.access_token.clone());
            let expires_at = fresh.expires_at(now);
            cell.with(|c| {
                connections::store_refreshed(c, GOOGLE, &token, expires_at).map_err(err)
            })?;
            Ok(token)
        }
    }
}

/// Everything a capability needs to reach Google: the client, the app
/// registration, and the person's connection. Absent any of them, the
/// refusal names which one — "not connected" and "no client configured" are
/// different problems with different fixes, and collapsing them into one
/// message costs an hour of someone's evening.
pub struct Reach<'a> {
    pub google: &'a Google,
    pub app: &'a App,
}

impl Reach<'_> {
    /// A call, where failure is an ANSWER rather than an error.
    ///
    /// Connectors fail routinely — the token expired, the account is rate
    /// limited, the network is gone — and none of that is a bug in the
    /// robot. Propagating it as `Err` would abort the turn, leaving the
    /// intent open with no receipt for the watchdog to find and the person
    /// staring at nothing. So a failure comes back as `Err(Trouble)`, which
    /// the capability turns into a failed `Outcome` carrying a sentence
    /// that says what to do about it.
    pub fn call(
        &self,
        cell: &Cell,
        method: &str,
        url: &str,
        body: Option<&serde_json::Value>,
        purpose: &str,
    ) -> Result<serde_json::Value, Trouble> {
        let t = token(cell, self.app, self.google).map_err(|e| Trouble(e.to_string()))?;
        self.google
            .call(method, url, t.expose(), body, purpose)
            .map_err(|e| Trouble(e.to_string()))
    }
}

/// A provider said no. Carries the sentence `hub::google` already made
/// actionable ("reconnect it", "rate-limiting", "already be gone").
#[derive(Debug)]
pub struct Trouble(pub String);

impl std::fmt::Display for Trouble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The failed outcome for a connector that could not do its job.
pub fn stumbled(what: &'static str, t: Trouble) -> Result<prism::types::Outcome, PrismError> {
    crate::caps::failed(
        crate::caps::note_evidence(what),
        format!("{what} failed: {t}"),
        prism::types::Rendering::new("connector_failed", serde_json::json!({ "why": t.0 })),
    )
}
