//! `robotd health` — §13e's post-upgrade check, callable any time.
//!
//! *"a failed post-upgrade health check (boot + journal replay + one
//! synthetic turn) rolls back automatically"* — this is that check. It
//! runs in the STAGED binary during `update --apply`, so what gets
//! promoted is the version that proved it can boot this robot's actual
//! data, not the version that compiled.
//!
//! Also `robotd loadtest` (M5's gate, scaled to a harness): synthetic
//! turns through the full governed pipeline — real cell, real journal,
//! real receipts — with the one number that defines the gate: **zero
//! dropped intents**. An intent is dropped if it is left open with no
//! terminal receipt; the harness counts them structurally rather than
//! trusting its own bookkeeping.

use anyhow::bail;
use prism::verdict::FallbackVerdict;
use prism::{Envelope, TurnDeps};
use std::time::Instant;

/// Boot the stores read-only-ish, verify the chain, run one synthetic
/// floor turn in a scratch cell. Exit code is the whole interface.
pub fn health(cfg: &crate::config::RobotConfig) -> anyhow::Result<i32> {
    let data_dir = std::path::Path::new(&cfg.robot.data_dir);
    // 1. keys + core open + chain verification (boot's own first moves)
    let keys = trust::keys::KeyChain::load_or_create(&data_dir.join("kek.key"))?;
    let core = trust::cells::open_encrypted(&data_dir.join("core.db"), &keys.core_db_key())?;
    trust::schema::init_core(&core)?;
    if !trust::boundary::verify_chain(&core)? {
        println!("health: FAIL -- boundary chain does not verify");
        return Ok(1);
    }
    let crossings = trust::boundary::count(&core)?;

    // 2. one synthetic governed turn, floor class, scratch cell -- proves
    // schema init, journal, receipts and rendering all function in THIS
    // binary against THIS machine
    let scratch = std::env::temp_dir().join(format!("health-{}.db", trust::ids::random_hex(6)));
    let conn = rusqlite::Connection::open(&scratch)?;
    prism::init_cell_schema(&conn)?;
    mind::init_cell_schema(&conn)?;
    soul::init_cell_schema(&conn)?;
    let cell = prism::Cell::new(conn);
    let registry = crate::caps::Registry::offline();
    let speak = crate::render::Speak::offline();
    let deps = TurnDeps {
        router: &registry,
        verdicts: &FallbackVerdict,
        renderer: &speak,
        crash: None,
        standing: None,
        on_early: None,
    };
    let msg_id = cell.with(|c| {
        mind::record_message(c, "in", "health", "what time is it?").map_err(crate::caps::mind_err)
    })?;
    let env = Envelope {
        surface: "health".into(),
        principal_id: 0,
        modality: "text".into(),
        content: "what time is it?".into(),
        ts: trust::ids::ts_ms(),
        device_trust: "session".into(),
        source_msg_id: Some(msg_id),
    };
    let out = prism::run_turn(&cell, &env, &deps)?;
    let _ = std::fs::remove_file(&scratch);
    if out.reply.trim().is_empty() {
        println!("health: FAIL -- the synthetic turn produced no reply");
        return Ok(1);
    }

    println!(
        "health: OK -- chain verified ({crossings} crossings), synthetic turn \
         answered, version {}",
        env!("CARGO_PKG_VERSION")
    );
    Ok(0)
}

/// M5's gate as a harness. `--turns N` synthetic floor turns at full speed
/// through the governed pipeline; the bar that matters is dropped == 0.
pub fn loadtest(turns: usize) -> anyhow::Result<i32> {
    let scratch = std::env::temp_dir().join(format!("load-{}.db", trust::ids::random_hex(6)));
    let conn = rusqlite::Connection::open(&scratch)?;
    prism::init_cell_schema(&conn)?;
    mind::init_cell_schema(&conn)?;
    soul::init_cell_schema(&conn)?;
    let cell = prism::Cell::new(conn);
    let registry = crate::caps::Registry::offline();
    let speak = crate::render::Speak::offline();
    let deps = TurnDeps {
        router: &registry,
        verdicts: &FallbackVerdict,
        renderer: &speak,
        crash: None,
        standing: None,
        on_early: None,
    };

    // the floor's own repertoire, cycled -- reads and writes both
    let scripts = [
        "what time is it?",
        "remind me in 90 minutes to stretch",
        "my reminders",
        "cancel the last reminder",
        "help",
        "/commitments",
    ];
    println!("loadtest: {turns} governed turns, one writer lane (the cellular unit of sec 2a)...");
    let started = Instant::now();
    let mut latencies: Vec<u64> = Vec::with_capacity(turns);
    for i in 0..turns {
        let text = scripts[i % scripts.len()];
        let msg_id = cell.with(|c| {
            mind::record_message(c, "in", "load", text).map_err(crate::caps::mind_err)
        })?;
        let env = Envelope {
            surface: "load".into(),
            principal_id: 0,
            modality: "text".into(),
            content: text.into(),
            ts: trust::ids::ts_ms(),
            device_trust: "session".into(),
            source_msg_id: Some(msg_id),
        };
        let t = Instant::now();
        let out = prism::run_turn(&cell, &env, &deps)?;
        latencies.push(t.elapsed().as_micros() as u64);
        if out.reply.is_empty() {
            bail!("turn {i} produced an empty reply");
        }
        if i % 5000 == 0 && i > 0 {
            println!("  {i} turns...");
        }
    }
    let wall = started.elapsed();

    // THE gate: an intent without a terminal state is a dropped request.
    // Counted from the journal, not from this function's own loop -- the
    // journal is what a dropped intent cannot hide from.
    let open = cell.with(prism::journal::open_intents)?;
    let dropped = open.len();

    latencies.sort_unstable();
    let pct = |p: f64| latencies[((latencies.len() as f64 - 1.0) * p) as usize] as f64 / 1000.0;
    let per_sec = turns as f64 / wall.as_secs_f64();
    let day_capacity = per_sec * 86_400.0;
    let _ = std::fs::remove_file(&scratch);

    println!(
        "\nloadtest: {turns} turns in {:.1}s -- {per_sec:.0} turns/sec sustained \
         ({day_capacity:.0}/day capacity on ONE lane; M5's gate is 100K/day \
         across all lanes = {:.1}x headroom)\n\
         latency p50 {:.2}ms  p95 {:.2}ms  p99 {:.2}ms\n\
         dropped intents: {dropped} (bar: 0) {}",
        wall.as_secs_f64(),
        day_capacity / 100_000.0,
        pct(0.50),
        pct(0.95),
        pct(0.99),
        if dropped == 0 { "" } else { "FAIL" }
    );
    if dropped > 0 {
        for id in open.iter().take(5) {
            println!("  open intent: {id}");
        }
    }
    Ok(i32::from(dropped != 0))
}
