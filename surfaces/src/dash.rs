//! Dashboard-lite (arch sec 10a): the owner's control room, served by the
//! binary. Q35 stack: server-rendered HTML, zero build chain, zero
//! node_modules. Three panels for the MVP: Overview, Registry, Boundary.

#[derive(Debug, Default, Clone)]
pub struct DashData {
    pub robot_id: String,
    pub robot_name: String,
    pub started_at: i64,
    pub now: i64,
    pub gateway_online: bool,
    pub search_online: bool,
    pub embedder_online: bool,
    pub cast_answer: String,
    pub cast_verdict: String,
    /// (id, kind, display_name, status)
    pub principals: Vec<(i64, String, String, String)>,
    pub message_count: i64,
    pub fact_count: i64,
    pub active_reminders: i64,
    pub boundary_count: i64,
    /// Result of re-hashing the whole chain at read time.
    pub boundary_chain_ok: bool,
    /// (content, source snippet, learned ts)
    pub facts: Vec<(String, String, i64)>,
    /// (ts, direction, channel, counterparty, purpose, size)
    pub boundary: Vec<(i64, String, String, String, String, i64)>,
    /// Conversations (panel 3), self only for now: (ts, direction, snippet)
    pub conversations: Vec<(i64, String, String)>,
    /// Commitments (panel 5): open (what, kind, due_ms)
    pub commitments_open: Vec<(String, String, Option<i64>)>,
    /// and recently closed (what, status, why)
    pub commitments_closed: Vec<(String, String, String)>,
    /// Receipts (panel 6): (ts-ish intent, status, claim)
    pub receipts: Vec<(String, String, String)>,
    /// Hub (panel 7): (connector, status line) -- no secrets, ever
    pub hub: Vec<(String, String)>,
    /// Models & Routing (panel 8): (seat, model) from the cast
    pub cast: Vec<(String, String)>,
    /// and the meter per seat, today: (seat, calls, cache%, usd, p50ms, ttft_p50ms)
    pub meter: Vec<(String, i64, f64, f64, i64, Option<i64>)>,
    /// Soul (panel 9)
    pub soul_stance: String,
    /// (dimension, value, floor, ceiling, pinned)
    pub soul_dial: Vec<(String, i64, i64, i64, bool)>,
    pub soul_evolution: bool,
    /// (created_at, reason, applied)
    pub soul_revisions: Vec<(i64, String, bool)>,
    /// System (panel 10)
    pub instance_id: String,
    pub version: String,
    pub vault_objects: i64,
    pub files_count: i64,
    pub standing_rules: i64,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn fmt_ts(ms: i64) -> String {
    // HH:MM:SS UTC without pulling chrono into surfaces
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60)
}

