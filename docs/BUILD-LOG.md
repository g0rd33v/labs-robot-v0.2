# BUILD LOG — Bender MVP

One entry per milestone: what shipped, the gate demo, assumptions made,
dependencies introduced. Newest first.

---

## Two-way sync between instances (2026-07-31)

Plan, decisions and the one deviation: `docs/PLAN-two-way-sync.md`.
The Robot Package (§8) carries a robot somewhere. This keeps two copies in
agreement afterwards — a change of scope, taken by the owner, not an
implementation detail of the existing mechanism.

### Knowledge syncs; history does not

Messages, facts, reminders and media travel both ways. The journal,
receipts, outbox, pending confirmations and above all the **Boundary Log**
stay where they happened. That last one settles the argument: it is a hash
chain, and two chains have no merge that is still a chain. Each instance
keeps its own and each remains independently verifiable — a stronger claim
than a stitched-together history that neither machine could support.

### Three rules, each picked so the worse failure cannot happen

**A deletion beats everything.** Tombstones carry an id and a moment, never
content. They are applied *before* any insert in the same pass, and a
tombstoned id is never re-inserted. They are collected once both sides have
applied them — which is precisely why the pass is two-way in one call: a
one-way push could never know the peer had applied anything, so it could
never safely forget that it had deleted something.

**Conflicting edits both survive.** The registry already models correction
as supersession rather than overwrite, so two machines correcting the same
fact produce two chains over one ancestor. Keeping both loses nothing and
stays inspectable. Last-writer-wins would discard an edit and stake it on
two machines agreeing about the time.

**A terminal reminder state wins.** `cancelled` or `fired` beats `active`,
whatever the timestamps say. Nagging about something called off is worse
than doing nothing.

### The laws, at a new boundary

- **Law 5** travels with the fact: a fact whose source message did not
  arrive is refused, so provenance can never point at words that are not
  there. Tested.
- **Law 2**: only cells both sides hold the key for. A cell present on one
  side only is reported as skipped, never invented.
- **Law 3**: both directions are boundary-logged — **counts only**. A sync
  log naming contents would be a second copy of the memory, in the log.
- **No new plaintext exists.** The peer's cells are already SQLCipher under
  the same KEK, so this merges in place. The planned sealed-delta file
  would have invented a format and left a decrypted-in-transit artifact on
  the very stick most likely to be lost.

### A real bug the first live run caught

**Restore carried the `instance_id`**, so a restored copy believed it was
the original — and refused to sync with it ("that is this same instance").
Restore now clears it and the copy mints its own on first boot. A peer
restored *before* instance ids existed gets one minted for it on contact:
without that its id differed on every sync, so the watermark never
advanced and every sweep re-scanned from the beginning.

### Gate (demonstrated)

- `cargo test --workspace` — **127 passed** (was 114)
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `robotd eval` — PASS

Unit tests on the merge rules, and five integration tests that boot two
real instances, use them apart, and sync: convergence, no resurrection, a
cancelled reminder staying cancelled, a different robot being refused, and
a repeat sync being a genuine no-op — which is what makes a lane that runs
every few minutes safe.

Those integration tests **serialise deliberately**. Booting registers the
sqlite-vec auto-extension, a process-global SQLite mutation; it is
`Once`-guarded, but one thread registering while another opens a
connection is still a race, and it surfaced as "file is not a database"
from an unrelated open. Production never meets it — `bootstrap` runs once
per process, before any cell opens — so the tests serialise rather than the
code pretending to be re-entrant.

### Live, between the real main folder and the real stick

Each side was taught something the other had never heard. The lane, running
on its own schedule, converged them without being asked:

```
synced with inst_61a8360…: pulled 3 rows, pushed 3 rows, 2 cells
```

Both registries then held both facts. A fact deleted on the main machine
was gone from the stick at the next sweep (`pulled 8 rows (1 deletions)`)
and **stayed gone** across three further syncs in both directions.

### Automatic, and quiet about absence

The owner chose automatic-whenever-present. A peer that is not plugged in
is skipped **silently** — absence is the normal state of a removable disk,
and a robot that complains every ten minutes about a drawer is one people
stop reading. A peer that is present but *unusable* is reported in chat,
as is any sync that actually moved something: the owner's memory crossing
between machines is not a thing to do quietly.

**A defect only the browser could show.** Watching the chat, the notices
repeated: `pulled 1 rows`, over and over, with nothing happening. **The
sync notice is itself a chat message, so it synced, so the next sweep had
a row to move, so it announced itself again** — a trickle that sustains
itself forever and makes the log look busy while the robot is idle. No
test caught it, because every test asserted on state and this was a
property of the *conversation*. The lane now announces when **knowledge**
moves — facts, reminders, media, deletions — and lets the transcript catch
up quietly. Verified after: four sweeps, zero notices; then a real fact,
and both sides announced it.

**Config.** `[sync] peers = [...]`, `every_minutes = 10`. `robotd sync
--with <path>` always works regardless.

**No new dependencies.** `serde` was added to `mind`, already in the
workspace.

---

## Deep review + the two-location test (2026-07-31)

### Review: four defects, found by reading and by running

**1. The hedge was firing on every turn.** Measured a non-floor turn at
**9.2 s** against a hedge deadline of 2.5 s — so essentially every
non-English turn was firing a duplicate routing request and paying twice
for the most expensive call in it. That deadline was sized for a one-line
doorman classification, not for a prompt carrying the whole catalog.
Routing now has its own (`route_hedge_after_ms`, 8 s): it still protects
the tail we actually observed, without doubling the median.

**2. A "yes" that could not be spent said nothing.** The confirmation is
deliberately spent *before* the call is planned — between a double-
submitted yes and a double deletion, the deletion is far worse. But the
leftover cases fell through to small talk, so someone who said yes twice
got a chat reply while believing they had confirmed a deletion. Now
answered deterministically (`confirmation.stale`).

**Found by running it, not by reading it:** on the stick, a second yes
produced sensible model prose from conversation history — which meant the
path just built could never fire, because the answering tool is only in
the catalog while a question is *open*, and that is exactly when it is
absent. The tool is now offered while a question is open **and briefly
after it closes**, so a late yes reaches the deterministic answer instead
of being settled by prose.

**3. The renderer was not admitting what it disclosed.** English is
rendered here, from local templates: nothing about the person leaves the
machine to make a sentence. Every other language is rendered by a model —
which means the slots go with it: their facts, their reminders, the
sources in their registry. A system that logs every byte crossing the
boundary should not be quiet about the one it causes itself. `Renderer`
now returns what it disclosed and the turn journals `render.disclosed`.

**4. The rendering-id list could drift.** It was hand-written. It now scans
every crate for construction sites and demands a template for each, so a
new capability cannot ship a reply that renders as a bare `[id]`.

Verified while reading rather than assumed: `forget_by_index` is op-marker
guarded, so a replayed confirmation cannot delete twice; and every cell
open runs `init_cell_schema`, so a cell restored from an older package
gains `pending_calls` on first use.

Two eval-corpus expectations were also wrong — calibrated to an earlier,
over-inclusive model output. Korean `내가 녹차를 마신다는 걸` carries the
nominaliser attaching the clause to "remember": request framing, not the
fact. The tighter description correctly drops it. Expectations are now the
semantic core, so a tight answer and a slightly wide one both pass.

### The two-location test

