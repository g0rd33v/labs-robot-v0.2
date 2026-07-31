//! Connected accounts and their tokens (Q29: *"Tokens → vault"*).
//!
//! These rows are the highest-value thing in the cell: a refresh token is
//! standing access to someone's mailbox, renewable indefinitely, and unlike
//! a password it was never memorised so its loss is invisible. Three rules,
//! enforced here rather than remembered elsewhere:
//!
//! * **They live in the cell**, which is encrypted at rest under the cell's
//!   own DEK. There is no plaintext-on-disk path.
//! * **They never travel.** `export` in [`crate::merge`] does not name this
//!   table. A synced token would put standing access on a USB stick.
//! * **They never reach a model.** A token leaves here only as a [`Secret`],
//!   which has a redacted `Debug`, no `Display`, and no `Serialize` — so it
//!   cannot fall into a log line, a `Rendering`, or a model context by
//!   accident. Reading the real value takes a call to [`Secret::expose`].
//!
//! Disconnecting deletes the row. There is no soft delete for a credential:
//! a token that is merely marked revoked is a token still on the disk.

use crate::MindError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// A connected account, WITHOUT its secrets. This is the shape anything
/// outside this module is allowed to see -- which is why the tokens are not
/// fields on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connected {
    pub provider: String,
    pub account: String,
    pub scopes: Vec<String>,
    pub expires_at: i64,
    pub connected_at: i64,
}

impl Connected {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// Store or replace the connection for a provider. Re-connecting keeps the
/// existing refresh token when the provider did not send a new one --
/// Google omits it on a re-consent it considers redundant, and overwriting
/// with NULL there would silently end the connection in an hour.
#[allow(clippy::too_many_arguments)]
pub fn save(
    conn: &Connection,
    provider: &str,
    account: &str,
    scopes: &[String],
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at: i64,
) -> Result<Connected, MindError> {
    let now = trust::ids::ts_ms();
    conn.execute(
        "INSERT INTO connections(provider, account, scopes, access_token, \
                                 refresh_token, expires_at, connected_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7) \
         ON CONFLICT(provider) DO UPDATE SET \
           account = excluded.account, \
           scopes = excluded.scopes, \
           access_token = excluded.access_token, \
           refresh_token = COALESCE(excluded.refresh_token, connections.refresh_token), \
           expires_at = excluded.expires_at, \
           updated_at = excluded.updated_at",
        params![
            provider,
            account,
            scopes.join(" "),
            access_token,
            refresh_token,
            expires_at,
            now
        ],
    )?;
    get(conn, provider)?.ok_or_else(|| MindError::Vault("connection missing after save".into()))
}

pub fn get(conn: &Connection, provider: &str) -> Result<Option<Connected>, MindError> {
    Ok(conn
        .query_row(
            "SELECT provider, account, scopes, expires_at, connected_at \
             FROM connections WHERE provider = ?1",
            params![provider],
            |r| {
                Ok(Connected {
                    provider: r.get(0)?,
                    account: r.get(1)?,
                    scopes: r
                        .get::<_, String>(2)?
                        .split_whitespace()
                        .map(String::from)
                        .collect(),
                    expires_at: r.get(3)?,
                    connected_at: r.get(4)?,
                })
            },
        )
        .optional()?)
}

pub fn list(conn: &Connection) -> Result<Vec<Connected>, MindError> {
    let mut stmt = conn.prepare("SELECT provider FROM connections ORDER BY provider")?;
    let providers: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![];
    for p in providers {
        if let Some(c) = get(conn, &p)? {
            out.push(c);
        }
    }
    Ok(out)
}

/// Forget an account. A hard delete: a credential marked revoked is a
/// credential still on the disk.
pub fn disconnect(conn: &Connection, provider: &str) -> Result<bool, MindError> {
    Ok(conn.execute("DELETE FROM connections WHERE provider = ?1", params![provider])? > 0)
}

/// A token, in a wrapper that cannot be printed, logged, or serialised.
///
/// The token has to become a value: refreshing and calling are both network
/// operations, and doing either while holding the cell's lock would block
/// every other turn for that person behind a remote server. So instead of
/// keeping the secret inside a closure, we make the secret a type that
/// resists escaping — `Debug` is redacted, `Display` is not implemented,
/// `Serialize` is not derived, and reading the real value takes a call to
/// [`Secret::expose`] that is visible in review.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Secret {
        Secret(s.into())
    }
    /// Deliberately named. Every use is a place to ask "does this value
    /// reach a log, a model, or a rendering?"
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(redacted)")
    }
}

/// What a caller must do before it can make a call.
#[derive(Debug, Clone)]
pub enum Access {
    /// Usable now.
    Ready(Secret),
    /// Spent. Refresh with this, then call [`store_refreshed`].
    NeedsRefresh(Secret),
}

