//! HTTP-layer integration tests against a REAL RobotCore.
//!
//! Until the lib/bin split these could not exist: `robotd` was a binary-only
//! crate, so nothing could import it, and the `surfaces` tests ran entirely
//! against a hand-written double. That left the whole seam untested --
//! upload→vault→receipt→history, `/dash` authorization against real state,
//! and cross-principal isolation at the session layer.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use robotd::config::{HubSection, MindSection, RobotConfig, RobotSection, ServerSection};
use tower::ServiceExt;

struct TestRobot {
    router: axum::Router,
    dir: std::path::PathBuf,
    slug: String,
}

impl Drop for TestRobot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A real Robot on a temp directory, with no outbound services configured:
/// the deterministic floor works, model turns answer honestly offline, and
/// no test touches the network.
fn boot_test_robot() -> TestRobot {
    let dir = std::env::temp_dir().join(format!("httptest-{}", trust::ids::random_hex(6)));
    let cfg = RobotConfig {
        robot: RobotSection {
            name: "bender-test".into(),
            data_dir: dir.to_string_lossy().into_owned(),
        },
        server: ServerSection {
            host: "127.0.0.1".into(),
            port: 0,
            public_base: String::new(),
        },
        mind: MindSection {
            embeddings: false,
            model_cache: dir.join("models").to_string_lossy().into_owned(),
        },
        hub: HubSection::default(),
            backup: robotd::config::BackupSection {
                every_hours: 0, // tests never shell out to a real backup
                script: String::new(),
            },
    };
    // hermetic: never pick up a developer's keys from the environment
    std::env::remove_var("OPENROUTER_API_KEY");
    std::env::remove_var("SERPER_API_KEY");
    let booted = robotd::boot::bootstrap(&cfg).expect("bootstrap");
    let slug = booted
        .slug_url
        .rsplit("/a/")
        .next()
        .expect("slug in url")
        .to_string();
    TestRobot {
        router: surfaces::router(booted.state.clone()),
        dir,
        slug,
    }
}