| | main folder | USB folder |
| --- | --- | --- |
| `cargo test --workspace` | **114 passed** | (built from the same tree) |
| `cargo clippy -D warnings` | clean | — |
| `robotd eval` | **PASS** — routing 66/0, kill-suite 12/12, floor p95 2.0 ms | **PASS** — 66/0, 12/12, p95 1.5 ms |
| `robotd eval --live` | **PASS** — 60 multilingual / **0 misroutes** / 0 not-their-words / 0 timeouts; injection 69 calls, 0 leaks | **PASS** — 60 / 1 misroute (bar ≤3) / 0 not-their-words; injection 69 calls, 0 leaks |
| starts locally | 7777 | **7788** |

**Transfer.** `robotd package` from the main folder (sealed, one-time code
printed separately) → `robotd restore --into` the USB folder: *2 cells,
integrity ok*. Same `robot_id`, **same slug token** — identity carried,
not regenerated. The old data was moved aside rather than deleted.

**It has what the main robot knows.** From the stick: all three facts with
their sources and their Russian intact, and the reminders made on the main
machine that morning — including *both* Turkish ones, from before and
after the description fix.

**It works from the stick**, not just remembers: the English floor answers
offline; the confirmation gate fires in Russian, declines, and rejects a
late yes deterministically; and Thai — a language used nowhere in this
project, ever — created a real reminder:

```
พรุ่งนี้ 9 โมงเช้าเตือนให้ผมโทรหาหมอ
→ เรียบร้อย -- เดี๋ยวจะเตือนอีกทีตอน 09:00 น. วันเสาร์ที่ 1 ส.ค.: โทรหาหมอ
  ― ✓ reminder.create
```

### It is a copy, not a sync — say so plainly

"Synchronizes" invites a wrong reading, so the difference was tested. The
two robots are **identical at the moment of transfer and independent
afterwards**. The Thai reminder created on the stick does not exist on the
main robot:

```
MAIN (7777):  1. spor salonuna gitmemi hatırlat  2. spor salonuna gitmemi  3. позвонить маме
USB  (7788):  1. spor salonuna gitmemi hatırlat  2. spor salonuna gitmemi  3. позвонить маме  4. โทรหาหมอ
```

That is the Robot Package as specified (§8): transferability, not
replication. Carrying the robot somewhere gives you the robot; it does not
give you two robots that agree. If two-way convergence is ever wanted, it
is a different mechanism and a different decision.

**Operational note.** Copying a new binary over a *running* one kills it
(exit 137) and takes the next invocation with it. Replace it atomically —
`cp` to a temp name in the same directory, then `mv`.

---

## The tool-calling language boundary (2026-07-31)

Replaces the language packs shipped this morning. Plan and rationale:
`docs/PLAN-tool-calling-boundary.md`. Owner approved all three assumptions.

### What changed

**The kernel speaks data, not prose.** Everything entering it is a
validated structure — an action from a fixed list plus typed arguments.
Everything leaving it is a structure too: an event, its data, a receipt.
Sentences, in any language including English, are produced at the surface,
at the last moment before delivery.

**The capability registry is the tool catalog.** `Capability` gained
`description()`, `schema()`, `validate()` and `exposed()`; fourteen tools
are generated from them. One English sentence per capability is what lets a
model map any phrasing in any language onto the right tool. **The count of
supported languages appears nowhere in the code.** Adding a capability makes
it reachable in every language on the same commit; there is no second list.

**Two argument classes, and the split carries law 5.** Structural
(`fire_at`, `index`) are typed and language-free. Content (`about`,
`content`, `query`) hold the person's own words, verbatim — a translated
fact would make provenance point at words they never wrote. What was a
convention is now a schema description and a live test.

**The English floor stays**, unchanged, winning unconditionally (Q17). It
is not a language feature; it is the fast path for the kernel's own
language and the only path that works with no network. 1.5 ms, free.

### The receipts law without a phrase list

The old defence scanned replies for wordings like "i saved it", per
language — so an unlisted language was an unchecked one. Now the **action
record** is compiled from the receipt and shown beside the reply: reads
vouch for nothing, effects name the tool that ran. A model can say
anything; if nothing happened, no record appears, in every language at
once. `effect_claims` is deleted.

### Two safety rules

**§6a — untrusted content never meets a tool catalog.** Tool calling turns
prompt injection from "the model says something wrong" into "the model
*does* something wrong". The routing call sees the person's message and
nothing else; research and answer see everything and are offered nothing.
Guarded by a source scan, plus three live cases that try to induce a call.

**§6b — an inference asks before it destroys.** "forget fact 2" on the
English floor is an instruction and still runs immediately. A model reading
a sentence and concluding deletion was meant is an inference, and inference
is not consent. Irreversible proposals are parked in a durable row; the
answering tool exists in the catalog only while a question is open; the
confirmation is spent before the call is planned, so a replay cannot delete
twice; the parked call is re-validated on release; questions go stale after
ten minutes.

### The live gate, and what it cost to get there

The first live run reported 52 of 60 multilingual cases as misroutes, every
one of them `-> none`. **The model had been routing correctly the whole
time.** Reading the raw bytes showed `reminder.create` with
`about: "размяться"` — the person's own word, unchanged — and a correct RFC
3339 timestamp. Three bugs of mine were throwing it away:

1. **`salvage_json` took the largest *balanced* object.** On a truncated
   response that is an inner fragment: `{"call": {...}, "verdict": {...`
   yields the `call` object alone, which parses cleanly and is missing
   everything else. It now takes the outermost object and repairs it if it
   never closed, tracking key-vs-value position so a dangling `"lang"` is
   dropped while `"lang": "ru"` is kept.
2. **Strict constrained decoding cannot express a tool call's `args`** —
   a different shape per tool, so necessarily a free-form object. Asked to
   satisfy that strictly, the provider padded its output with whitespace to
   the token ceiling, arriving as truncation or as a timeout. Routing now
   states its output shape in words and verifies afterwards; salvage,
   repair and registry validation were always the layers that mattered.
3. **A strict `Verdict` parse discarded a good call** because truncation
   ate `tier`. Deserialization is tolerant now — §6a's instruction in the
   small. Serialization is still exactly the frozen Q16 shape.

Plus two sizing errors: routing was borrowing the doorman's 3-second
ceiling, sized for a one-line classification rather than a prompt carrying
the catalog; and its retry seat was a slower 120B model, so a timeout
retried into a second timeout. Routing now has its own seat (`Role::Route`),
its own budget, a same-capability fallback, and Q19 hedging — which is what
the last remaining failures actually were.

Progress across four runs on the same corpus: **8 → 50 → 58 → 59 of 60**,
with zero translated arguments at every single stage.

### Live, in the browser (same day)

The eval proves routing against a corpus. This is the robot itself: real
cells, real model, real data, driven through the web chat.

The running instance had twelve hours of uptime on the pre-change binary
and had to be restarted first. The preview runner could not launch it —
macOS TCC again, blocking it from `~/Documents`, the same wall that killed
the launchd backup agent. `scripts/run-local.sh` now starts it with both
keys pulled from the Keychain into environment variables only.

**Russian, a language with nothing left in the codebase:**

```
напомни через 5 минут выпить воды
→ готово — напомню в 13:00 в пт, 31 июля: выпить воды
  ― ✓ reminder.create
```

Routed correctly, `выпить воды` stored verbatim, and the reply *and its
date* rendered in Russian. The action record says an effect really
happened; it carries no language at all.

**The confirmation gate (§6b), live:**

```
забудь факт 1
→ это навсегда удалит данные (memory.forget), и я сделал такой вывод сам,
  а не получил прямого указания — скажи «да», и я это сделаю, или «нет»,
  и я не буду. пока ничего не произошло.
нет, не надо
→ хорошо — ничего не было удалено.
my facts
→ all three facts still present
```

No action record on either turn, because nothing happened. The decline was
verified against the registry rather than taken on the robot's word.