pub fn render(d: &DashData) -> String {
    let uptime_min = (d.now - d.started_at) / 60_000;
    let onoff = |b: bool| {
        if b {
            "<span class=ok>online</span>"
        } else {
            "<span class=off>offline</span>"
        }
    };

    let principals_rows: String = d
        .principals
        .iter()
        .map(|(id, kind, name, status)| {
            format!(
                "<tr><td>{id}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(kind),
                esc(name),
                esc(status)
            )
        })
        .collect();

    let fact_rows: String = if d.facts.is_empty() {
        "<tr><td colspan=3 class=dim>no facts stored</td></tr>".into()
    } else {
        d.facts
            .iter()
            .enumerate()
            .map(|(i, (content, src, ts))| {
                format!(
                    "<tr><td>{}</td><td>{}</td><td class=dim>\"{}\" &middot; {}</td></tr>",
                    i + 1,
                    esc(content),
                    esc(src),
                    fmt_ts(*ts)
                )
            })
            .collect()
    };

    let boundary_rows: String = d
        .boundary
        .iter()
        .map(|(ts, dir, channel, counterparty, purpose, size)| {
            format!(
                "<tr><td class=dim>{}</td><td class={}>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=dim>{size}B</td></tr>",
                fmt_ts(*ts),
                if dir == "in" { "ok" } else { "outb" },
                dir,
                esc(channel),
                esc(counterparty),
                esc(purpose),
            )
        })
        .collect();

    let convo_rows: String = if d.conversations.is_empty() {
        "<tr><td colspan=2 class=dim>no conversation yet</td></tr>".into()
    } else {
        d.conversations
            .iter()
            .map(|(_, dir, snip)| {
                format!(
                    "<tr><td class={}>{}</td><td>{}</td></tr>",
                    if dir == "in" { "ok" } else { "outb" },
                    if dir == "in" { "you" } else { "robot" },
                    esc(snip)
                )
            })
            .collect()
    };

    let open_rows: String = if d.commitments_open.is_empty() {
        "<tr><td colspan=3 class=dim>nothing owed right now</td></tr>".into()
    } else {
        d.commitments_open
            .iter()
            .map(|(what, kind, due)| {
                format!(
                    "<tr><td>{}</td><td class=dim>{}</td><td class=dim>{}</td></tr>",
                    esc(what),
                    esc(kind),
                    due.map(fmt_ts).unwrap_or_default()
                )
            })
            .collect()
    };
    let closed_rows: String = if d.commitments_closed.is_empty() {
        "<tr><td colspan=3 class=dim>nothing closed yet</td></tr>".into()
    } else {
        d.commitments_closed
            .iter()
            .map(|(what, status, why)| {
                format!(
                    "<tr><td>{}</td><td class=dim>{}</td><td>{}</td></tr>",
                    esc(what),
                    esc(status),
                    esc(why)
                )
            })
            .collect()
    };

    let receipt_rows: String = if d.receipts.is_empty() {
        "<tr><td colspan=3 class=dim>no receipts yet</td></tr>".into()
    } else {
        d.receipts
            .iter()
            .map(|(intent, status, claim)| {
                format!(
                    "<tr><td class=dim>{}</td><td class={}>{}</td><td>{}</td></tr>",
                    esc(intent),
                    if status == "verified" { "ok" } else { "outb" },
                    esc(status),
                    esc(claim)
                )
            })
            .collect()
    };

    let hub_rows: String = d
        .hub
        .iter()
        .map(|(name, status)| {
            format!("<tr><td>{}</td><td class=dim>{}</td></tr>", esc(name), esc(status))
        })
        .collect();

    let cast_rows: String = d
        .cast
        .iter()
        .map(|(seat, model)| {
            format!("<tr><td>{}</td><td class=dim>{}</td></tr>", esc(seat), esc(model))
        })
        .collect();
    let meter_rows: String = if d.meter.is_empty() {
        "<tr><td colspan=6 class=dim>no calls in the last 24h</td></tr>".into()
    } else {
        d.meter
            .iter()
            .map(|(seat, calls, cache, usd, p50, ttft)| {
                format!(
                    "<tr><td>{}</td><td>{calls}</td><td class=dim>{cache:.1}%</td>\
                     <td class=dim>${usd:.4}</td><td class=dim>{p50}ms</td><td class=dim>{}</td></tr>",
                    esc(seat),
                    ttft.map(|t| format!("{t}ms")).unwrap_or_else(|| "--".into())
                )
            })
            .collect()
    };

    let dial_rows: String = d
        .soul_dial
        .iter()
        .map(|(dim, value, floor, ceiling, pinned)| {
            format!(
                "<tr><td>{}</td><td><b>{value}</b></td><td class=dim>{floor}..{ceiling}{}</td></tr>",
                esc(dim),
                if *pinned { " (pinned)" } else { "" }
            )
        })
        .collect();
    let rev_rows: String = if d.soul_revisions.is_empty() {
        "<tr><td colspan=3 class=dim>no revisions yet -- nightly self-revision (S3) \
         waits on the owner's numbers</td></tr>"
            .into()
    } else {
        d.soul_revisions
            .iter()
            .map(|(ts, reason, applied)| {
                format!(
                    "<tr><td class=dim>{}</td><td>{}</td><td class=dim>{}</td></tr>",
                    fmt_ts(*ts),
                    esc(reason),
                    if *applied { "applied" } else { "proposed" }
                )
            })
            .collect()
    };

    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>bender / dashboard</title>
<style>
  :root {{ color-scheme: dark; }}
  body {{ margin:0; background:#0d1117; color:#e6edf3;
         font:14px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; }}
  header {{ padding:10px 20px; border-bottom:1px solid #21262d; font-weight:600;
            color:#7ee787; letter-spacing:.04em; }}
  header small {{ color:#8b949e; font-weight:400; margin-left:8px; }}
  header a {{ color:#58a6ff; float:right; text-decoration:none; }}
  main {{ padding:20px; max-width:1080px; margin:0 auto; }}
  h2 {{ font-size:13px; text-transform:uppercase; letter-spacing:.1em;
        color:#8b949e; margin:28px 0 10px; }}
  .cards {{ display:grid; grid-template-columns:repeat(auto-fill,minmax(160px,1fr)); gap:10px; }}
  .card {{ background:#161b22; border:1px solid #21262d; border-radius:10px; padding:12px 14px; }}
  .card b {{ display:block; font-size:20px; }}
  .card span.lbl {{ color:#8b949e; font-size:12px; }}
  table {{ width:100%; border-collapse:collapse; background:#161b22;
           border:1px solid #21262d; border-radius:10px; overflow:hidden; }}
  th {{ text-align:left; font-size:11px; text-transform:uppercase; letter-spacing:.06em;
        color:#8b949e; padding:8px 10px; border-bottom:1px solid #21262d; }}
  td {{ padding:7px 10px; border-bottom:1px solid #1c2128; vertical-align:top; }}
  .dim {{ color:#8b949e; font-size:12px; }}
  .ok {{ color:#7ee787; }} .off {{ color:#f85149; }} .outb {{ color:#d29922; }}
</style></head><body>
<header>bender<small>dashboard &middot; the control room</small><a href="__PREFIX__/chat">&larr; chat</a></header>
<main>
<h2>overview</h2>
<div class="cards">
  <div class=card><span class=lbl>robot</span><b>{name}</b><span class=dim>{id}</span></div>
  <div class=card><span class=lbl>uptime</span><b>{uptime_min}m</b></div>
  <div class=card><span class=lbl>model gateway</span><b>{gw}</b><span class=dim>{verdict} / {answer}</span></div>
  <div class=card><span class=lbl>web search</span><b>{search}</b></div>
  <div class=card><span class=lbl>embeddings</span><b>{embed}</b></div>
  <div class=card><span class=lbl>messages</span><b>{msgs}</b></div>
  <div class=card><span class=lbl>facts</span><b>{facts_n}</b></div>
  <div class=card><span class=lbl>active reminders</span><b>{rems}</b></div>
  <div class=card><span class=lbl>boundary crossings</span><b>{bnd}</b><span class=dim>{chain}</span></div>
</div>

<h2>people</h2>
<table><tr><th>id</th><th>role</th><th>name</th><th>status</th></tr>{principals}</table>

<h2>registry (pims) -- every fact and its source</h2>
<table><tr><th>#</th><th>fact</th><th>source (your words)</th></tr>{facts_rows}</table>

<h2>conversations -- your last {convo_n} turns (others by policy, later)</h2>
<table><tr><th>who</th><th>message</th></tr>{convos}</table>

<h2>commitments -- the second law as a screen</h2>
<table><tr><th>still owed</th><th>kind</th><th>due</th></tr>{copen}</table>
<p></p>
<table><tr><th>recently closed</th><th>status</th><th>why</th></tr>{cclosed}</table>

<h2>receipts -- every action's evidence</h2>
<table><tr><th>intent</th><th>status</th><th>claim</th></tr>{receipts}</table>

<h2>boundary log -- last {bshown} crossings (every byte in and out)</h2>
{chainbanner}
<table><tr><th>ts</th><th>dir</th><th>channel</th><th>counterparty</th><th>purpose</th><th>size</th></tr>{brows}</table>

<h2>hub -- every connector, no secret ever displayed</h2>
<table><tr><th>connector</th><th>status</th></tr>{hub}</table>

<h2>models &amp; routing -- the cast, and what each seat did (24h)</h2>
<table><tr><th>seat</th><th>model</th></tr>{cast}</table>
<p></p>
<table><tr><th>seat</th><th>calls</th><th>cache</th><th>usd</th><th>avg</th><th>ttft</th></tr>{meter}</table>

<h2>soul -- speaking as: {stance} &middot; self-adjustment {evolution}</h2>
<table><tr><th>dimension</th><th>value</th><th>bounds</th></tr>{dial}</table>
<p></p>
<table><tr><th>when</th><th>revision</th><th>state</th></tr>{revs}</table>

<h2>system</h2>
<div class="cards">
  <div class=card><span class=lbl>version</span><b>{version}</b></div>
  <div class=card><span class=lbl>instance</span><b class=dim>{instance}</b></div>
  <div class=card><span class=lbl>vault objects</span><b>{vault_n}</b><span class=dim>{files_n} named files</span></div>
  <div class=card><span class=lbl>standing rules</span><b>{rules_n}</b></div>
</div>
<p class=dim>export: `robotd package` &middot; backups: `robotd backup` &middot; updates:
`robotd update --check` (signed; unsigned never installs) &middot; recovery:
`robotd recovery-kit` -- if the passphrase and the kit are both lost, the data
is gone, by design.</p>
</main></body></html>"#,
        name = esc(&d.robot_name),
        id = esc(&d.robot_id),
        gw = onoff(d.gateway_online),
        verdict = esc(&d.cast_verdict),
        answer = esc(&d.cast_answer),
        search = onoff(d.search_online),
        embed = onoff(d.embedder_online),
        msgs = d.message_count,
        facts_n = d.fact_count,
        rems = d.active_reminders,
        bnd = d.boundary_count,
        chain = if d.boundary_chain_ok {
            "<span class=ok>chain verified</span>"
        } else {
            "<span class=off>CHAIN BROKEN</span>"
        },
        principals = principals_rows,
        facts_rows = fact_rows,
        bshown = d.boundary.len(),
        chainbanner = if d.boundary_chain_ok {
            String::new()
        } else {
            "<p class=off>the hash chain does not verify: the record below cannot be              trusted as complete. this is reported, not hidden.</p>".to_string()
        },
        brows = boundary_rows,
        convo_n = d.conversations.len(),
        convos = convo_rows,
        copen = open_rows,
        cclosed = closed_rows,
        receipts = receipt_rows,
        hub = hub_rows,
        cast = cast_rows,
        meter = meter_rows,
        stance = esc(&d.soul_stance),
        evolution = if d.soul_evolution { "on" } else { "off" },
        dial = dial_rows,
        revs = rev_rows,
        version = esc(&d.version),
        instance = esc(&d.instance_id),
        vault_n = d.vault_objects,
        files_n = d.files_count,
        rules_n = d.standing_rules,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_escapes_and_includes_sections() {
        let d = DashData {
            robot_name: "bender".into(),
            robot_id: "robot_x".into(),
            facts: vec![("<script>x</script>".into(), "said".into(), 1)],
            boundary: vec![(1, "in".into(), "chat".into(), "local".into(), "conv".into(), 5)],
            boundary_chain_ok: true,
            principals: vec![(1, "owner".into(), "owner".into(), "active".into())],
            ..Default::default()
        };
        let html = render(&d);
        assert!(html.contains("registry"));
        assert!(html.contains("boundary log"));
        assert!(!html.contains("<script>x</script>")); // escaped
        assert!(html.contains("&lt;script&gt;"));
    }
}
