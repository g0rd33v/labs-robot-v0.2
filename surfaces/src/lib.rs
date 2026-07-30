//! surfaces: how people reach the Robot (arch sec 10).
//!
//! M1 ships the built-in web Chat (arch sec 10b) with Tier-3 slug auth
//! (Q32/sec 7b): opening the secret slug URL *is* authentication; first
//! open binds a session cookie. Localhost-scoped, honestly.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::{
    collections::HashSet,
    future::Future,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

/// The surface's view of the Robot: hand in text, get the reply. The
/// implementation (robotd's RobotCore) owns boundary logging and journaling.
pub trait Robot: Send + Sync {
    fn handle_message(&self, text: String) -> anyhow::Result<String>;
}

pub struct WebState {
    pub robot: Arc<dyn Robot>,
    pub slug_hash: String,
    sessions: Mutex<HashSet<String>>,
}

impl WebState {
    pub fn new(robot: Arc<dyn Robot>, slug_hash: String) -> Self {
        Self {
            robot,
            slug_hash,
            sessions: Mutex::new(HashSet::new()),
        }
    }

    fn session_valid(&self, headers: &HeaderMap) -> bool {
        let Some(cookie) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) else {
            return false;
        };
        let sid = cookie
            .split(';')
            .filter_map(|p| p.trim().strip_prefix("sid="))
            .next();
        match sid {
            Some(s) => self.sessions.lock().expect("sessions lock").contains(s),
            None => false,
        }
    }
}

pub fn router(state: Arc<WebState>) -> Router {
    Router::new()
        .route("/a/{token}", get(open_slug))
        .route("/chat", get(chat_page))
        .route("/api/message", post(api_message))
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

async fn open_slug(State(st): State<Arc<WebState>>, Path(token): Path<String>) -> Response {
    if trust::ids::sha256_hex(token.as_bytes()) != st.slug_hash {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let sid = trust::ids::random_hex(32);
    st.sessions
        .lock()
        .expect("sessions lock")
        .insert(sid.clone());
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

async fn chat_page(State(st): State<Arc<WebState>>, headers: HeaderMap) -> Response {
    if !st.session_valid(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            "open your robot's slug URL first (printed at boot)",
        )
            .into_response();
    }
    Html(include_str!("chat.html")).into_response()
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
    if !st.session_valid(&headers) {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    }
    let robot = st.robot.clone();
    match tokio::task::spawn_blocking(move || robot.handle_message(msg.text)).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct Echo;
    impl Robot for Echo {
        fn handle_message(&self, t: String) -> anyhow::Result<String> {
            Ok(format!("echo: {t}"))
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

    #[tokio::test]
    async fn wrong_slug_is_404_and_no_session_is_401() {
        let (st, _) = test_state();
        let app = router(st);

        let res = app
            .clone()
            .oneshot(Request::get("/a/wrong").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        let res = app
            .clone()
            .oneshot(Request::get("/chat").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let res = app
            .oneshot(
                Request::post("/api/message")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"text":"hi"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn slug_binds_session_and_messages_flow() {
        let (st, token) = test_state();
        let app = router(st);

        let res = app
            .clone()
            .oneshot(
                Request::get(format!("/a/{token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();

        let res = app
            .oneshot(
                Request::post("/api/message")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"text":"hi"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&bytes).contains("echo: hi"));
    }
}