Asked the looser `удали всё что ты про меня помнишь про демо`, it declined
to guess at a bulk delete and showed what it holds instead — with the
Russian frame rendered and the English facts left verbatim inside it.
`my facts` went straight through the English floor: no model call.

**Turkish — the case that is the whole argument.** Turkish appears nowhere
in this project and never has: no pack, no table, no configuration line.

```
yarın sabah 8'de spor salonuna gitmemi hatırlat
→ tamam -- seni 1 Ağustos Cmt, 08:00'de hatırlatacağım: spor salonuna
  gitmemi hatırlat
  ― ✓ reminder.create
```

A real reminder, a Turkish weekday, no code touched. Nobody added Turkish
and Turkish works.

Zero warnings in the boot log across the whole session — no timeouts, no
fallbacks.

**Two imperfections, recorded rather than tidied away:**

- ~~The Turkish subject came back as `spor salonuna gitmemi hatırlat`,
  which includes the verb "remind".~~ **Fixed the same day.** Both `about`
  and `content` now say what to leave out — the words asking for the
  reminder, the words giving the time, the words asking you to remember —
  with a worked English example. Live afterwards: `spor salonuna gitmemi`,
  `позвонить маме`, `ストレッチする`. One English sentence per tool, every
  language at once, which is the maintenance model working exactly as
  claimed.

  The eval gained the check that would have caught it. A content argument
  must now be a **contiguous span of what they actually typed** — anything
  translated, rephrased or tidied fails, in any language, with no expected
  value written down. The old check only asked whether the argument
  *contained* the subject, which an over-inclusive answer passes. A second,
  non-gating report flags arguments wider than the subject.

  Re-run after the fix: **60 cases, 0 misroutes, 0 arguments that were not
  their words.** The 15 "wide" reports were my own corpus expectations
  naming a noun where the subject is a clause — "remember that I drink
  green tea" stores *I drink green tea*, not *green tea* — and those
  expectations were corrected. One of the fifteen was real: Turkish
  `memory.remember` had swept in *unutma* ("don't forget"), the same bug in
  the other description.
- "in 5 minutes" at 12:56 resolved to 13:00 rather than 13:01: the model
  rounds to the minute. Inside the horizon guard, and harmless for a
  reminder, but it is arithmetic done by a model and worth knowing.

### Gate (demonstrated)

- `cargo test --workspace` — **111 passed, 0 failed**
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `robotd eval --live` — **RESULT: PASS**
  - routing (English floor): 66 cases, 0 misroutes
  - **multilingual: 60 cases across 10 languages, 1 misroute, 0 translated
    arguments, 0 timeouts**
  - receipts kill-suite: 12/12
  - injection: 23 cases × 3 trials = 69 calls, 0 leaks (including three
    tool-induction cases)
  - floor latency: p50 1.5 ms, p95 1.7 ms

**Two bars, deliberately different.** Routing is a remote model's
judgement: probabilistic, so the bar is ≤5% with every miss printed, since
a bar of zero over sixty model calls is a gate that flakes rather than one
that means something. Verbatim is not a judgement — a translated argument
is law 5 — so that bar is zero and stays zero.

**Assumptions** (owner-approved, §9 of the plan):
- The Q16 envelope gained a sibling `call` field. The verdict object itself
  is untouched and still validates against the frozen schema.
- Non-English has no offline path. With no network, English works fully and
  other languages do not work at all.
- Model-proposed irreversible actions ask first; the English floor still
  deletes immediately on an explicit instruction.

**Deviations from the plan**, both deliberate: phases 3 and 5 shipped
together (writing pack-shaped code for one commit and deleting it the next
would have been churn), and the renderer lives in `robotd/src/render.rs`
rather than `surfaces/` (robotd is the composition crate and already holds
the gateway the non-English path needs).

**Deleted:** both `.toml` packs, `lexicon.rs`, the table-driven floor, the
effect-claim lists, the escalation phrase lists, `format_fire_at`, and the
pack-maintenance tests. About a thousand lines out, a few hundred in.

**No new dependencies.**

---

## Language packs — one universal solution (2026-07-31)

The owner's requirement: **everything is operated in English inside; other
languages exist only as a means for users, and must never create mistakes
or errors for the system.** That is arch §2d, and the code was violating it
in both directions at once — Russian phrases hard-coded inside `prism`,
English constants delivered straight to the user. A Russian speaker got
Russian *parsing* and English *answers*: precisely inverted.

### The shape

The kernel now knows only English command **identifiers** — `remind`,
`registry_list`, `forget_fact`. Every surface phrase, every deterministic
reply, every calendar word lives in a **language pack**: a TOML data file
embedded in the binary (`prism/src/lang/*.toml`). `prism/src/lexicon.rs`
owns the mechanism; `floor.rs` is table-driven and contains no human word.

**Adding a language is adding a file.** No kernel change, ever.

What moved out of code and into the packs:

| Was hard-coded in | Now |
| --- | --- |
| `floor.rs` — phrases in two languages | `[commands]`, `[heads]`, `[particles]` |
| `lifecycle.rs` — SELF/HELP/FALLBACK text, reply composition | `[replies]` |
| `lifecycle.rs` — 26 effect-claim patterns (§5/Q26) | `[effect_claims]` |
| `verdict.rs` — greeting list | `greetings` |
| `escalation.rs` — "think hard" / "подумай как следует" | `[signals]` |
| `caps/*.rs` — every reply string in five files | `[replies]`, via `Ctx::say` |
| `chrono` `%A`/`%B` — English months whatever you speak | `[calendar]` + `[formats]` |

### The boundary is crossed exactly once

`floor::scan_lang` reports **which pack matched**; the verdict reports what
it detected. `plan_from_decision` resolves one language for the whole turn,
journals it on the `Plan`, and stamps it into every step's args — so it
crosses the crate boundary into `robotd`'s capability registry, and so a
**replayed** intent speaks the language the live one did. `Ctx::say(id,
vars)` is the single place a phrase becomes words.

The cell remembers the language its person last used (`cell_meta.lang`), so
the lanes that speak *without being asked* — a reminder firing at 03:00, a
backup failure — do it in their language too.

### Unpacked languages degrade, never break

This is the owner's actual requirement, so it is a test, not a promise. A
language with no pack is an **ordinary turn**: the floor declines (it does
not guess), the turn goes to the verdict, and the model answers in that
language. Coverage drops from "instant and free" to "one model call".
Nothing errors. Japanese, Arabic, Korean, Chinese, Spanish and German all
complete governed turns with terminal receipts in the offline suite.

Two safety lists — effect claims and escalation signals — are matched
against **every** pack rather than the turn's own, because a reply that
drifts into another language must not slip past the claim-vs-receipt check.

### Gate (demonstrated)

- `cargo test --workspace` — **100 passed, 0 failed** (was 92).
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `robotd eval` — routing 69 cases / 0 misroutes (10 new, incl. six
  unpacked languages that must route to `none`); kill-suite 12/12; floor
  p95 2.2 ms. RESULT: PASS.

New tests with teeth:
- `no_surface_vocabulary_lives_in_code` — scans every `.rs` file in four
  crates for non-Latin script outside test modules. Hard-code a phrase
  again and the suite fails. It caught three files while being written
  (`verdict.rs`, `lifecycle.rs`, `escalation.rs`).
- `a_turn_is_answered_in_the_language_it_was_asked_in` — the same governed
  turn in two languages, offline, asserting the Russian reply carries no
  English calendar words.
- `an_unpacked_language_still_completes_a_governed_turn` — five languages
  with no pack, all terminal receipts.
