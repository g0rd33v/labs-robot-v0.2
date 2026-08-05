//! surfaces: how people reach the Robot (arch sec 10).
//!
//! M5: the built-in web Chat (sec 10b) with Tier-3 slug auth for the owner
//! (Q32) and one-time invite links for members (Q2); SSE message push;
//! voice-note/file upload; Dashboard-lite (sec 10a: Overview, Registry,
//! Boundary) served by the binary, owner-only.

pub mod dash;

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio_stream::StreamExt;

pub use dash::DashData;

/// The surface's view of the Robot. Every method is principal-scoped:
/// cell isolation (law #2) starts at this boundary.
pub trait Robot: Send + Sync {
    fn handle_message(&self, principal: i64, text: String) -> anyhow::Result<String>;
    fn handle_media(
        &self,
        principal: i64,
        filename: String,
        bytes: Vec<u8>,
    ) -> anyhow::Result<String>;
    /// (ts, direction, content, intent_id) -- intent empty when there is
    /// no receipt behind the line.
    fn history(&self, principal: i64, after_ts: i64)
        -> anyhow::Result<Vec<(i64, String, String, String)>>;
    /// Redeem a one-time invite token; returns the new member principal.
    fn accept_invite(&self, token: &str) -> anyhow::Result<(i64, String)>;
    /// New-message signal: receivers get the principal id that has news.
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<i64>;
    /// Live draft text while an answer streams: (principal, accumulated).
    fn subscribe_drafts(&self) -> tokio::sync::broadcast::Receiver<(i64, String)>;
    /// The receipt behind one turn, as JSON (spec 4.1.4's inspector).
    fn receipt(&self, principal: i64, intent_id: &str) -> anyhow::Result<serde_json::Value>;
    /// Anything waiting for this person's yes, as JSON cards.
    fn pending_approvals(&self, principal: i64) -> anyhow::Result<serde_json::Value>;
    /// Answer one. Returns the robot's reply.
    fn answer_approval(
        &self,
        principal: i64,
        intent_id: &str,
        approved: bool,
    ) -> anyhow::Result<String>;
    /// Everything held about THIS person, all five sec 4b categories.
    fn my_registry(&self, principal: i64) -> anyhow::Result<serde_json::Value>;
    /// Everything held about this person, as one portable document.
    fn my_export(&self, principal: i64) -> anyhow::Result<serde_json::Value>;
    /// Remove a person and destroy their cell (spec 4.2.3.4). `actor` may
    /// remove themselves; only the owner may remove anyone else.
    fn remove_person(&self, actor: i64, target: i64) -> anyhow::Result<String>;
    /// One item action: correct | confirm | erase.
    fn registry_action(
        &self,
        principal: i64,
        category: &str,
        index: usize,
        action: &str,
        value: Option<&str>,
    ) -> anyhow::Result<String>;
    fn dashboard(&self, principal: i64) -> anyhow::Result<DashData>;
    fn owner_principal(&self) -> i64;
    /// Finish an OAuth sign-in; returns which account was connected.
    fn complete_google_auth(&self, state: &str, code: &str) -> anyhow::Result<String>;
    /// Say something to the owner in their chat, unprompted.
    fn tell_owner(&self, text: &str) -> anyhow::Result<()>;
}

/// Sessions live in memory only. They are capped and aged so a long-lived
/// robot cannot accumulate every cookie it ever issued (each visit to the
/// bookmarked slug URL mints a new one, and nothing ever removed them).
const SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000; // 30 days
const MAX_SESSIONS: usize = 512;

struct Session {
    principal: i64,
    last_seen: i64,
}

pub struct WebState {
    pub robot: Arc<dyn Robot>,
    pub slug_hash: String,
    /// Mount point when the robot lives under a path on a shared domain
    /// (`/bender/demo`). Empty = the root. Every link and fetch the client
    /// makes is written relative to this, so one binary serves both shapes
    /// and neither guesses.
    pub prefix: String,
    /// sid -> session
    sessions: Mutex<HashMap<String, Session>>,
}

impl WebState {
    pub fn new(robot: Arc<dyn Robot>, slug_hash: String) -> Self {
        Self::mounted(robot, slug_hash, String::new())
    }

