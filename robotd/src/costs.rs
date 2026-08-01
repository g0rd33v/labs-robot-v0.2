//! `robotd cost` — the §2b cost model, measured (gap item 12).
//!
//! §2b's table is estimates over assumptions marked **TO-VERIFY**. The
//! meter (`model_calls` in core.db) records what every call actually was:
//! tokens in and out, cache hits, the provider's own charge, latency. This
//! reads it back per seat, which turns the two numbers the doc says the
//! vendor total swings ±40% on — calls per turn, escalation shares — into
//! queries instead of guesses.
//!
//! Cost figures come from the provider's accounting (`usage.cost`), never a
//! local price table: prices drift, and a stale table is an estimate
//! wearing a measurement's clothes. A call with no reported cost is shown
//! as unpriced rather than priced wrongly.

use rusqlite::Connection;

#[derive(Debug, Default)]
struct SeatRow {
    role: String,
    calls: i64,
    prompt: i64,
    completion: i64,
    cached: i64,
    cost: f64,
    priced_calls: i64,
    p50_ms: i64,
    p95_ms: i64,
}

fn percentile(sorted: &[i64], p: f64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// The report, over the last `days` days.
pub fn report(core: &Connection, days: i64) -> anyhow::Result<String> {
    let since = trust::ids::ts_ms() - days * 24 * 60 * 60 * 1000;
    let mut stmt = core.prepare(
        "SELECT role, prompt_tokens, completion_tokens, cached_tokens, cost_usd, latency_ms, \
         first_token_ms FROM model_calls WHERE ts > ?1 ORDER BY role",
    )?;
    let mut seats: std::collections::BTreeMap<String, (SeatRow, Vec<i64>)> = Default::default();
    let rows = stmt.query_map([since], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Option<f64>>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, Option<i64>>(6)?,
        ))
    })?;
    let mut ttft: std::collections::BTreeMap<String, Vec<i64>> = Default::default();
    for row in rows {
        let (role, p, c, cached, cost, lat, first) = row?;
        let entry = seats.entry(role.clone()).or_default();
        entry.0.role = role.clone();
        entry.0.calls += 1;
        entry.0.prompt += p;
        entry.0.completion += c;
        entry.0.cached += cached;
        if let Some(usd) = cost {
            entry.0.cost += usd;
            entry.0.priced_calls += 1;
        }
        entry.1.push(lat);
        if let Some(f) = first {
            ttft.entry(role).or_default().push(f);
        }
    }

    if seats.is_empty() {
        return Ok(format!(
            "the meter is empty for the last {days} day(s) -- no model calls, \
             or a build from before the meter existed."
        ));
    }

    let mut out = format!(
        "the meter -- every model call, last {days} day(s) (sec 2b, measured):\n\n\
         {:<11} {:>6} {:>10} {:>9} {:>7} {:>9} {:>7} {:>7}\n",
        "seat", "calls", "in-tok", "out-tok", "cache%", "usd", "p50ms", "p95ms"
    );
    let (mut tp, mut tc, mut tcached, mut tcost, mut tcalls, mut tpriced) =
        (0i64, 0i64, 0i64, 0f64, 0i64, 0i64);
    for (_, (mut seat, mut lats)) in seats {
        lats.sort_unstable();
        seat.p50_ms = percentile(&lats, 0.50);
        seat.p95_ms = percentile(&lats, 0.95);
        let cache_pct = if seat.prompt > 0 {
            100.0 * seat.cached as f64 / seat.prompt as f64
        } else {
            0.0
        };
        let usd = if seat.priced_calls > 0 {
            format!("{:.4}", seat.cost)
        } else {
            "unpriced".into()
        };
        out.push_str(&format!(
            "{:<11} {:>6} {:>10} {:>9} {:>6.1}% {:>9} {:>7} {:>7}\n",
            seat.role, seat.calls, seat.prompt, seat.completion, cache_pct, usd,
            seat.p50_ms, seat.p95_ms
        ));
        tp += seat.prompt;
        tc += seat.completion;
        tcached += seat.cached;
        tcost += seat.cost;
        tcalls += seat.calls;
        tpriced += seat.priced_calls;
    }
    let total_cache = if tp > 0 {
        100.0 * tcached as f64 / tp as f64
    } else {
        0.0
    };
    out.push_str(&format!(
        "\ntotal: {tcalls} calls, {tp} in / {tc} out tokens, cache-hit {total_cache:.1}% \
         of input, ${tcost:.4} across {tpriced} priced calls\n"
    ));
    if tpriced < tcalls {
        out.push_str(&format!(
            "({} calls carried no provider cost figure and are counted but unpriced)\n",
            tcalls - tpriced
        ));
    }
    // sec 2c: TTFT exists only where a call streamed; absence is honest
    for (role, mut f) in ttft {
        f.sort_unstable();
        out.push_str(&format!(
            "ttft[{role}]: p50 {}ms, p95 {}ms over {} streamed calls\n",
            percentile(&f, 0.50),
            percentile(&f, 0.95),
            f.len()
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        trust::schema::init_core(&conn).unwrap();
        conn
    }

    fn call(conn: &Connection, role: &str, p: i64, c: i64, cached: i64, cost: Option<f64>, lat: i64) {
        conn.execute(
            "INSERT INTO model_calls(ts, role, model, prompt_tokens, completion_tokens, \
             cached_tokens, cost_usd, latency_ms) VALUES (?1,?2,'m',?3,?4,?5,?6,?7)",
            rusqlite::params![trust::ids::ts_ms(), role, p, c, cached, cost, lat],
        )
        .unwrap();
    }

    /// The report is arithmetic over the meter, and it says when it is
    /// NOT priced rather than inventing a price.
    #[test]
    fn the_report_aggregates_per_seat_and_admits_unpriced_calls() {
        let c = core();
        call(&c, "route", 1000, 100, 800, Some(0.001), 900);
        call(&c, "route", 1000, 100, 900, Some(0.001), 1100);
        call(&c, "answer", 3000, 350, 0, None, 2500);

        let r = report(&c, 7).unwrap();
        assert!(r.contains("route"), "{r}");
        assert!(r.contains("answer"));
        // cache-hit: route cached 1700 of 2000 prompt tokens
        assert!(r.contains("85.0%"), "route cache rate: {r}");
        assert!(r.contains("$0.0020"), "summed provider cost: {r}");
        assert!(r.contains("unpriced"), "the unpriced seat says so: {r}");
        assert!(r.contains("1 calls carried no provider cost"), "{r}");
    }

    #[test]
    fn an_empty_meter_says_so_plainly() {
        let c = core();
        let r = report(&c, 7).unwrap();
        assert!(r.contains("meter is empty"), "{r}");
    }
}