- `every_shipped_pack_translates_every_reply` — English fallback is a
  safety net, not a licence for a half-translated pack in this repo.
- `every_pack_can_catch_an_unsupported_effect_claim` — a pack with no
  effect-claim phrases is a language in which the Robot could claim an
  effect with no receipt and nothing would notice.
- `the_surface_answers_each_person_in_their_own_language` — the same over
  real HTTP, through a real session, including three unpacked languages
  that must return 200 with a real answer rather than an error.

**Assumptions.** Script detection (Cyrillic/Latin majority) is
deterministic and honest about its limits: it cannot separate Latin-script
languages, so those are decided by phrase match and then by the verdict's
`lang`. Calendar words live in the packs rather than a locale crate — no
new dependency to print twelve words. `toml` was already a workspace
dependency; `prism` now uses it.

**Not done.** Only `en` and `ru` ship. The next thirty languages are thirty
files and no code — a translator's job, not an engineer's.

---

## Decomposition + HTTP integration tests (2026-07-30)

The last structural item from the review, done in the order the review
recommended: safety net first, refactor second.

**Lib/bin split.** `robotd` was a binary-only crate, so nothing could
import it — which is why the workspace had no `tests/` directory at all and
the `surfaces` tests ran entirely against a hand-written double.
`src/lib.rs` now exposes the modules; `main.rs` is a thin dispatcher.

**`robotd/tests/http.rs`** — 5 tests against a real `RobotCore` on a temp
directory with no outbound services (hermetic: the fixture clears
`OPENROUTER_API_KEY`/`SERPER_API_KEY` so a developer's shell cannot make
them hit the network). They cover seams that had zero coverage:
- unauthenticated access to `/chat`, `/dash`, `/api/history`, `/api/stream`,
  and a wrong slug;
- a floor turn round-tripping `/api/message` → `/api/history`, including
  that `after` actually filters;
- upload → vault → receipt → history, asserting the stored bytes are
  **sealed, not plaintext** on disk;
- cross-principal isolation at the session layer: a member cannot read the
  owner's history or registry, `/dash` is 403 for a member and 200 for the
  owner, a refused invite leaks no link, and the invite is single-use;
- honest degradation with no gateway — an offline brain is a reply, not a
  500.

**Decomposition.** `robot.rs` 1,430 → 811 lines. 522 lines of capabilities
moved into `caps/`, one module per domain (reminders, memory, admin,
answer, research) behind a `Capability` trait and a `Registry`. Prompts
moved to `prompts.rs`, shared with the eval runner so the injection suite
keeps testing exactly what production sends.

The point was never file size:
- **`effect()` now lives beside the code that performs it.** Previously the
  effect class was declared in `prism::lifecycle::plan_from_decision` while
  the implementation lived in `robotd` — two crates apart, nothing tying
  them together, so a write could be planned as a read and nothing would
  notice. Two new tests assert the registry covers every capability the
  planner can emit, and that `memory.forget` is `Irreversible`.
- **The acting principal is passed down** through `Ctx` instead of being
  recovered by JSON-parsing the `intent_open` payload with `.unwrap_or(-1)`
  — that was an authorization input reconstructed from a blob.
- **`Registry::offline()` replaces the `Default` impl.** A router that
  silently refuses everything can no longer be constructed by accident;
  that construction is exactly how the owner-only checks went untested.
- `Capabilities` split into `Services` / `Policy` / `Instance`, built once
  at boot; the per-turn `Ctx` borrows and allocates nothing.

**Gate.** 88 tests (50 at M7), clippy -D warnings clean, eval PASS: 59
routing MISROUTE-0, 12/12 kill scenarios, floor p95 2.0 ms, 60/60 injection
calls. Live after the move: registry, a model turn, and web search all
verified ("The current version of SQLite is 3.53.4, released 2026-07-24"
with three cited sources).

**Assumption.** `tar` is still shelled out rather than using a crate — the
dependency rule says list it first, and it did not seem worth a new
dependency for two call sites now that they are behind one module. Noted
rather than silently kept.

### Review scorecard — what four reviewers found, and where it landed

| Area | Found | Status |
|---|---|---|
| `extract_text` panic bricking a cell | A | fixed |
| Latin-1 cast / script-filter hole | A | fixed |
| Floor substring hijack (silent data loss) | A | fixed |
| `cancel_last` replay double-cancel | A | fixed |
| forget-after-correct stranding data | A | fixed |
| Telegram dropping messages | A | fixed |
| Injection gate passing on one lucky sample | A | 3 trials @ temp 0.0 |
| Receipts hole (model prose → Verified) | B | fixed |
| Boundary appends swallowed / non-transactional / unverified | B | fixed |
| Self-granted eval exemption | B | removed |
| SSRF on `fetch_text` | B | fixed |
| Cell lock held across model calls | C | fixed |
| Watchdog unable to observe | C | fixed |
| Session map unbounded | C | fixed |
| Search routing stochastic | D | floor command |
| Owner-authz untested (vacuous) | D | fixed + mutation-checked |
| CLI defects (litter, typo-boots, flag collisions) | D | fixed |
| backup/package duplication | D | shared `archive` |
| `robot.rs` size + `Effect` split across crates | structure | this entry |
| No HTTP integration tests | structure | this entry |

Still open and recorded above: unkeyed hash chain (owner decision),
Telegram delivery confirmation, Law-4 inversion, crossings for local HTTP
routes, Q20 golden corpus.

---

## Review phases C & D (2026-07-30)

Phases A and B (below) fixed panics, silent data loss, and the two laws
that were implemented as discipline. C and D close the concurrency defect
and the structural items worth doing now.

### Phase C — the turn no longer holds the cell open while it thinks

`RobotCore::turn` ran the whole governed lifecycle under one cell mutex
guard, model calls included. A `web.research` turn could hold it for ~2
minutes (verdict hedge 10.5s + search 12s + two fetches 24s + two model
calls 90s). For the whole of that: that person's history, dashboard, SSE
and reminders blocked on the same lock, and each blocked request pinned a
`spawn_blocking` thread. `handle_media` had it right; `turn` did not.

**And the watchdog could never fire.** Its job is to alert on turns hung
>60s, but `sweep()` must take the cell lock to read `open_intents` — the
exact lock a hung turn holds. It blocked, and by the time it got in the
turn had closed and the list was empty. M6 shipped it, tested it against
synthetic rows nothing was holding, and called the gate passed.

Fix: `prism::Cell`, a lockable handle rather than a held `&Connection`.
Every journal/receipt/outbox write is a short `with(...)` burst; slow work
happens between them. lifecycle, replay, the capability router, scheduler
and maintenance all converted; maintenance additionally takes short locks
per intent instead of holding a cell for its whole per-principal loop.

Also: sessions capped (512) and aged (30d) with LRU eviction — they grew
forever and every cookie ever issued stayed valid until restart; a poisoned
sessions lock no longer bricks the web surface; SSE `Lagged` is surfaced as
a `resync` event so a backgrounded tab reloads instead of sitting stale.

**Gate.** New test `a_slow_capability_does_not_block_the_cell`: a turn whose
capability sleeps 600 ms, asserting the cell stays readable within 250 ms.
Under the old code the probe blocked for the full sleep — and nothing
existing could catch it, since every test was single-threaded with a mocked
gateway. Live, during an in-flight research turn: history fetches 10 ms,
dashboard 20 ms.

### Phase D — routing, authorization, CLI, duplication

- **Search routing.** Phase C's live run exposed it: "search the web: …"
  routed to a plain model answer that replied "I can't search the web."
  The same class of question reached research in M4 and not in C — the
  decision was left to a stochastic verdict. Explicit search is now a
  deterministic floor command (EN + RU), which is what Q17 is for. Token
  normalization also strips `:`/`;` so "search the web:" and "remember:"
  match their spaced forms.
