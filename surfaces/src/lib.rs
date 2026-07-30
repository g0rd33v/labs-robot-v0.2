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
    fn history(&self, principal: i64, after_ts: i64)
        -> anyhow::Result<Vec<(i64, String, String)>>;
    /// Redeem a one-time invite token; returns the new member principal.
    fn accept_invite(&self, token: &str) -> anyhow::Result<(i64, String)>;
    /// New-message signal: receivers get the principal id that has news.
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<i64>;
    fn dashboard(&self, principal: i64) -> anyhow::Result<DashData>;
    fn owner_principal(&self) -> i64;
}

pub struct WebState {
    pub robot: Arc<dyn Robot>,
    pub slug_hash: String,
    /// sid -> principal
    sessions: Mutex<HashMap<String, i64>>,
}

impl WebState {
    pub fn new(robot: Arc<dyn Robot>, slug_hash: String) -> Self {
        Self {
            robot,
            slug_hash,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn mint_session(&self, principal: i64) -> String {
        let sid = trust::ids::random_hex(32);
        self.sessions
            .lock()
            .expect("sessions lock")
            .insert(sid.clone(), principal);
        sid
    }

    fn session_principal(&self, headers: &HeaderMap) -> Option<i64> {
        let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
        let sid = cookie
            .split(';')
            .filter_map(|p| p.trim().strip_prefix("sid="))
            .next()?;
        self.sessions.lock().expect("sessions lock").get(sid).copied()
    }
}

pub fn router(state: Arc<WebState>) -> Router {
    Router::new()
        .route("/a/{token}", get(open_slug))
        .route("/i/{token}", get(open_invite))
        .route("/chat", get(chat_page))
        .route("/dash", get(dash_page))
        .route("/api/message", post(api_message))
        .route("/api/history", get(api_history))
        .route("/api/upload", post(api_upload))
        .route("/api/stream", get(api_stream))
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

fn session_redirect(sid: String) -> Response {
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::LOCATION, "/chat")
        .header(
            header::SET_COOKIE,
            format!("sid={sid}; HttpOnly; SameSite=Strict; Path=/"),
        )
        .body(Body::empty())
        .expect("static response")
}

async fn open_slug(State(st): State<Arc<WebState>>, Path(token): Path<String>) -> Response {
    if trust::ids::sha256_hex(token.as_bytes()) != st.slug_hash {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let sid = st.mint_session(st.robot.owner_principal());
    session_redirect(sid)
}

async fn open_invite(State(st): State<Arc<WebState>>, Path(token): Path<String>) -> Response {
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.accept_invite(&token)).await {
        Ok(Ok((principal, name))) => {
            tracing::info!("invite redeemed: {name} (principal {principal})");
            let sid = st.mint_session(principal);
            session_redirect(sid)
        }
        Ok(Err(e)) => {
            tracing::warn!("invite rejected: {e}");
            (StatusCode::FORBIDDEN, "invite invalid or already used").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "invite failed").into_response(),
    }
}

async fn chat_page(State(st): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    if st.session_principal(&headers).is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            "open your robot's slug URL (owner) or invite link (member) first",
        )
            .into_response();
    }
    Html(include_str!("chat.html")).into_response()
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
        Ok(Ok(data)) => Html(dash::render(&data)).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "dashboard failed").into_response(),
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
                .map(|(ts, direction, content)| HistoryRow {
                    ts,
                    direction,
                    content,
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
    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(move |item| {
        match item {
            Ok(p) if p == principal => {
                Some(Ok::<Event, std::convert::Infallible>(Event::default().data("new")))
            }
            _ => None,
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct Echo;
    impl Robot for Echo {
        fn handle_message(&self, p: i64, t: String) -> anyhow::Result<String> {
            Ok(format!("echo[{p}]: {t}"))
        }
        fn handle_media(&self, p: i64, name: String, bytes: Vec<u8>) -> anyhow::Result<String> {
            Ok(format!("stored[{p}]: {name} ({} bytes)", bytes.len()))
        }
        fn history(&self, p: i64, after: i64) -> anyhow::Result<Vec<(i64, String, String)>> {
            Ok(vec![(after + 1, "out".into(), format!("h[{p}]"))])
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