    /// As `new`, mounted under a path prefix.
    pub fn mounted(robot: Arc<dyn Robot>, slug_hash: String, prefix: String) -> Self {
        let prefix = prefix.trim_end_matches('/').to_string();
        Self {
            robot,
            slug_hash,
            prefix,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn mint_session(&self, principal: i64) -> String {
        let sid = trust::ids::random_hex(32);
        let now = trust::ids::ts_ms();
        // a poisoned sessions lock must not brick the whole web surface
        let mut sessions = match self.sessions.lock() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        sessions.retain(|_, s| now - s.last_seen < SESSION_TTL_MS);
        if sessions.len() >= MAX_SESSIONS {
            // drop the least recently used
            if let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, s)| s.last_seen)
                .map(|(k, _)| k.clone())
            {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(
            sid.clone(),
            Session {
                principal,
                last_seen: now,
            },
        );
        sid
    }

    fn session_principal(&self, headers: &HeaderMap) -> Option<i64> {
        let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
        let sid = cookie
            .split(';')
            .filter_map(|p| p.trim().strip_prefix("sid="))
            .next()?;
        let mut sessions = match self.sessions.lock() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = trust::ids::ts_ms();
        let session = sessions.get_mut(sid)?;
        if now - session.last_seen >= SESSION_TTL_MS {
            sessions.remove(sid);
            return None;
        }
        session.last_seen = now;
        Some(session.principal)
    }
}

pub fn router(state: Arc<WebState>) -> Router {
    let prefix = state.prefix.clone();
    let app = routes(state);
    if prefix.is_empty() {
        app
    } else {
        // nest, so the app answers on /bender/demo/... and nothing else --
        // a robot mounted under a path must not also answer at the root of
        // a domain it shares with other services
        Router::new().nest(&prefix, app)
    }
}

fn routes(state: Arc<WebState>) -> Router {
    Router::new()
        .route("/a/{token}", get(open_slug))
        .route("/i/{token}", get(open_invite))
        .route("/chat", get(chat_page))
        .route("/dash", get(dash_page))
        .route("/api/message", post(api_message))
        .route("/api/history", get(api_history))
        .route("/api/upload", post(api_upload))
        .route("/api/stream", get(api_stream))
        .route("/api/receipt/{intent}", get(api_receipt))
        .route("/api/approvals", get(api_approvals))
        .route("/api/approvals/{intent}", post(api_answer_approval))
        .route("/api/registry", get(api_registry))
        .route("/api/registry/action", post(api_registry_action))
        .route("/api/people/{id}/remove", post(api_remove_person))
        .route("/api/export", get(api_export))
        .route("/me", get(me_page))
        .route("/oauth/google/callback", get(oauth_callback))
        .with_state(state)
}

/// Serve until `shutdown` resolves.
pub async fn serve(
    state: Arc<WebState>,
    addr: SocketAddr,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

fn session_redirect(sid: String, prefix: &str) -> Response {
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::LOCATION, format!("{prefix}/chat"))
        .header(
            header::SET_COOKIE,
            format!(
                "sid={sid}; HttpOnly; SameSite=Strict; Path={}/",
                if prefix.is_empty() { "" } else { prefix }
            ),
        )
        .body(Body::empty())
        .expect("static response")
}

async fn open_slug(State(st): State<Arc<WebState>>, Path(token): Path<String>) -> Response {
    if trust::ids::sha256_hex(token.as_bytes()) != st.slug_hash {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let sid = st.mint_session(st.robot.owner_principal());
    session_redirect(sid, &st.prefix)
}

async fn open_invite(State(st): State<Arc<WebState>>, Path(token): Path<String>) -> Response {
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.accept_invite(&token)).await {
        Ok(Ok((principal, name))) => {
            tracing::info!("invite redeemed: {name} (principal {principal})");
            let sid = st.mint_session(principal);
            session_redirect(sid, &st.prefix)
        }
        Ok(Err(e)) => {
            tracing::warn!("invite rejected: {e}");
            (StatusCode::FORBIDDEN, "invite invalid or already used").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "invite failed").into_response(),
    }
}

/// The loopback redirect Google sends the person back to (Q29).
///
/// Deliberately NOT session-authenticated: the browser arriving here has
/// come from Google's consent screen, not from the robot's own pages, and
/// requiring a session would break the flow for anyone whose consent opened
/// in a different browser. What stands in for a session is the `state`
/// value -- unguessable, single-use, and minted only by a person typing
/// `/connect` in an authenticated chat.
async fn oauth_callback(
    State(st): State<Arc<WebState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    // the person declined on Google's screen, or Google refused
    if let Some(err) = q.get("error") {
        return Html(done_page(&format!(
            "sign-in was not completed ({}). nothing was connected.",
            html_escape(err)
        )))
        .into_response();
    }
    let (Some(state), Some(code)) = (q.get("state"), q.get("code")) else {
        return (StatusCode::BAD_REQUEST, "missing code or state").into_response();
    };
    let (state, code) = (state.clone(), code.clone());
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.complete_google_auth(&state, &code)).await {
        Ok(Ok(account)) => {
            let account: String = account;
            // the robot says so in the chat too, so the connection is
            // visible where the person actually is
            let _ = st.robot.tell_owner(&format!(
                "connected google as {account}. try \"what's on my calendar tomorrow\"."
            ));
            Html(done_page(&format!(
                "connected as {}. you can close this tab and go back to the chat.",
                html_escape(&account)
            )))
            .into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!("google sign-in failed: {e}");
            (
                StatusCode::FORBIDDEN,
                Html(done_page(
                    "that sign-in link was already used or has expired. \
                     type /connect in the chat to get a fresh one.",
                )),
            )
                .into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed").into_response(),
    }
}

/// The only page a person sees from an outside redirect. No scripts, no
/// state, nothing to interact with -- it exists to say what happened.
fn done_page(message: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>robot</title>\
         <body style=\"font:16px/1.6 system-ui;max-width:34rem;margin:4rem auto;padding:0 1rem\">\
         <p>{message}</p></body>"
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The receipt behind a reply (spec 4.1.4). Scoped to the caller's own
/// cell -- a receipt names what the robot did FOR THIS PERSON, and cells
/// do not read each other.
async fn api_receipt(
    State(st): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(intent): Path<String>,
) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.receipt(principal, &intent)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => {
            tracing::warn!("receipt lookup failed: {e:#}");
            (StatusCode::NOT_FOUND, "no receipt for that turn").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "receipt failed").into_response(),
    }
}

async fn api_approvals(State(st): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.pending_approvals(principal)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "approvals failed").into_response(),
    }
}

