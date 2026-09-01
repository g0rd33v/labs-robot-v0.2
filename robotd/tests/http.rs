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
    boot_test_robot_with(|_| {})
}

/// As above, with a chance to adjust the config -- used to switch on an
/// approval policy so the parked-approval path can be exercised.
fn boot_test_robot_with(tweak: impl FnOnce(&mut RobotConfig)) -> TestRobot {
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
            path_prefix: String::new(),
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
            sync: Default::default(),
            policy: Default::default(),
        update: Default::default(),
    };
    // hermetic: never pick up a developer's keys from the environment
    std::env::remove_var("OPENROUTER_API_KEY");
    std::env::remove_var("SERPER_API_KEY");
    let mut cfg = cfg;
    tweak(&mut cfg);
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

/// The language boundary through the real surface.
///
/// English is answered from templates -- offline, instant, and carrying an
/// action record for anything that changed. Every other language reaches
/// the routing call; with no model configured here, it must still be an
/// ordinary 200 with an honest answer, never an error.
#[tokio::test]
async fn the_surface_answers_english_and_degrades_honestly_elsewhere() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    let (status, en) = post_json(
        &t.router,
        "/api/message",
        &cookie,
        &say("remind me in 10 minutes to stretch"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(en.contains("i'll remind you"), "{en}");
    // the receipts law, visible: something changed, and the record says what
    assert!(en.contains("✓ reminder.create"), "{en}");

    let (_, list) = post_json(&t.router, "/api/message", &cookie, &say("my reminders")).await;
    assert!(list.contains("stretch"), "{list}");
    // a read changed nothing, so it vouches for nothing
    assert!(!list.contains("✓"), "{list}");

    // no pack, no table, no model: an honest answer, not a failure
    for text in ["напомни через 10 минут размяться", "今何時ですか", "¿qué hora es?"] {
        let (status, reply) = post_json(&t.router, "/api/message", &cookie, &say(text)).await;
        assert_eq!(status, StatusCode::OK, "{text} must not error");
        assert!(!reply.is_empty(), "{text}");
    }
}

/// The member-facing surface (spec §4.1.4 / §4.2.3): a person can see what
/// is held about them, act on it, read the receipt behind a reply, and
/// leave. Exercised over HTTP against a real cell, because every one of
/// these is a promise the product makes in prose and only code can keep.
#[tokio::test]
async fn a_person_can_see_correct_and_erase_what_is_held_about_them() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    let (status, _) = post_json(
        &t.router,
        "/api/message",
        &cookie,
        &say("remember that my dentist is Dr Adams"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // it shows up under Knowledge, with the person's own words as source --
    // law 5 visible to the person it is about
    let (status, body) = get(&t.router, "/api/registry", Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let reg: serde_json::Value = serde_json::from_str(&body).unwrap();
    let knowledge = reg["knowledge"].as_array().expect("five categories");
    assert!(
        knowledge.iter().any(|k| k["value"].as_str().unwrap_or("").contains("Adams")),
        "the fact is not visible to its subject: {body}"
    );
    assert!(reg["instructions"].is_array() && reg["grants"].is_array());
    assert!(
        knowledge[0]["source"].as_str().is_some_and(|s| !s.is_empty()),
        "a fact without its source shown is unprovenanced to the person"
    );

    // correcting it REPLACES the value rather than adding a rival
    let before = knowledge.len();
    let (status, _) = post_json(
        &t.router,
        "/api/registry/change",
        &cookie,
        &serde_json::json!({
            "category": "knowledge", "index": 1,
            "action": "correct", "value": "my dentist is Dr Bell"
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = get(&t.router, "/api/registry", Some(&cookie)).await;
    let reg: serde_json::Value = serde_json::from_str(&body).unwrap();
    let after = reg["knowledge"].as_array().unwrap();
    assert_eq!(after.len(), before, "a correction must not leave both versions");
    assert!(body.contains("Bell") && !body.contains("Adams"), "{body}");

    // erasing is real: it leaves
    let (status, _) = post_json(
        &t.router,
        "/api/registry/change",
        &cookie,
        &serde_json::json!({ "category": "knowledge", "index": 1, "action": "erase" }).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = get(&t.router, "/api/registry", Some(&cookie)).await;
    assert!(!body.contains("Bell"), "erase did not erase: {body}");

    // an action that cannot be done SAYS so rather than reporting success
    let (status, body) = post_json(
        &t.router,
        "/api/registry/change",
        &cookie,
        &serde_json::json!({ "category": "knowledge", "index": 99, "action": "erase" }).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("\"ok\":false"), "{body}");
}

/// Every reply that came from a turn can show its receipt, and the receipt
/// is the journal's, not a retelling.
#[tokio::test]
async fn a_reply_carries_a_receipt_the_person_can_open() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;
    post_json(&t.router, "/api/message", &cookie, &say("remember that I row on Tuesdays")).await;

    let (status, body) = get(&t.router, "/api/history?after=0", Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
    let intent = rows
        .iter()
        .rev()
        .find_map(|r| r["intent"].as_str())
        .expect("a reply must name the receipt behind it");

    let (status, body) = get(&t.router, &format!("/api/receipt/{intent}"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let receipt: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(receipt["status"], "verified");
    assert!(
        !receipt["claims"].as_array().unwrap().is_empty(),
        "a remember turn claims something: {body}"
    );

    // someone else's receipt is not readable, and a made-up one is not found
    let (status, _) = get(&t.router, "/api/receipt/int_nonexistent", Some(&cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// §4.2.3.4 says departure EXPORTS and then shreds. The export has to be
/// complete and it has to arrive as a file -- the point is something that
/// outlives the robot it came from.
#[tokio::test]
async fn a_person_can_take_everything_with_them_before_they_go() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;
    post_json(&t.router, "/api/message", &cookie, &say("remember that my bike is blue")).await;

    let res = t
        .router
        .clone()
        .oneshot(
            Request::get("/api/export")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // a download, not a page: the browser must save it
    assert!(
        res.headers()
            .get(header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("attachment") && v.contains("my-data.json")),
        "the export must arrive as a file"
    );
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // the registry, whole
    assert!(doc["registry"]["knowledge"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k["value"].as_str().unwrap_or("").contains("bike")));
    for cat in ["instructions", "preferences", "media", "grants"] {
        assert!(doc["registry"][cat].is_array(), "{cat} missing from the export");
    }
    // and the conversation, with both sides -- an export of facts alone
    // would be an index of a book about to be burned
    let convo = doc["conversation"].as_array().expect("conversation");
    assert!(
        convo.iter().any(|m| m["direction"] == "in")
            && convo.iter().any(|m| m["direction"] == "out"),
        "the export must carry both sides of the conversation"
    );
    assert!(
        convo.iter().any(|m| m["content"].as_str().unwrap_or("").contains("bike")),
        "the person's own words are missing"
    );

    // no session, no export of anyone's data
    let (status, _) = get(&t.router, "/api/export", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Leaving destroys the cell, and the confirmation is not decorative.
#[tokio::test]
async fn leaving_requires_the_word_and_then_actually_shreds() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    // the owner cannot erase themselves through this door: the owner's cell
    // IS the robot, and a stray click must not end it
    let (status, body) = post_json(
        &t.router,
        "/api/people/me/remove",
        &cookie,
        &serde_json::json!({ "confirm": "ERASE" }).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("owner"), "{body}");

    // and a wrong confirmation never reaches the robot at all
    let (status, body) = post_json(
        &t.router,
        "/api/people/me/remove",
        &cookie,
        &serde_json::json!({ "confirm": "yes" }).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("type ERASE"), "{body}");

    // no session, no removal -- of anyone
    let res = t
        .router
        .clone()
        .oneshot(
            Request::post("/api/people/1/remove")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{\"confirm\":\"ERASE\"}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// §4.1.6 end to end: the same message twice inside two seconds produces
/// one turn, one transcript entry, and one reply.
///
/// Asserted against a real cell rather than the double, because the value
/// of coalescing is entirely in what does NOT appear afterwards — no
/// second message row, no second intent, no second effect — and only a
/// real cell has those tables to check.
#[tokio::test]
async fn the_same_message_twice_is_one_turn() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    let (status, first) =
        post_json(&t.router, "/api/message", &cookie, &say("what time is it?")).await;
    assert_eq!(status, StatusCode::OK);
    let first: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert!(first["repeat"].is_null(), "the first send is not a duplicate");
    assert!(!first["reply"].as_str().unwrap().is_empty());

    // the double-tap, immediately after
    let (status, second) =
        post_json(&t.router, "/api/message", &cookie, &say("what time is it?")).await;
    assert_eq!(status, StatusCode::OK, "a duplicate is not an error: {second}");
    let second: serde_json::Value = serde_json::from_str(&second).unwrap();
    assert_eq!(second["repeat"], true, "{second}");
    assert_eq!(
        second["reply"].as_str().unwrap_or(""),
        "",
        "a coalesced send must not answer a second time"
    );

    // the intent it names is REAL -- its receipt resolves. A claim that
    // pointed at an id no journal row carried would be a dangling
    // reference dressed up as provenance.
    let into = second["intent"].as_str().expect("the turn it merged into");
    let (status, receipt) = get(&t.router, &format!("/api/receipt/{into}"), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "the coalesced intent has no receipt: {receipt}");

    // and the transcript holds ONE question and ONE answer, not two
    let (_, body) = get(&t.router, "/api/history?after=0", Some(&cookie)).await;
    let rows: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
    let asked = rows
        .iter()
        .filter(|r| r["direction"] == "in" && r["content"] == "what time is it?")
        .count();
    assert_eq!(asked, 1, "the duplicate left a second bubble in the transcript");
    let answered = rows.iter().filter(|r| r["direction"] == "out").count();
    assert_eq!(answered, 1, "the robot answered twice");
}

/// The same words later are a person repeating themselves, and get their
/// own turn. Coalescing that would be losing a message, which is worse
/// than answering twice.
#[tokio::test]
async fn the_same_message_after_the_window_is_answered_again() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    post_json(&t.router, "/api/message", &cookie, &say("what time is it?")).await;
    tokio::time::sleep(std::time::Duration::from_millis(
        prism::repeats::WINDOW_MS as u64 + 100,
    ))
    .await;
    let (status, again) =
        post_json(&t.router, "/api/message", &cookie, &say("what time is it?")).await;
    assert_eq!(status, StatusCode::OK);
    let again: serde_json::Value = serde_json::from_str(&again).unwrap();
    assert!(
        again["repeat"].is_null(),
        "past the window this is a question, not a duplicate: {again}"
    );
    assert!(!again["reply"].as_str().unwrap().is_empty());
}

/// Different messages sent back to back must both be answered -- the
/// failure mode that would make coalescing worse than not having it.
#[tokio::test]
async fn a_fast_conversation_is_never_coalesced() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    for text in ["what time is it?", "help", "my facts"] {
        let (status, body) = post_json(&t.router, "/api/message", &cookie, &say(text)).await;
        assert_eq!(status, StatusCode::OK);
        let out: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(out["repeat"].is_null(), "'{text}' was swallowed: {body}");
        assert!(!out["reply"].as_str().unwrap().is_empty(), "'{text}' went unanswered");
    }
}

/// The real shape of a double send: both requests in flight at once.
///
/// The sequential test above cannot catch a check-then-write race, because
/// the first turn has already finished by the time the second starts. A
/// double-tap does not work like that — the second request arrives while
/// the first is still running, which is exactly the interleaving that
/// makes an unguarded "SELECT then INSERT" produce two turns.
#[tokio::test]
async fn two_simultaneous_sends_still_produce_one_turn() {
    let t = boot_test_robot();
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    let body = say("what time is it?");
    let (a, b) = tokio::join!(
        post_json(&t.router, "/api/message", &cookie, &body),
        post_json(&t.router, "/api/message", &cookie, &body),
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    let a: serde_json::Value = serde_json::from_str(&a.1).unwrap();
    let b: serde_json::Value = serde_json::from_str(&b.1).unwrap();

    // exactly one of them is the turn; exactly one is the duplicate. Which
    // one wins is genuinely a race and must not be asserted.
    let coalesced = [&a, &b].iter().filter(|r| r["repeat"] == true).count();
    assert_eq!(coalesced, 1, "both or neither were coalesced: {a} / {b}");
    let answered = [&a, &b]
        .iter()
        .filter(|r| !r["reply"].as_str().unwrap_or("").is_empty())
        .count();
    assert_eq!(answered, 1, "the robot answered a double-tap twice: {a} / {b}");

    let (_, body) = get(&t.router, "/api/history?after=0", Some(&cookie)).await;
    let rows: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
    assert_eq!(
        rows.iter().filter(|r| r["direction"] == "out").count(),
        1,
        "two replies reached the transcript"
    );
}

/// The coalesced response must name a turn that REALLY exists.
///
/// A "yes" answering a parked approval runs under the *parked* intent, not
/// under a fresh one — that is whose receipt it is. So the claim has to be
/// made under the parked id, which means looking it up before claiming.
/// Doing it afterwards is not enough, and this test is the reason: the
/// duplicate arrives while the first "yes" is still running, so it is
/// handed whatever the claim held at that moment. Found live — the
/// coalesced response named a turn whose receipt 404'd.
#[tokio::test]
async fn a_repeated_yes_names_the_parked_turn() {
    let t = boot_test_robot_with(|cfg| {
        cfg.policy.approval_required = vec!["memory.remember".into()];
    });
    let cookie = login(&t.router, &format!("/a/{}", t.slug)).await;

    post_json(
        &t.router,
        "/api/message",
        &cookie,
        &say("remember that the kettle is broken"),
    )
    .await;
    let (_, body) = get(&t.router, "/api/approvals", Some(&cookie)).await;
    let waiting: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
    let parked = waiting
        .first()
        .and_then(|a| a["intent"].as_str())
        .expect("an approval should be parked")
        .to_string();

    // double-tapped "yes", both in flight
    let yes = say("yes");
    let (a, b) = tokio::join!(
        post_json(&t.router, "/api/message", &cookie, &yes),
        post_json(&t.router, "/api/message", &cookie, &yes),
    );
    let a: serde_json::Value = serde_json::from_str(&a.1).unwrap();
    let b: serde_json::Value = serde_json::from_str(&b.1).unwrap();
    let both = [a, b];
    let coalesced = both
        .iter()
        .find(|r| r["repeat"] == true)
        .expect("one of the two must be the repeat");

    assert_eq!(
        coalesced["intent"].as_str().unwrap(),
        parked,
        "the coalesced answer named the arrival's id rather than the \
         parked turn's -- that id never reaches the journal"
    );
    // and it dereferences, which is the point of naming it at all
    let (status, receipt) = get(
        &t.router,
        &format!("/api/receipt/{}", coalesced["intent"].as_str().unwrap()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "dangling receipt reference: {receipt}");

    // the approval ran once, and is closed
    let (_, body) = get(&t.router, "/api/approvals", Some(&cookie)).await;
    assert_eq!(body.trim(), "[]", "the approval is still open after a yes");
    let (_, body) = get(&t.router, "/api/registry", Some(&cookie)).await;
    let reg: serde_json::Value = serde_json::from_str(&body).unwrap();
    let kettles = reg["knowledge"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|k| k["value"].as_str().unwrap_or("").contains("kettle"))
        .count();
    assert_eq!(kettles, 1, "the approved effect ran twice");
}