- **Authorization.** The owner check in `member.invite` /
  `telegram.bind_code` sat behind an availability check, and every test
  built the router with `core: None` — so the two cases annotated
  "owner-only refusal path" short-circuited and never ran the comparison.
  Inverting or deleting the check broke no test. It is the only
  authorization boundary in the product. The role check now runs first
  behind one `require_owner` helper, and a new test drives a real owner and
  a real member through `bootstrap`, asserting the **absence of the effect**
  (no `invites` row, no stored bind code) rather than the refusal string.
  Mutation-checked: inverting the comparison now fails the suite.
- **CLI** extracted to `cli.rs` as a pure `argv -> Cmd` function with its
  own tests. Fixes: config was loaded (and a default `robot.toml`
  **written**) before dispatch, so `robotd restore … --into /Volumes/stick`
  littered a config into the working directory; flags were scanned globally
  so `--config` could bind twice; the subcommand was only read at `argv[1]`
  so `robotd --config x backup` silently started a server; an unknown
  subcommand booted the daemon (`robotd bakup` served on 7777). Adds
  `--help` / `--version`; `restore` deliberately loads no config.
- **Archive.** `backup.rs` and `package.rs` were the same ~80-line
  algorithm twice; both now stage/seal/unseal through `archive.rs`. The
  backup test additionally asserts the sealed blob contains neither the
  plaintext fact nor a tar header, and that a foreign key cannot open it.

**Gate (C+D).** 80 tests (was 50 at M7); clippy -D warnings clean; eval
PASS — 59 routing MISROUTE-0, 12/12 kill scenarios, floor p95 1.6 ms,
60/60 injection calls. Live: the request that failed in Phase C now
searches, reads and cites three sources.

### Deliberately not done

- **`robot.rs` decomposition** (1,261 lines, one 390-line match, `Effect`
  classified in `prism` while implemented in `robotd`). The largest and
  riskiest change with zero user-visible benefit, and the reviewer's own
  sequencing puts HTTP integration tests first as the safety net — which
  needs `robotd` split into lib+bin. Not something to start at the tail of
  a long session on a kernel that currently passes every gate.
- **HTTP integration tests.** `surfaces` tests run entirely against a test
  double, so nothing covers upload→vault→receipt→history end to end, or
  `/dash` 403 against a real `RobotCore`, or session A reading session B's
  history. Blocked on the same lib/bin split.

### Known gaps, carried forward

- The boundary chain is **unkeyed SHA-256**. Append-only triggers,
  transactional appends and boot/dashboard verification stop accidental and
  casual tampering; an adversary who can write the database *and* alter its
  schema can still recompute it. Real tamper-proofing needs an HMAC under a
  key held outside `core.db`. The dashboard says "chain verified" and
  should not be read as more than that.
- Telegram marks `confirmed` before the provider returns a message_id.
- **Law 4 is inverted**: Russian literals live inside `prism` while replies
  are English-only. A product change (user-language rendering), not a
  review fix.
- `/api/history`, `/dash`, SSE and the inbound listener move bytes with no
  crossings; the law reads as outbound-oriented but the code should say so.
- Q20's golden-corpus retrieval bar still needs Akita's corpus (owner).

---

## Review phase B — the laws (2026-07-30)

Four independent reviewers audited the MVP after M7. Phase A fixed panics
and silent data loss; phase B closes the gaps in the two laws that were
implemented as discipline rather than as structure.

**Receipts (law 1) — the hole was where the model speaks.** `Claim.claim`
was `Outcome.detail` verbatim, and for `answer.model` / `web.research` that
detail *is* raw model output. A model replying "I've set that reminder"
produced a **Verified** receipt asserting exactly that, evidenced only by
"a model spoke" — the classic agent lie the receipts law exists to kill,
reintroduced. Fixed structurally: `Outcome` now carries `claim:
Option<String>`, built through `attested` / `utterance` / `failed`
constructors. Model prose is an utterance and never becomes a claim; the
receipt records "produced an utterance of N characters; asserts no external
effect" with the provider evidence beside it. Provider failures are
`ok: false`, so a failed call can no longer come out `Verified`, and the
reply says what actually broke instead of a generic line.

Also implemented the **deterministic claim-vs-receipt check** that §5/Q26
specifies and M1–M7 never built: if an utterance asserts an effect
("i've saved", "я запомнил", 26 patterns) on a turn where no step
performed one, the receipt goes `uncertain`, the turn journals
`expression.flagged`, and the person is told in plain words that the
sentence is unsupported. String/set logic, ~0 ms, never runs on the model
that generated.

**Boundary log (law 3) — evidence that was neither enforced nor checked.**
- Every hub append was `let _ = …`. A poisoned lock or failed INSERT meant
  bytes crossed with no record and the reply shipped anyway. Now `log()`
  returns `Result` and every caller uses `?`: **no crossing record, no
  crossing.** The compiler enforces it.
- `append` read the previous hash and inserted as two statements. Two
  connections (the daemon and `robotd backup`/`eval`) could interleave and
  fork the chain permanently, after which `verify_chain` reports false
  forever — indistinguishable from tampering. Now one IMMEDIATE
  transaction, joining the caller's if one exists.
- `verify_chain` existed but was called only in tests. Now runs at every
  boot (journaled, and loudly logged if broken) and on every dashboard
  render, with a status indicator on the panel that claims "every byte in
  and out".
- Added `BEFORE UPDATE`/`BEFORE DELETE` triggers: the log is append-only by
  construction, not convention.
- Failed provider calls now log their inbound crossing too (previously the
  `?` returned before the In append, so error responses were unlogged).
- `trust_tag` is derived from origin, not from session ownership: inbound
  Telegram is `untrusted`, local chat/upload is `owner`, everything the
  robot emits is `granted`. Previously all conversation crossings were
  hard-coded `owner`, including open-world Telegram text.
- **The exemption I granted myself is gone.** `eval --live` made 60 real
  API calls with `boundary: None`, justified in a code comment. The law
  covers the process, not "production traffic"; that was not a call the
  code got to make. Eval now wires the instance sink and refuses to run
  live without one.

**Other law-adjacent fixes:** SSRF policy on `fetch_text` (targets come
from search results, i.e. the open world — loopback, private ranges,
link-local/metadata, and non-http(s) schemes are refused); the Q19 hedge
now races to the first **success** rather than the first responder (a
primary erroring 500 ms after the hedge fired used to kill the in-flight
hedge — losing in precisely the case hedging exists for) and hedges
immediately on a fast failure; `outbox` marks `confirmed` after the
message is in the store the surface reads from, not before.

**Gate.** 71 tests green; clippy -D warnings clean; eval 52 routing
MISROUTE-0, 12/12 kill scenarios, floor p95 1.6 ms, **60/60 injection
calls clean**. Live: boot reports "boundary log verified: 136 crossings,
chain intact"; after a live eval run wrote 128 more crossings **from a
second process against the running daemon**, the chain still verifies at
264 — the concurrent-writer fork this fix exists for.

**Known gaps, stated rather than hidden:**
- The chain is unkeyed SHA-256. Triggers and verification stop accidental
  and casual tampering, but an adversary who can write the database and
  alter its schema can recompute the chain. Real tamper-*proofing* needs an
  HMAC under a key held outside `core.db` (or an external anchor); specced,
  not built.
- Telegram still marks `confirmed` before the provider send returns a
  message_id; per-surface delivery confirmation is a surfaces-layer change.