#[derive(serde::Deserialize)]
struct ApprovalBody {
    approved: bool,
}

/// Approve or deny (spec 4.1.4's buttons). The same durable path a typed
/// "yes" takes -- the buttons are a surface over sec 3b.2, not a bypass.
async fn api_answer_approval(
    State(st): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(intent): Path<String>,
    Json(body): Json<ApprovalBody>,
) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || {
        robot.answer_approval(principal, &intent, body.approved)
    })
    .await
    {
        Ok(Ok(reply)) => Json(serde_json::json!({ "reply": reply })).into_response(),
        Ok(Err(e)) => {
            tracing::warn!("approval answer failed: {e:#}");
            (StatusCode::CONFLICT, "that approval is no longer open").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "approval failed").into_response(),
    }
}

/// The member's own Registry (spec 4.2.4 + 10.1.3: full self-view, all
/// five categories -- the owner's decision).
async fn api_registry(State(st): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.my_registry(principal)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => {
            tracing::warn!("registry read failed: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "registry failed").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "registry failed").into_response(),
    }
}

#[derive(serde::Deserialize)]
struct RegistryAction {
    category: String,
    index: usize,
    action: String,
    #[serde(default)]
    value: Option<String>,
}

/// Take it with you (spec 4.2.3.4). Served as a download rather than a
/// page, because the point is a file that outlives this robot.
async fn api_export(State(st): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.my_export(principal)).await {
        Ok(Ok(doc)) => {
            let body = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into());
            (
                [
                    (header::CONTENT_TYPE, "application/json"),
                    (
                        header::CONTENT_DISPOSITION,
                        "attachment; filename=\"my-data.json\"",
                    ),
                ],
                body,
            )
                .into_response()
        }
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "export failed").into_response(),
    }
}

