# BUILD LOG — Bender MVP

One entry per milestone: what shipped, the gate demo, assumptions made,
dependencies introduced. Newest first.

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
