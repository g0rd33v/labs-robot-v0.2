# BUILD LOG — Bender MVP

One entry per milestone: what shipped, the gate demo, assumptions made,
dependencies introduced. Newest first.

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