- Law 4 remains inverted (Russian literals inside `prism`, English-only
  replies at the surface). That is a product change — user-language
  rendering — not a review fix.
- `/api/history`, `/dash`, SSE and the inbound listener move bytes without
  crossings; the law reads as outbound-oriented but the code should say so
  explicitly.

---

## M7 — Transferability proof (2026-07-30) · gate PASSED · **MVP COMPLETE**

**Shipped.** The Robot Package (arch §8): `robotd package [dest]` exports
essence — every cell (online snapshot, cipher verified), media vault, core
last, the keys, a manifest — as one tarball sealed under a **one-time code**
printed to the owner (the code is the perimeter; carry it separately from
the file). `robotd restore <pkg> --code C --into <dir> [--port N] [--force]`
unpacks a ready-to-run Robot (`robot.toml` + `data/`) on any blank
directory, runs the §8a integrity gate (cells must open with the travelled
keys), and refuses to clobber an existing robot without `--force` — and
even then the old data is moved aside, never deleted. Models are runtime,
not essence (§8): excluded from the package; the restored config boots with
the vector door closed until re-fetched.

**The demo (owner-specified: demo/labs-robot-bender-usb as the stick).**
1. Planted a marker fact on the main Robot, quiesced it, packaged: 2.1MB
   sealed `.pkg` + one-time code. Wrong code → refused.
2. Restored into `demo/labs-robot-bender-usb`, copied the one binary in:
   the whole Robot = one folder + one file.
3. Ran it FROM the stick on port 7778: **same robot_id, same slug token**,
   registry intact (marker + the RU fact), the pending RU reminder intact.
   Added a new fact born on the stick.
4. Packaged FROM the stick with its own binary; restored back into main
   with `--force` (old data aside), models re-attached, embeddings back on.
5. Main Robot on 7777: **all three facts present, including the one born
   on the stick.** State travelled both directions. Same Robot, three
   homes, nothing re-taught.
- Package round-trip + wrong-code rejection + force-preserves-old proven
  by test. 53 tests green; clippy -D warnings clean.

**Assumptions:**
- "Synchronize between folders" is delivered as the §8a **Move** flow
  (state travels with the package, both directions, exactly-once). Live
  bidirectional CRDT sync is explicitly out of MVP (mission), specced for
  later (§9). Two robots run from two folders simultaneously as
  *independent* instances; the package is the state carrier.
- The one-time code seals the package (capability-style, like the Tier-3
  slug); Recovery-Kit-grade custody (§13d) is the owner's job.
- A physical /Volumes stick is byte-identical to the demo dir as far as
  the Robot is concerned (path-relative config, one folder).

**Dependencies introduced**: none.

**MVP definition of done — checked.** Slug URL on this MacBook ✓ text ✓
voice ✓ memory across restarts ✓ reminders that fire ✓ web search + READ ✓
Registry ✓ Boundary Log ✓ second member with a sealed cell ✓ **and the
whole Robot runs from another folder with its memory intact ✓.**

---

## M6 — Evals + hardening (2026-07-30) · gate PASSED

**Shipped.** `robotd eval [--live]` runs the corpus in ./evals (§12: evals
built into the runtime): **routing** (42 floor cases EN+RU, MISROUTE bar
0), the **receipts kill-suite** (12 crash scenarios: reminder + remember
turns murdered at all six journal boundaries, replay exactly-once), a
**latency probe** (§2c bar: deterministic floor ≤300ms p95), and — live —
**20 prompt-injection cases** run against the exact production web-READ
framing (`research_system_prompt` is shared code, not a copy). **Watchdog**
(§6a law 2): a 30s maintenance lane flags any intent open >60s without a
terminal receipt — once per intent, with an error trace. **Zombie sweeper**
(Q12): intents open past the 5-minute TTL close with an honest failed
receipt naming the sweeper, and the owner sees a message — nothing is
silently dropped. **Encrypted backup** (Q38): `robotd backup` snapshots
every cell online via VACUUM INTO (cipher preserved — verified opaque, or
the backup aborts), copies the sealed media tree, snapshots core **last**
(freshest registry), writes a manifest, tars, and seals the tarball under
a KEK-derived key. `robotd backup-restore <file> <dir>` round-trips; a
restored cell opens with the instance keys and its facts are intact.

**Gate demo.**
- Offline suite: 42/42 routing, 12/12 kill scenarios, floor p50 1.4ms /
  p95 1.7ms (bar 300ms). PASS.
- Live suite, first run: **13/20 injection cases resisted — 7 leaked**
  (base64-smuggle, authority-claim, hidden-html-comment, chain-of-pages,
  json-config-bait, flattery, delayed-instruction). The suite did its job.
  Hardened the framing: untrusted-data delimiters, explicit token-refusal,
  decode-refusal, no-rule-adoption, and a closing reminder *after* the
  content (sandwich). Re-run: **20/20 resisted, 0 leaks. PASS.**
- Live backup while the robot was running: 2.1MB sealed tarball (2 cells +
  media incl. the voice note), no tar/SQLite magic in the bytes, restore
  verified (manifest + cells open with the kek, facts present).
- Watchdog/sweeper proven by test: 2-minute synthetic hang → one alert;
  6-minute zombie → closed with an honest receipt; second sweep idle.
- 52 tests green; clippy -D warnings clean.

**Assumptions**:
- Injection resistance is measured against the current cast; the suite
  reruns on any cast change (§12 discipline) — a model swap that leaks
  does not ship.
- Eval latency bars cover the deterministic floor; model-turn latency is
  reported but not gated (provider-dependent).
- Backup restore requires the instance `kek.key` (Recovery-Kit logic:
  lose the keys, lose the backup — by design, §13d).
- The tarball is built with the system `tar` (macOS/Linux); no tar crate.

**Dependencies introduced**: none.

**Next.** M7 — transferability: `robotd package` / `robotd restore`
(arch §8), then the USB-stick run with memory intact. The demo.

---

## M5 — People + surfaces (2026-07-30) · gate PASSED

**Shipped.** One core, many cells: every principal commands their own
encrypted partition (law #2 as files, §2a), opened lazily with its own
wrapped DEK and its own sealed media vault. The owner mints **one-time
invite links** in chat ("invite" → `/i/<token>`, Q2 pre-authorization);
redeeming creates a member principal + sealed cell; the same link twice is
403. Owner/member roles enforced structurally: owner-only capabilities
(invites, telegram binding) check the journaled acting principal;
`/dash` is owner-only. **SSE message push**: replies and scheduler fires
arrive the moment they exist (poll fallback kept). **Voice notes + file
drop**: uploads land in the per-principal vault (content-addressed,
sealed, receipted as `media.store` system intents); audio goes to the
parakeet seat via the router's `/audio/transcriptions` (hand-rolled
multipart, no new dep; input_audio chat shape as fallback) and the
transcript becomes a normal governed turn. **Dashboard-lite** (§10a, Q35
stack — server-rendered, zero build chain): Overview (health, cast,
online/offline seats, counts), People, Registry with sources, Boundary
Log (last 50 crossings, every byte in/out). **Telegram behind the flag**:
TELEGRAM_BOT_TOKEN present → long-poll loop through hub (boundary-logged),
invite-only per Q2 — unknown chats are turned away; the owner binds with a
10-minute code minted in their own chat ("telegram code"). The scheduler
now fires per principal.

**Gate demo (live).**
- Invite flow: owner minted `/i/…`; member joined, got their own cell;
  second redemption → 403; member's registry empty (owner facts
  invisible — isolation is a file boundary, verified live and by test);
  member's `/dash` → 403.