/// What the person must type to prove they mean it. Not a checkbox: this
/// destroys a key, and there is no undo behind it.
#[derive(serde::Deserialize)]
struct RemoveBody {
    confirm: String,
}

/// Erase a person (spec 4.2.3.4). The authorisation check lives in the
/// robot, not here -- a surface must never be the thing that decides who
/// may delete whom.
async fn api_remove_person(
    State(st): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(who): Path<String>,
    Json(body): Json<RemoveBody>,
) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    if body.confirm.trim() != "ERASE" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "message": "type ERASE to confirm" })),
        )
            .into_response();
    }
    // "me" is the only identity a member's own page ever names; an owner
    // removing someone else passes that person's id.
    let target = if who == "me" {
        principal
    } else {
        match who.parse::<i64>() {
            Ok(n) => n,
            Err(_) => return (StatusCode::BAD_REQUEST, "not a person").into_response(),
        }
    };
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.remove_person(principal, target)).await {
        Ok(Ok(msg)) => Json(serde_json::json!({ "ok": true, "message": msg })).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "message": e.to_string() })),
        )
            .into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "removal failed").into_response(),
    }
}

async fn api_registry_action(
    State(st): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(body): Json<RegistryAction>,
) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || {
        robot.registry_action(
            principal,
            &body.category,
            body.index,
            &body.action,
            body.value.as_deref(),
        )
    })
    .await
    {
        Ok(Ok(msg)) => Json(serde_json::json!({ "ok": true, "message": msg })).into_response(),
        // an erase that failed must SAY so -- spec 4.2.4: never fake success
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "message": e.to_string() })),
        )
            .into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "action failed").into_response(),
    }
}

async fn me_page(State(st): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    if st.session_principal(&headers).is_none() {
        return (StatusCode::UNAUTHORIZED, "open your link first").into_response();
    }
    Html(include_str!("me.html").replace("__PREFIX__", &st.prefix)).into_response()
}

async fn chat_page(State(st): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    if st.session_principal(&headers).is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            "open your robot's slug URL (owner) or invite link (member) first",
        )
            .into_response();
    }
    Html(include_str!("chat.html").replace("__PREFIX__", &st.prefix)).into_response()
}