async fn get(app: &axum::Router, path: &str, cookie: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::get(path);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post_json(app: &axum::Router, path: &str, cookie: &str, body: &str) -> (StatusCode, String) {
    let res = app
        .clone()
        .oneshot(
            Request::post(path)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn login(app: &axum::Router, path: &str) -> String {
    let res = app
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER, "login at {path}");
    res.headers()
        .get(header::SET_COOKIE)
        .expect("session cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

fn say(text: &str) -> String {
    serde_json::json!({ "text": text }).to_string()
}

#[tokio::test]
async fn unauthenticated_requests_are_refused_everywhere() {
    let t = boot_test_robot();
    for path in ["/chat", "/dash", "/api/history?after=0", "/api/stream"] {
        let (status, _) = get(&t.router, path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path} must require a session");
    }
    let (status, _) = get(&t.router, "/a/definitely-not-the-slug", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_floor_turn_round_trips_through_http_and_history() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    let (status, body) = post_json(&t.router, "/api/message", &cookie, &say("what time is it?")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("it's"), "floor answer expected: {body}");

    // history serves both sides of the exchange
    let (status, hist) = get(&t.router, "/api/history?after=0", Some(cookie.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&hist).unwrap();
    assert_eq!(rows.len(), 2, "one in, one out: {hist}");
    assert_eq!(rows[0]["direction"], "in");
    assert_eq!(rows[1]["direction"], "out");

    // ...and `after` actually filters
    let latest = rows[1]["ts"].as_i64().unwrap();
    let (_, hist) = get(
        &t.router,
        &format!("/api/history?after={latest}"),
        Some(cookie.as_str()),
    )
    .await;
    assert_eq!(serde_json::from_str::<Vec<serde_json::Value>>(&hist).unwrap().len(), 0);
}

/// The upload seam end to end: bytes in, vault storage, a receipt, and the
/// reply visible in history. Nothing covered this before.
#[tokio::test]
async fn upload_reaches_the_vault_and_produces_a_receipt() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    let payload = b"not really a pdf, but bytes are bytes".to_vec();
    let res = t
        .router
        .clone()
        .oneshot(
            Request::post("/api/upload")
                .header(header::COOKIE, &cookie)
                .header("x-filename", "notes.pdf")
                .body(Body::from(payload.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let reply = String::from_utf8_lossy(&bytes);
    assert!(reply.contains("notes.pdf"), "{reply}");

    // the file is in the vault, content-addressed and sealed on disk
    let hash = trust::ids::sha256_hex(&payload);
    let on_disk = t.dir.join("media");
    let found = walk(&on_disk)
        .into_iter()
        .find(|p| p.to_string_lossy().contains(&hash[2..10]));
    let found = found.unwrap_or_else(|| panic!("vault file for {hash} not found under {on_disk:?}"));
    let sealed = std::fs::read(&found).unwrap();
    assert!(
        !sealed.windows(payload.len()).any(|w| w == payload.as_slice()),
        "vault contents must not be plaintext on disk"
    );

    // and the reply reached history
    let (_, hist) = get(&t.router, "/api/history?after=0", Some(cookie.as_str())).await;
    assert!(hist.contains("notes.pdf"), "upload reply missing from history");
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Cell isolation (law #2) at the session layer: a member's cookie must not
/// reach the owner's history, and the dashboard is the owner's alone.
#[tokio::test]
async fn a_member_session_cannot_read_the_owner_or_the_dashboard() {
    let t = boot_test_robot();
    let owner = login(&t.router, &format!("/a/{}", t.slug)).await;

    // owner stores something private and mints an invite
    let (_, _) = post_json(
        &t.router,
        "/api/message",
        &owner,
        &say("remember that the launch code is 4242"),
    )
    .await;
    let (_, invite_reply) = post_json(&t.router, "/api/message", &owner, &say("invite")).await;
    let token = invite_reply
        .split("/i/")
        .nth(1)
        .and_then(|s| s.split(['\\', '"', ' ', '\n']).next())
        .expect("invite token")
        .to_string();

    let member = login(&t.router, &format!("/i/{token}")).await;

    // the member's history is their own, and empty
    let (status, hist) = get(&t.router, "/api/history?after=0", Some(member.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!hist.contains("4242"), "owner content leaked to member: {hist}");
    assert_eq!(
        serde_json::from_str::<Vec<serde_json::Value>>(&hist).unwrap().len(),
        0
    );

    // the registry the member sees is their own, not the owner's
    let (_, reply) = post_json(&t.router, "/api/message", &member, &say("my facts")).await;
    assert!(!reply.contains("4242"), "cross-cell read: {reply}");

    // the dashboard is owner-only, and says so with 403 rather than 500
    let (status, _) = get(&t.router, "/dash", Some(member.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, dash) = get(&t.router, "/dash", Some(owner.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(dash.contains("boundary log"), "dashboard should render panels");

    // a member cannot mint invites, and the attempt creates nothing
    let (_, reply) = post_json(&t.router, "/api/message", &member, &say("invite")).await;
    assert!(reply.contains("only the owner"), "{reply}");
    assert!(!reply.contains("/i/"), "a refused invite must not leak a link");

    // and the invite link is one-time
    let res = t
        .router
        .clone()
        .oneshot(Request::get(format!("/i/{token}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "invite must be single-use");
}

/// With no gateway configured the robot must stay useful and honest rather
/// than erroring: the floor works, model turns say the brain is offline.
#[tokio::test]
async fn offline_robot_degrades_honestly_over_http() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    let (status, reply) = post_json(&t.router, "/api/message", &cookie, &say("tell me a joke")).await;
    assert_eq!(status, StatusCode::OK, "an offline brain is not a 500");
    assert!(
        reply.contains("offline") || reply.contains("floor"),
        "should say why it cannot answer: {reply}"
    );

    // the floor still serves real work
    let (_, reply) = post_json(
        &t.router,
        "/api/message",
        &cookie,
        &say("remind me in 10 minutes to stretch"),
    )
    .await;
    assert!(reply.contains("i'll remind you"), "{reply}");
    let (_, reply) = post_json(&t.router, "/api/message", &cookie, &say("my reminders")).await;
    assert!(reply.contains("stretch"), "{reply}");
}

/// The language architecture through the real surface, not just the kernel:
/// the same robot, the same session, two languages, each answered in the
/// language it was asked in -- and a language with no pack still gets an
/// ordinary 200 rather than an error.
#[tokio::test]
async fn the_surface_answers_each_person_in_their_own_language() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    let (status, ru) = post_json(
        &t.router,
        "/api/message",
        &cookie,
        &say("напомни через 10 минут размяться"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ru.contains("готово — напомню"), "{ru}");
    // the date is russian too, not an english month glued onto a russian sentence
    assert!(!ru.contains("Jul") && !ru.contains(" on "), "{ru}");

    let (_, ru_list) = post_json(&t.router, "/api/message", &cookie, &say("мои напоминания")).await;
    assert!(ru_list.contains("размяться"), "{ru_list}");

    let (_, en_list) = post_json(&t.router, "/api/message", &cookie, &say("my reminders")).await;
    assert!(en_list.contains("your reminders"), "{en_list}");

    // no pack for these; the robot must still respond, not fail
    for text in ["今何時ですか", "¿qué hora es?", "지금 몇 시야"] {
        let (status, reply) = post_json(&t.router, "/api/message", &cookie, &say(text)).await;
        assert_eq!(status, StatusCode::OK, "{text} must not error");
        assert!(!reply.is_empty(), "{text}");
    }
}
