# BUILD LOG — Bender MVP

One entry per milestone: what shipped, the gate demo, assumptions made,
dependencies introduced. Newest first.

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
