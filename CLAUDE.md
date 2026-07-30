# Bender MVP — Labs Robot v0.2

A fully featured Robot running locally on this machine, later moved to a
USB stick unchanged. These rules are binding for every change.

## Frozen spec

- `docs/labs-robot-v0_2-bender-architecture.md` (rev-Y — source of truth)
- `docs/labs-robot-v0_2-bender-decisions.md` (Q1–Q43 — decided; do not reopen)

Where the docs conflict with convenience, the docs win. Do not redesign;
implement. If a task genuinely requires deviating, stop and raise it with
the owner — the spec changes first, then the code.

## The five laws (non-negotiable)

1. **Receipts law.** No reply may claim an effect without a verified journal
   receipt.
2. **Cell isolation.** Per-principal encrypted SQLite cells; zero cross-cell
   reads except by explicit policy.
3. **Boundary Log.** Every byte in/out of the process is logged, both
   directions, hash-chained. All external I/O goes through the Hub gateway —
   no other socket path.
4. **English-only internals** (arch §2d); the user's language at the surface.
5. **Provenance.** No fact without a source pointer (FK constraint, not
   convention).

## Engineering rules

- **No new dependencies without listing them.** Every dependency is declared
  once in the root `Cargo.toml` `[workspace.dependencies]` with a comment
  saying why it exists. New ones are listed in BUILD-LOG before use.
- Secrets (`OPENROUTER_API_KEY`, `SERPER_API_KEY`) come from env/secret
  store; never on disk unencrypted, never in model context.
- macOS (Apple Silicon) is the target; keep Linux compiling.
- Milestone ritual: tests green → gate demonstrated → handoff note in
  `docs/BUILD-LOG.md` → commit. Always run
  `cargo test && cargo clippy -- -D warnings`.
- Small spec silences: pick the simplest option, record it in BUILD-LOG
  under "Assumptions", continue. Ask the owner only for missing API keys,
  spec contradictions, or anything touching the five laws.

## Workspace layout (MVP)

Six crates, per the owner's MVP mission (Q40's `soul` + `dashboard` crates
are deferred post-MVP; soul is a static persona directive, dashboard-lite is
served from the binary):

- `robotd` — the one binary: boot, config, composition (RobotCore)
- `prism` — governed execution kernel: journal, outbox, intent lifecycle
- `mind` — memory: messages, facts+sources, retrieval, registry
- `hub` — the sole external gate: models, search, connectors
- `trust` — substrate: keys, encrypted cells, boundary log, core schema
- `surfaces` — built-in web Chat, later Telegram behind a config flag

MVP milestones M1–M7 and current status live in `docs/BUILD-LOG.md`.

## Runtime layout

`robot.toml` (config, no secrets) + `data/` (`kek.key`, `core.db`,
`cells/`, `media/`) — both gitignored; the data directory is the Robot.