/// Read the access token, deciding whether it must be refreshed first.
///
/// Returns rather than calling, so the cell's lock is released before any
/// network operation. The two-step shape is the point: this function is
/// fast and holds the lock, the network is slow and does not.
pub fn access(
    conn: &Connection,
    provider: &str,
    now: i64,
    margin_ms: i64,
) -> Result<Access, MindError> {
    let row: Option<(String, Option<String>, i64)> = conn
        .query_row(
            "SELECT access_token, refresh_token, expires_at FROM connections \
             WHERE provider = ?1",
            params![provider],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((token, refresh_token, expires_at)) = row else {
        return Err(MindError::Vault(format!("{provider} is not connected")));
    };
    if now + margin_ms < expires_at {
        return Ok(Access::Ready(Secret(token)));
    }
    // spent. Without a refresh token there is nothing to do but say so --
    // silently using an expired token would surface as an opaque 401.
    match refresh_token {
        Some(rt) => Ok(Access::NeedsRefresh(Secret(rt))),
        None => Err(MindError::Vault(format!(
            "the {provider} connection expired and has no refresh token -- reconnect it"
        ))),
    }
}

/// Write back a token obtained by refreshing.
pub fn store_refreshed(
    conn: &Connection,
    provider: &str,
    token: &Secret,
    expires_at: i64,
) -> Result<(), MindError> {
    conn.execute(
        "UPDATE connections SET access_token = ?2, expires_at = ?3, updated_at = ?4 \
         WHERE provider = ?1",
        params![provider, token.expose(), expires_at, trust::ids::ts_ms()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::init_cell_schema(&conn).unwrap();
        conn
    }

    fn scopes() -> Vec<String> {
        vec!["calendar.events".into(), "gmail.readonly".into()]
    }

    #[test]
    fn a_connection_round_trips_without_exposing_its_tokens() {
        let c = cell();
        let saved = save(
            &c,
            "google",
            "a@b.com",
            &scopes(),
            "ya29-SECRET-ACCESS",
            Some("1//SECRET-REFRESH"),
            9_000,
        )
        .unwrap();
        assert_eq!(saved.account, "a@b.com");
        assert!(saved.has_scope("gmail.readonly"));
        assert!(!saved.has_scope("gmail.send"));

        // the visible shape has no field that could carry a secret
        let json = serde_json::to_string(&saved).unwrap();
        assert!(
            !json.contains("SECRET"),
            "neither token may appear in the visible record: {json}"
        );
        assert_eq!(list(&c).unwrap().len(), 1);
    }

    /// Google omits the refresh token on a re-consent it thinks is
    /// redundant. Overwriting with nothing would end the connection in an
    /// hour, and it would look like a server problem.
    #[test]
    fn re_connecting_without_a_new_refresh_token_keeps_the_old_one() {
        let c = cell();
        save(&c, "google", "a@b.com", &scopes(), "at1", Some("rt1"), 1_000).unwrap();
        save(&c, "google", "a@b.com", &scopes(), "at2", None, 2_000).unwrap();

        let kept: Option<String> = c
            .query_row(
                "SELECT refresh_token FROM connections WHERE provider = 'google'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept.as_deref(), Some("rt1"));
        assert_eq!(list(&c).unwrap().len(), 1, "re-connect replaces, not appends");
    }

    #[test]
    fn a_live_token_is_handed_over_and_a_spent_one_asks_to_be_refreshed_first() {
        let c = cell();
        save(&c, "google", "a@b.com", &scopes(), "live", Some("rt"), 100_000).unwrap();

        match access(&c, "google", 0, 60_000).unwrap() {
            Access::Ready(t) => assert_eq!(t.expose(), "live"),
            other => panic!("a live token should be usable as is: {other:?}"),
        }

        // inside the margin: the caller is told to refresh, and with what
        let refreshed = match access(&c, "google", 50_000, 60_000).unwrap() {
            Access::NeedsRefresh(rt) => {
                assert_eq!(rt.expose(), "rt");
                Secret::new("fresh")
            }
            other => panic!("a spent token must not be handed out: {other:?}"),
        };
        store_refreshed(&c, "google", &refreshed, 200_000).unwrap();

        match access(&c, "google", 50_000, 60_000).unwrap() {
            Access::Ready(t) => assert_eq!(t.expose(), "fresh", "written back"),
            other => panic!("{other:?}"),
        }
    }

    /// A token that can be printed is a token that ends up in a log line,
    /// a rendering, or a model context. It must resist all three.
    #[test]
    fn a_secret_cannot_be_printed_into_anything() {
        let s = Secret::new("ya29-THE-ACTUAL-TOKEN");
        assert_eq!(format!("{s:?}"), "Secret(redacted)");
        assert!(!format!("{s:#?}").contains("ACTUAL"));
        // and the real value takes a deliberate, greppable call
        assert_eq!(s.expose(), "ya29-THE-ACTUAL-TOKEN");
    }

    /// An expired connection with nothing to refresh must say so, not hand
    /// out a dead token and surface as an opaque 401 three layers up.
    #[test]
    fn an_expired_connection_without_a_refresh_token_refuses_plainly() {
        let c = cell();
        save(&c, "google", "a@b.com", &scopes(), "dead", None, 100).unwrap();
        let e = access(&c, "google", 999_999, 60_000).unwrap_err();
        assert!(e.to_string().contains("reconnect"), "{e}");

        // and an absent connection is a different, equally plain refusal
        let e = access(&c, "nope", 0, 0).unwrap_err();
        assert!(e.to_string().contains("not connected"), "{e}");
    }

    #[test]
    fn disconnecting_deletes_the_row_outright() {
        let c = cell();
        save(&c, "google", "a@b.com", &scopes(), "at", Some("rt"), 9_000).unwrap();
        assert!(disconnect(&c, "google").unwrap());

        let n: i64 = c
            .query_row("SELECT count(*) FROM connections", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "hard delete -- a revoked flag leaves the token on disk");
        assert!(!disconnect(&c, "google").unwrap());
    }
}