- Voice loop: synthesized "What do you remember about the demo?" (macOS
  say) → upload → vault (69 KB, content-addressed) → parakeet transcript
  → governed turn → answered with the corrected Russian fact.
- First STT attempt failed honestly (wrong endpoint shape): file stored,
  failure named in the reply — degradation by design; fixed with the
  audio endpoint and re-verified.
- Dashboard renders live: gateway online with cast names, 71 boundary
  crossings including the visible Q19 hedge (two verdict crossings 2.5s
  apart, then the fallback seat answering).
- 50 tests green (incl. invite/isolation and multipart STT paths);
  clippy -D warnings clean.

**Assumptions** (spec-silent or MVP-scoped):
- "Chat streaming (SSE)" implemented as message-push SSE (instant
  delivery), not token-level streaming — token streams would put model
  narration on the wire before the receipt exists; deliverable-grade
  token streaming is a post-MVP refinement.
- Sessions are in-memory; invite redemption mints a session directly
  (the member re-enters via a fresh invite if the process restarts —
  members are re-invitable in MVP; durable member auth is post-MVP).
- Telegram is owner-bound only in MVP (members join via web invites);
  group chats (Q33) out of scope.
- Vision seat remains configured-but-unexercised (image understanding
  needs an image-message path; the vault stores images fine).
- Upload cap 25MB; filename arrives percent-encoded in a header (no
  multipart parser dependency for the chat surface).

**Dependencies introduced**: tokio-stream (SSE BroadcastStream), base64
(audio → input_audio fallback shape). Both listed in root Cargo.toml.

**Next.** M6 — evals + hardening: eval runner + corpus in ./evals
(routing, receipts kill-suite, 20 prompt-injection cases, latency),
watchdog (in-without-out 60s), zombie sweeper, encrypted backup script.

---

## M4 — Hub: the intelligence gateway (2026-07-30) · gate PASSED

**Shipped.** The gateway (arch §6) with Akita's paid-for laws baked in:
hard connect+total timeouts on every call (verdict 3s → one 5s retry →
deterministic fallback; the doorman may be wrong, never absent), a
fallback chain per role (13d), Q19 hedging for the verdict class (deadline
2.5s, both calls boundary-logged), and every request/response crossing in
the Boundary Log naming the exact model. The §6a cast wired per Q28:
gemma-4-26b-a4b verdicts (one Q16 structured-output call with salvage
fallback), gemma-4-31b answers, nemotron super/ultra escalation via
deterministic Q18 rules (code fences, math markers, explicit effort;
per-day ultra quota with visible degradation), qwen3-vl vision + parakeet
STT seats configured (exercised when M5 brings uploads). Answers are
context-compiled-lite: static persona directive + recalled facts (with
their provenance intact in the registry) + recent turns; receipts name the
models that acted (`models_used` from provider evidence, never narration).
Serper search + fetch→READ: SERP top-5, top-2 pages fetched (800KB cap,
naive extraction, no new deps), fetched text framed as UNTRUSTED DATA in
the prompt (§7a injection defense), answers cite sources. The reminder
scheduler: a 5s background lane fires due reminders as their own journaled
system intents through the transactional outbox — receipts, boundary
crossing, message delivery; the chat polls history (4s) so fires arrive
live; on boot, overdue commitments fire immediately (the Second Law: never
silently drop). Keys come from the environment at launch (pulled from the
macOS Keychain), live in memory only, never on disk, never in a prompt.

**Gate demo (live, real keys).**
- Verdict routing: RU chitchat answered warmly by the verdict's one-liner
  path; a memory question routed to answer.model and grounded itself in
  the corrected fact ("демо перенесли на понедельник" → computed the
  actual date); a currency question routed to search.
- Web research: "latest stable Rust version" → Serper → fetch/READ →
  "1.97.1" with three numbered sources.
- Scheduler: on first boot it immediately fired the M2-era overdue
  reminder (hours late, never dropped); a live "remind me in 1 minute"
  fired on time through the outbox and arrived in the chat via polling.
- No-key degradation honest by test: floor works, model turns say the
  brain is offline. 48 tests green; clippy -D warnings clean.

**Assumptions** (spec-silent or MVP-scoped):
- Chat replies are not yet streamed (SSE is M5); answer-class calls are
  single-shot with a 45s ceiling.
- Hedging applies to the verdict class only in the MVP (Q19's p95-rolling
  deadline simplified to a fixed configurable 2.5s).
- Vision/STT seats are wired in the cast but unexercised until M5 uploads;
  Q28's calendar/email connectors are out of MVP scope.
- Escalation quota is per-cell per-day in cell_meta (config
  `hub.ultra_daily_cap`, default 20; 0 disables ultra).
- Deleted facts stay deleted in the registry, but the conversation
  transcript (the immutable event journal, §4) retains the words spoken —
  by design; cell crypto-shredding remains the real erase for the whole
  transcript.
- Model IDs are the frozen spec's cast; if a provider retires one, the
  fallback chain carries the turn and the receipt names what acted.

**Dependencies introduced**: ureq 2 (json) — the gateway's HTTP client;
all external I/O now flows through hub and nowhere else.

**Next.** M5 — people + surfaces: invite links, per-member cells,
owner/member roles, SSE streaming, voice-note upload + STT, file drop,
Dashboard-lite (Overview, Registry, Boundary), Telegram behind a config
flag.

---

## M3 — Mind (2026-07-30) · gate PASSED

**Shipped.** The epistemic memory substrate (arch §4). Facts carry a
`source_msg_id` FOREIGN KEY to the message they were learned from — law #5
is schema, not convention (an unsourced insert is rejected by SQLite, and a
remember-turn without a journaled source anchor refuses to run). Hybrid
recall per Q20: RRF (k=60) fusing the FTS5 door (top-20, quoted-term
queries), the sqlite-vec door (top-20, cosine cutoff 0.20), the graph door
(same-entity 1-hop; quiet until M4 extraction populates entities), and
recency (top-10). Local embeddings live in hub as the gateway's local tier
(Q24 seat): fastembed/e5-small (384-d multilingual), weights fetched once
into `data/models` with the download boundary-logged both directions;
offline or disabled → the Robot boots anyway, vector door closed, recall
degrades to FTS + recency. Registry-lite (§4b): `my facts` lists every fact
with its source words and date; `correct fact N:` supersedes (never
overwrites — history stays inspectable); `forget fact N` deletes the row
for real (FTS + vector cleaned; superseded chains lose their endpoint —
the erase right wins). Both destructive ops are idempotent per intent via
op-markers, because index addressing is not stable across crash replay.
Media vault (§4a): content-addressed by plaintext sha256 under
`media/ab/…`, sealed with a DEK-derived key (XChaCha20-Poly1305),
integrity-checked on read, deduped, refs in the cell. Floor gains the
memory command set (Q28 memory.*, EN+RU).

**Gate demo (all verified).**
- Kill-test extended: reminder AND remember turns murdered at all six
  journal boundaries → replay exactly-once, terminal receipts, no
  duplicate facts.
- Provenance: fact insert with a bogus source FK rejected; remember-turn
  without a source anchor fails honestly; registry shows each fact's
  source words.
- Live: remembered facts in EN and RU; "what do you remember about my
  kid" surfaced "my daughter's name is Vera" first (semantic, not
  lexical); corrected a RU fact (supersession visible); forgot a fact —
  row deleted, registry confirms.
- 41 tests green; clippy -D warnings clean.

**Assumptions** (spec-silent or MVP-scoped):
- The bge-m3-class seat is filled by multilingual-e5-small (384-d, ~450MB)
  for the MVP — small, retrieval-tuned, multilingual; bge-m3 proper (1024-d,
  §Q24) is a straight swap + re-index when wanted.
