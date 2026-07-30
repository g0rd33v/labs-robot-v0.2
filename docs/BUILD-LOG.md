# BUILD LOG — Bender MVP

One entry per milestone: what shipped, the gate demo, assumptions made,
dependencies introduced. Newest first.

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