async fn dash_page(State(st): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    if principal != st.robot.owner_principal() {
        return (
            StatusCode::FORBIDDEN,
            "the dashboard is the owner's control room",
        )
            .into_response();
    }
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.dashboard(principal)).await {
        Ok(Ok(data)) => Html(dash::render(&data).replace("__PREFIX__", &st.prefix)).into_response(),
        Ok(Err(e)) => {
            tracing::error!("dashboard failed: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "dashboard failed").into_response()
        }
        Err(e) => {
            tracing::error!("dashboard join error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "dashboard failed").into_response()
        }
    }
}

#[derive(serde::Deserialize)]
struct MsgIn {
    text: String,
}

#[derive(serde::Serialize)]
struct MsgOut {
    reply: String,
}

async fn api_message(
    State(st): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(msg): Json<MsgIn>,
) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.handle_message(principal, msg.text)).await {
        Ok(Ok(reply)) => Json(MsgOut { reply }).into_response(),
        Ok(Err(e)) => {
            tracing::error!("turn failed: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "turn failed").into_response()
        }
        Err(e) => {
            tracing::error!("turn join error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "turn failed").into_response()
        }
    }
}

#[derive(serde::Deserialize)]
struct HistoryQ {
    #[serde(default)]
    after: i64,
}

#[derive(serde::Serialize)]
struct HistoryRow {
    ts: i64,
    direction: String,
    content: String,
    /// The turn behind this reply, so the chat can offer its receipt.
    /// Empty when there is none (inbound, or recorded before this existed).
    #[serde(skip_serializing_if = "String::is_empty")]
    intent: String,
}

async fn api_history(
    State(st): State<Arc<WebState>>,
    headers: HeaderMap,
    Query(q): Query<HistoryQ>,
) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.history(principal, q.after)).await {
        Ok(Ok(rows)) => Json(
            rows.into_iter()
                .map(|(ts, direction, content, intent)| HistoryRow {
                    ts,
                    direction,
                    content,
                    intent,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "history failed").into_response(),
    }
}

async fn api_upload(
    State(st): State<Arc<WebState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let filename = headers
        .get("x-filename")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("upload.bin")
        .to_string();
    if body.len() > 25 * 1024 * 1024 {
        return (StatusCode::PAYLOAD_TOO_LARGE, "25MB cap for the MVP").into_response();
    }
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || {
        robot.handle_media(principal, filename, body.to_vec())
    })
    .await
    {
        Ok(Ok(reply)) => Json(MsgOut { reply }).into_response(),
        Ok(Err(e)) => {
            tracing::error!("upload failed: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "upload failed").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "upload failed").into_response(),
    }
}

async fn api_stream(State(st): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    let Some(principal) = st.session_principal(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let rx = st.robot.subscribe();
    let news = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(move |item| {
        match item {
            Ok(p) if p == principal => Some(Ok::<Event, std::convert::Infallible>(
                Event::default().data("new"),
            )),
            Ok(_) => None, // another principal's news is not ours
            // The receiver fell behind and messages were dropped. Saying
            // nothing leaves a backgrounded tab silently stale forever;
            // "resync" tells the client to refetch history.
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!("sse receiver lagged by {n} events; asking client to resync");
                Some(Ok(Event::default().data("resync")))
            }
        }
    });
    // sec 2c #1: the draft lane. Accumulated text (never deltas), so a
    // dropped frame costs smoothness, not words; JSON-encoded so newlines
    // survive the SSE framing. Display-only -- the canonical reply still
    // arrives via "new" and the history endpoint, receipt attached.
    let drx = st.robot.subscribe_drafts();
    let drafts = tokio_stream::wrappers::BroadcastStream::new(drx).filter_map(move |item| {
        match item {
            Ok((p, text)) if p == principal => {
                let payload = serde_json::to_string(&text).unwrap_or_default();
                Some(Ok::<Event, std::convert::Infallible>(
                    Event::default().event("draft").data(payload),
                ))
            }
            Ok(_) => None,
            Err(_) => None, // a lagged draft is just a less smooth draft
        }
    });
    let stream = news.merge(drafts);
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

#[cfg(test)]
mod tests {
    /// A page's script must not bind to markup that does not exist yet.
    ///
    /// This is a real regression, not a hypothetical: the receipt modal was
    /// appended after `</script>`, so `getElementById('modal')` returned
    /// null, the TypeError killed the rest of the script, and with it the
    /// history poll -- the chat rendered COMPLETELY EMPTY. Every test
    /// passed, because no test opens the page. So this one reads the
    /// document order instead: every id the script looks up at load time
    /// must appear above the script that looks it up.
    #[test]
    fn every_element_the_script_binds_to_exists_before_the_script() {
        for (name, html) in [
            ("chat.html", include_str!("chat.html")),
            ("me.html", include_str!("me.html")),
        ] {
            let script_at = html.find("<script>").unwrap_or_else(|| panic!("{name}: no script"));
            let (markup, script) = html.split_at(script_at);
            // every getElementById('x') the script performs
            let mut rest = script;
            while let Some(at) = rest.find("getElementById('") {
                rest = &rest[at + "getElementById('".len()..];
                let id = &rest[..rest.find('\'').expect("closing quote")];
                assert!(
                    markup.contains(&format!("id=\"{id}\"")),
                    "{name}: the script binds #{id}, but that element is not in \
                     the markup ABOVE the script. At load time it is null, and \
                     the TypeError takes every later statement with it -- \
                     including whatever renders the page."
                );
            }
        }
    }

    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct Echo;
    impl Robot for Echo {
        fn complete_google_auth(&self, _state: &str, _code: &str) -> anyhow::Result<String> {
            anyhow::bail!("no connector in the test double")
        }
        fn tell_owner(&self, _text: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn subscribe_drafts(&self) -> tokio::sync::broadcast::Receiver<(i64, String)> {
            tokio::sync::broadcast::channel(1).1
        }
        fn receipt(&self, _p: i64, intent: &str) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({ "intent": intent, "status": "verified", "claims": [] }))
        }
        fn pending_approvals(&self, _p: i64) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!([]))
        }
        fn answer_approval(&self, _p: i64, _i: &str, _a: bool) -> anyhow::Result<String> {
            anyhow::bail!("no approval in the test double")
        }
        fn my_registry(&self, _p: i64) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({ "knowledge": [], "instructions": [],
                                   "preferences": [], "media": [], "grants": [] }))
        }
        fn my_export(&self, _p: i64) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({ "registry": {}, "conversation": [] }))
        }
        fn remove_person(&self, _a: i64, _t: i64) -> anyhow::Result<String> {
            Ok("erased".into())
        }
        fn registry_action(
            &self,
            _p: i64,
            _c: &str,
            _i: usize,
            _a: &str,
            _v: Option<&str>,
        ) -> anyhow::Result<String> {
            Ok("ok".into())
        }

        fn handle_message(&self, p: i64, t: String) -> anyhow::Result<String> {
            Ok(format!("echo[{p}]: {t}"))
        }
        fn handle_media(&self, p: i64, name: String, bytes: Vec<u8>) -> anyhow::Result<String> {
            Ok(format!("stored[{p}]: {name} ({} bytes)", bytes.len()))
        }
        fn history(&self, p: i64, after: i64) -> anyhow::Result<Vec<(i64, String, String, String)>> {
            Ok(vec![(after + 1, "out".into(), format!("h[{p}]"), String::new())])
        }
        fn accept_invite(&self, token: &str) -> anyhow::Result<(i64, String)> {
            if token == "good" {
                Ok((7, "member-1".into()))
            } else {
                anyhow::bail!("bad invite")
            }
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<i64> {
            tokio::sync::broadcast::channel(4).1
        }
        fn dashboard(&self, _p: i64) -> anyhow::Result<DashData> {
            Ok(DashData::default())
        }
        fn owner_principal(&self) -> i64 {
            1
        }
    }

    fn test_state() -> (Arc<WebState>, String) {
        let token = "test-token-123".to_string();
        let st = Arc::new(WebState::new(
            Arc::new(Echo),
            trust::ids::sha256_hex(token.as_bytes()),
        ));
        (st, token)
    }

    async fn login(app: &Router, path: &str) -> String {
        let res = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER, "{path}");
        res.headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn owner_and_member_sessions_are_distinct_principals() {
        let (st, token) = test_state();
        let app = router(st);
        let owner_cookie = login(&app, &format!("/a/{token}")).await;
        let member_cookie = login(&app, "/i/good").await;

        for (cookie, expect) in [(&owner_cookie, "echo[1]"), (&member_cookie, "echo[7]")] {
            let res = app
                .clone()
                .oneshot(
                    Request::post("/api/message")
                        .header(header::COOKIE, cookie)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"text":"hi"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            assert!(
                String::from_utf8_lossy(&bytes).contains(expect),
                "expected {expect}"
            );
        }
    }

    #[tokio::test]
    async fn bad_invite_is_403_and_dash_is_owner_only() {
        let (st, token) = test_state();
        let app = router(st);
        let res = app
            .clone()
            .oneshot(Request::get("/i/wrong").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let member_cookie = login(&app, "/i/good").await;
        let res = app
            .clone()
            .oneshot(
                Request::get("/dash")
                    .header(header::COOKIE, &member_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let owner_cookie = login(&app, &format!("/a/{token}")).await;
        let res = app
            .oneshot(
                Request::get("/dash")
                    .header(header::COOKIE, &owner_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn upload_reaches_handle_media() {
        let (st, token) = test_state();
        let app = router(st);
        let cookie = login(&app, &format!("/a/{token}")).await;
        let res = app
            .oneshot(
                Request::post("/api/upload")
                    .header(header::COOKIE, &cookie)
                    .header("x-filename", "note.ogg")
                    .body(Body::from(vec![1u8, 2, 3]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&bytes).contains("note.ogg (3 bytes)"));
    }
}