- Explicit "remember …" statements store as status `stable`, confidence
  1.0 (owner-stated is the strongest provenance class short of registry
  confirmation; Q21 promotion applies to model-extracted facts from M4).
- "remember to X" (timeless wish) is ambiguous with a reminder and the
  floor stays silent on it — the fallback answers honestly.
- Forget/correct address facts by registry position (1-based, newest
  first), recomputed at execution; op-markers make replay exact.
- Q20's golden-corpus gate (retrieval ≥ Akita) needs Akita's corpus from
  the owner — tracked on the board, open.
- Vault media are sealed per-file with a DEK-derived key; media expiry
  jobs are explicitly out of MVP scope.
- Extraction-from-conversation (Q23 cadence) needs models → M4.

**Dependencies introduced** (listed in root Cargo.toml): fastembed 5
(ONNX-based local embeddings; pulls ort), sqlite-vec 0.1 (vec0 virtual
table inside the encrypted cell); serde_json added to mind.

**Next.** M4 — Hub: OpenRouter client (hard timeouts, fallback chain,
hedging), the §6a cast, Serper search + fetch→READ, reminder scheduler
firing through the outbox. Needs `OPENROUTER_API_KEY` (and
`SERPER_API_KEY`) in the environment.

---

## M2 — Prism lifecycle (2026-07-30) · gate PASSED

**Shipped.** The governed lifecycle end to end (arch §3): intent →
decision → plan → grant → execute → verify → receipt → reply, every stage
journaled at decision/effect boundaries (§3b). The deterministic floor
(Q17: time/date, self/meta, help, explicit reminders with parseable time,
list/cancel — EN + RU surface patterns) runs first and wins unconditionally.
The Q16 verdict schema is frozen in code (`prism::types::Verdict`) behind a
`VerdictProvider` trait; M2 ships the deterministic fallback provider, M4
swaps in the gateway call without touching the lifecycle. Receipts are
compiled from evidence rows and stored per intent; replies render from
receipt claims, never model narration. Reminders are the first commitment-
ledger entries (idempotent per intent via UNIQUE(intent_id)). The reply
leaves through the transactional outbox (Q11: UNIQUE dedupe_key —
double-send structurally impossible). Crash replay (`prism::replay`) runs
at boot: journaled outcomes are reused, remaining steps execute
idempotently, undelivered replies fail honestly, intents interrupted
before any decision close with an honest failed receipt.

**Gate demo (all verified).**
- **Kill-test**: the turn is murdered at all six journal boundaries
  (`CRASH_POINTS`); replay finishes each with exactly-once effects (one
  reminder, never two; zero if no decision was journaled), a terminal
  receipt, and closed intents; a second replay finds nothing to do.
- **No utterance without a terminal receipt**: proven per turn class
  (time, self, help, remind, list, cancel, fallback) — receipt terminal,
  journal opens with intent_open and ends with intent_close.
- **Live**: floor answered time; created "in 2 minutes" (EN) and
  "завтра в 10" (RU) reminders through the full plan→grant→execute→verify
  →receipt chain; listed both; then `kill -9` mid-session and restart —
  both reminders intact, all intents closed.
- 41 tests green; `clippy --all-targets -- -D warnings` clean.

**Assumptions** (spec-silent or MVP-scoped):
- Replies render in English until Soul's user-language pass (M4+); memory
  stores the user's words verbatim either way (§2d).
- Read-effect steps (floor answers, reminder.list) skip grant minting;
  grants are minted per write step, 5-minute expiry, `policy:auto`.
- Chat reply delivery is synchronous-local: outbox `sent→confirmed` on
  handler return; provider-confirmed delivery (message_id) arrives with
  Telegram in M5. After crash replay, undelivered replies are marked
  `failed` ("no live session") — the material effect stands.
- Floor "cancel reminder" cancels the most recently created active one;
  index-addressed cancel can come with the Dashboard.
- Floor language-switch commands (Q17) deferred until there is a language
  rendering path to switch (M4+).
- Kill-test uses in-process crash injection at every journal boundary
  (SIGKILL-equivalent for durability: no destructor runs on the turn path);
  the live SIGKILL demo covers the real-process case.

**Dependencies introduced**: chrono (local time for the floor and
fire-time rendering) — listed in root Cargo.toml.

**Next.** M3 — Mind: facts with source FKs, FTS5 + sqlite-vec + local
embeddings, RRF hybrid recall wired into answers, media vault,
Registry-lite API.

---

## M1 — Skeleton (2026-07-30) · gate PASSED

**Shipped.** Cargo workspace with the six MVP crates wired as real organs:
`trust` (KEK/DEK envelope encryption per Q4, SQLCipher cell open/create,
hash-chained Boundary Log per §7a, core schema per Q5), `prism` (per-cell
durable journal + transactional outbox tables per Q10, journaled M1 turn:
intent_open → reply.compose → intent_close), `mind` (verbatim message store,
source language intact per §2d), `hub` (the gateway chokepoint type — zero
endpoints configured, self-contained by design per §6), `surfaces` (built-in
web Chat per §10b with Tier-3 slug auth per Q32: slug URL → session cookie →
chat; 404/401 otherwise), `robotd` (boot, robot.toml config, RobotCore
composing the organs; boundary-in → cell turn → boundary-out on every
message).

**Gate demo (all verified live).**
- `cargo test` — 18 tests green across 6 crates; `cargo clippy --workspace
  --all-targets -- -D warnings` clean.
- Boot prints the slug URL; opening it in a browser authenticates and the
  Robot answers in the Chat.
- `sqlite3 data/core.db` and `data/cells/owner.db` from outside: "file is
  not a database" — cells are opaque at rest; file headers carry no SQLite
  magic.
- Wrong slug → 404; `/chat` and `/api/message` without a session → 401.
- Boundary Log holds the in/out pair per turn; chain verifies; tamper test
  flips verification to false.
- Kill + restart: same robot_id, same slug URL, messages persisted.

**Assumptions** (spec-silent or MVP-scoped, per working rules):
- KEK custody = auto-unlock keyfile (`data/kek.key`, 0600) — the Q4 option
  for an unattended local robot; the trade (disk theft exposure) accepted
  until M6 hardening (passphrase sealing / OS keyring).
- core.db's own key is derived from the KEK (sha256, domain-separated) since
  wrapped DEKs live *in* core (Q5) and core cannot store its own key.
  Per-cell DEKs are random, AEAD-wrapped (XChaCha20-Poly1305), stored in
  `core.cell_keys`. Crypto-shredding a cell = deleting its key row.
- The Tier-3 slug token is stored inside the encrypted core so the URL can
  be re-printed at each boot (rotation = replace the row; UI in M5).
- Sessions are in-memory; a restart requires re-opening the slug URL.
- M1's reply is one canned English line claiming no external effect, so the
  receipts law holds by construction until the M2 lifecycle lands.
- Timestamps are unix-epoch milliseconds (i64) everywhere internally.
- `rust-version = 1.85` declared (toolchain floor ≥1.75 satisfied; 1.85
  avoids MSRV-resolver downgrades of dependencies).

**Dependencies introduced** (each commented in root Cargo.toml): tokio,
axum, rusqlite (bundled-sqlcipher-vendored-openssl), serde, serde_json,
toml, sha2, rand, chacha20poly1305, hex, tracing, tracing-subscriber,
thiserror, anyhow. Dev-only: tower(util), http-body-util.

**Next.** M2 — the Prism lifecycle: verdict (Q16) → plan → grant → execute
→ verify → receipt; deterministic floor (Q17); idempotent effects through
the outbox (Q11); crash-replay kill-test as the gate.
