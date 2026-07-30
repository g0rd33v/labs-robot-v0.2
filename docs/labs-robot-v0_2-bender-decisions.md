# Robot Bender — Executable Decisions (Q1–Q43)

**Companion to `labs-robot-v0_2-bender-architecture.md` (rev-V) · 2026-07-29 · status: proposed, awaiting owner sign-off**
Each answer is the industry-standard or best-practice choice, stated as a decision. 🔴 = M0/M1 gate · 🟡 = M2–M4 gate · ⚪ = later.

---

## Identity, accounts, auth

**Q1 🔴 Labs Account vs self-hosted.** Superseded by arch §7b: **three access tiers.** Day one = Tier 3 (secret slug URL printed at first boot — capability-URL auth, zero account, local scope). Tier 2 = Labs Account magic link per device (no passwords anywhere; email is the perimeter). Tier 1 = magic link + 6-digit PIN verified in an HSM (Labs cannot open data even under compulsion). Passkeys are an optional Tier-2 enhancement, not the foundation. Nothing in v0.2 requires the Control Plane; Connect to Labs adds routing (robot.labs.co, @LabsRobot) and Sync as opt-ins.

**Q2 🔴 Telegram → principal.** **Invite-only by default.** The binding key is the Telegram numeric user-ID. Unknown user messages the bot → a `pending` principal is created, the owner gets an approve/deny card; an **invite link/QR** (deep-link `t.me/bot?start=<invite_token>`) pre-authorizes and assigns the role. Owner may switch the instance to open-guest mode explicitly.

**Q3 🟡 One person, two surfaces.** One principal, many **surface bindings** (a table: principal_id ↔ {telegram_id | webauthn_credential | api_key}). Linking uses the device-linking pattern (Signal/WhatsApp): from an authenticated surface, mint a short-lived 6-digit code; enter it on the other surface; bound.

**Q4 🔴 Key hierarchy.** **Envelope encryption.** One master KEK wraps per-cell DEKs (DEKs stored wrapped, in core). KEK at rest: OS keyring where present; on a headless server, a KEK file sealed by the owner passphrase, unlocked at boot via **TPM2/systemd-creds where hardware allows**, otherwise a passphrase prompt with an optional auto-unlock keyfile — the owner chooses, and the trade (auto-unlock = disk theft exposure) is stated at setup. The Recovery Kit (arch §13d) is a second wrap of the KEK. From Tier 2 up (arch §7b), a third wrap is **escrowed** with the Labs Key Service HSM — released by email proof (Tier 2) or email + in-HSM PIN (Tier 1), rate-limited in hardware.

## Cells & storage

**Q5 🔴 Schema split.** **Core file:** principals + surface bindings, roles/policies, grant registry, Hub config + wrapped secrets refs, Boundary Log, shared spaces (group conversations, shared memory), fleet stats, update state. **Member cell:** messages, facts/graph, embeddings, media index, commitments, soul (persona/lessons/revisions/journal), per-cell Prism journal, usage counters. **Group/shared conversations live in core's shared space**, each message attributed to its author-principal.

**Q6 🔴 Cross-cell queries.** Never fan-out at read time. **Event-driven rollups:** each cell maintains local counters; a write-through hook posts deltas to `core.stats` (spend, messages, tool calls per principal/day). Dashboard reads core only. Drill-down into one member's content (where policy permits) opens that one cell.

**Q7 🟡 Schema skew.** **Expand-contract migrations**, binary supports schema N and N-1, `PRAGMA user_version` checked at cell open, cells migrate lazily on first open, new columns behind feature gates until the fleet floor reaches N. A half-migrated instance is a normal state.

**Q8 🟡 Shared memory.** Explicit by construction: anything said in a **shared space** is shared (that's what the space means, and members see this at join); anything in a private conversation is private and moves to shared only by an explicit act (`share` action / owner policy), journaled as a memory mutation.

**Q9 ⚪ Media placement.** **Filesystem beside the cells**, content-addressed by hash (`media/ab/cdef…`), references + metadata in the cell. Large blobs in SQLite are an anti-pattern (backup churn, page bloat). The unit of portability becomes the **robot directory**: `core.db + cells/ + media/ + releases/` — "one folder" replaces "one file" at the instance level; a personal Robot remains one cell + its media.

## Prism & the journal

**Q10 🔴 Journal shape.** **Per-cell journal** for member turns; **core journal** for system/shared actions. Step log: `(step_id, intent_id, seq, kind, payload_json, started_at, completed_at, outcome_hash)`. Outbox: `(effect_id, intent_id, dedupe_key, target, payload_ref, state: pending|sent|confirmed|failed, attempts, last_error)`.

**Q11 🔴 Idempotency.** `dedupe_key = hash(intent_id ‖ step_id ‖ canonical_payload)`. Effects are written to the outbox **before** the network call (transactional outbox); replay consults outbox state; provider idempotency keys used where offered (Stripe-style); Telegram confirms via returned message_id stored on the effect. Double-send is structurally impossible, not statistically unlikely.

**Q12 🔴 Intent TTLs.** Per risk class: interactive intents reach a terminal state or an honest `failed` receipt in **≤5 min**; long tasks declare their own TTL in the plan; `awaiting_approval` parks up to **30 days**, then expires with a notice to both sides. A **zombie sweeper** (background lane, every minute) closes anything past TTL — the watchdog generalized.

**Q13 🟡 Interrupt vs grant expiry.** Approval **refreshes the grant**: resuming an interrupted step re-mints the grant at approval time; a stale parked grant can never fire. Parking max = 30 days (Q12).

**Q14 🟡 Fork safety.** Forks run in the same process, low-priority lane, with **Hub in replay mode**: recorded observations are served back (the VCR/record-replay pattern); live network is unreachable by construction in a fork. Forks write to a scratch overlay, never to cells.

**Q15 🟡 Retry policy.** Transient errors: **3 attempts, exponential backoff (1s/5s/25s), jittered**. Non-transient: step fails immediately. Optional steps fail-soft (plan continues); required steps → intent `partial` or `failed` with an honest receipt. **No automatic rollback** — compensations are explicit per capability (e.g., `calendar.delete` compensates `calendar.create`), proposed to the user, never auto-fired.

## Verdict & routing

**Q16 🔴 Verdict schema (frozen for M3).**
```json
{"action":"answer|task|search|meta|clarify|chitchat",
 "domain":"reminder|note|fact|calendar|email|file|none",
 "door":"exact|vector|web|blended|followup",
 "tier":"fast|super|ultra",
 "lang":"ru", "mood":{"valence":-1..1,"urgency":0..1},
 "confidence":0..1, "reply":"optional one-liner for chitchat"}
```
One call, structured output, salvage fallback (arch §6a).

**Q17 🔴 Floor contents + arbitration.** V1 floor: time/date/timezone, self/meta (SAP set), explicit reminder with parseable time, cancel/confirm of the pending item, help/commands, language switch. **The floor runs first and wins unconditionally** — a deterministic match never yields to a model verdict. (Cheapest call is no call; also the offline floor.)

**Q18 🟡 Escalation rules.** Declarative rules in `robot.toml`, evaluated deterministically over verdict output + content signals (code fences, math tokens, explicit "think hard", contradiction-with-memory flag). Ultra quota: **per-principal per-day**, enforced by the gateway; on exhaustion, degrade to super **with a visible note in the reply and the receipt**. Owner sees quota state in Dashboard.

**Q19 🟡 Hedging.** Deadline = rolling p95 TTFT per role (default 2.5 s verdict-class, 4 s answer-class). On hedge: fire schema-identical fallback, first responder wins, loser's request aborted; both calls appear in the Boundary Log and cost accounting; hedge-rate >5% raises an owner-visible alarm (provider health signal).

## Mind

**Q20 🔴 Retrieval v1.** RRF with k=60; doors: FTS top-20 (tokenizers per owner languages), vector top-20 (bge-m3, cutoff 0.20 carried from Akita), graph 1-hop top-10, recency as an RRF signal. **Gate:** Akita's golden corpus ported to Bender's schema pre-M2 (a named M2 entry task); ship bar = equal-or-better on the ported set.

**Q21 🟡 Promotion thresholds.** tentative → contextual: 2 independent sources *or* 1 explicit owner statement. contextual → stable: 3 occurrences across ≥7 days *or* owner confirmation in the Registry. Contradiction unresolved after 2 gentle prompts: keep both, mark `contested`, prefer the newest in answers with a hedge ("last I knew…"), surface in Registry.

**Q22 🟡 Temporal model.** **Write §4c** into the architecture: bi-temporal facts (event time + record time), supersession-never-overwrite, validity-filtered retrieval, volatility freshness horizons, commitment ledger as the future axis. (Text already drafted in our exchange; one paste away.)

**Q23 🟡 Extraction cadence.** **Async, post-turn, debounced:** extraction batches on a 2-minute conversation lull (background lane), plus a nightly consolidation pass (rollups, contradictions, salience). Nothing extraction-related sits on the reply path. The §2b 0.5-calls/turn line stands.

**Q24 ⚪ Embeddings.** **bge-m3** (1024-d, multilingual, retrieval-tuned, permissive license). Model change = background re-index with dual-index cutover (build new, verify on golden set, swap, drop old).

## Soul

**Q25 🟡 Soul storage.** In each member cell: `soul_persona` (dial + bounds), `soul_lessons` (source-linked), `soul_revisions` (diff + reason + rollback ref), `soul_journal` (first-person entries). Instance-level defaults in core. This *is* the SOUL-MASTER interface: that doc defines semantics; these four tables are its runtime home.

**Q26 🟡 Evaluator seat.** `gemma-4-26b-a4b` (≠ the 31b generator) for sampled expression-verify (10% of routine turns) and **always** on actioned/risky turns; deterministic claim-vs-receipt check runs on **every** turn (it's string/set logic, ~0 ms). Latency: verify runs post-stream for routine turns (audit), pre-send only for actioned turns.

**Q27 ⚪ Persona dial surface.** Both: `/soul` in chat (Akita's lesson: chat-first discoverability, buttons not free-text-only) and the Dashboard Soul panel.

## Hub, tools, skills

**Q28 🔴 V1 tool registry (frozen list).**
`memory.remember / recall / correct / forget` (read/write, no approval) · `reminder.create / list / cancel` (reversible_write, auto) · `web.search`, `web.fetch` (read, auto) · `calendar.list / find_slots / create / update / delete` (create+ = reversible_write, default auto with owner-settable approval) · `email.search / read / draft` (auto) + `email.send` (**approval: always by default**) · `file.save / read / list` (auto, vault-scoped) · surface send (implicit, journaled). Every name, effect class, and approval default lands in Appendix A's registry table.

**Q29 🔴 Calendar & email.** **Google first, native OAuth connectors in Hub** (MCP variants optional later — native gives us token scoping and receipts we fully control). OAuth: standard authorization-code with PKCE; for self-hosted, loopback redirect to the embedded server (the desktop-app pattern) or device-code flow where loopback is impossible. Tokens → vault; scopes minimal (`calendar.events`, `gmail.readonly` + `gmail.compose`; `gmail.send` only when the owner enables the send capability).

**Q30 🟡 Robot as MCP server.** Per-principal **scoped API keys minted in Hub** (shown once, vault-hashed), carried as bearer over an authenticated channel only (tailnet or Cloud proxy — never raw internet). Each key maps to a grant subset; exposed tools = that subset; every call journaled + Boundary-logged like any surface.

**Q31 🟡 Skill isolation v1.** Separate OS process under a dedicated unprivileged uid, launched via **systemd-run sandbox** (Linux: PrivateTmp, ProtectSystem=strict, no home, cgroup CPU/mem caps, network off unless the manifest grants domains); IPC = **JSON-RPC over stdio** with Prism as the broker — the skill never sees sockets, cells, or secrets; it sees arguments and returns results. (WASM replaces this at marketplace time, same contract.)

## Surfaces

**Q32 🔴 Chat bootstrap (the no-email problem).** Solved by Q1/arch §7b: **the Tier-3 slug URL** — no email required, ever, for first contact; the owner upgrades to magic link (Tier 2) or +PIN (Tier 1) when ready. Members joining via web (no Telegram): owner mints an invite link in Dashboard; the link carries the invite token; the member enrolls a passkey on arrival. Email magic links become available only if an email connector exists or via the optional Labs Cloud relay — a convenience, never the foundation.

**Q33 🟡 Telegram groups.** A group = a **shared space in core** (Q5/Q8). Known members map to their principals; unknown members follow instance policy (ignore / auto-guest / owner-prompt). Quota bills the **owner** by default (it's the owner's space), switchable to per-member attribution.

**Q34 ⚪ Telegram streaming.** Draft-edit cadence: first edit at first sentence, then every ~1.2 s or ~40 new tokens (whichever later) — inside Bot API rate limits; final message replaces the draft with full formatting. Web Chat streams token-level via SSE.

## Dashboard

**Q35 🟡 UI stack.** **No build-chain framework:** server-rendered HTML + htmx (or a single-file Preact bundle compiled once), assets embedded with `include_bytes!` — the Dashboard versions with the binary, zero node_modules in the release path. API auth: session cookie (HttpOnly, SameSite=Strict) from passkey login + CSRF tokens; the local API is same-origin only.

**Q36 ⚪ Join-policy presets.** Three, shown at join in one sentence each: **Private** (owner sees your usage numbers only, never content) · **Standard** (owner can open content in an audit, and you are notified when it happens) · **Open** (owner has full view). Owner picks the instance default; changing it requires member re-consent.

## Ops, deploy, Control Plane

**Q37 🔴 Control Plane in V1: none.** M0–M5 run with zero Control Plane: local accounts (Q1), updates via a **static signed release manifest** (a URL the binary polls; signature verified against a pinned key — The Update Framework pattern, minimal profile). The Control Plane is a parallel/post-V1 product that *adds* convenience (accounts, fleet, backup coordination), never a dependency.

**Q38 🟡 Backup.** Per-cell `sqlite3_backup` (online, consistent per file) to a staging dir + hardlinked content-addressed media + core last → one encrypted tarball with a manifest (cell versions + journal high-water marks). Cross-cell coherence beyond that is not required — cells are independent by design; core-last captures the freshest registry. Restore drill is an M5 gate addition.

**Q39 🟡 TO-VERIFY (§2b).** Action item, pre-M3, on the owner/Akita side: grep Akita logs for (a) router-model calls per turn, (b) super/ultra escalation shares. Until then the budget carries the ±40% band explicitly.

**Q40 ⚪ Repo & license.** Cargo workspace: `robotd` (bin) + crates `prism, soul, mind, hub, trust, surfaces, dashboard`. Home: labs-hub Gitea, mirrored to GitHub at open-sourcing. CI targets: linux x86_64/aarch64, macOS aarch64, Windows x86_64 (the SQLCipher CI rule from §13a). **License: AGPL-3.0** with a Labs commercial license for embedding (the standard open-core defense for exactly this category — openness guaranteed, cloud-clone resale deterred).

## Product edges

**Q41 🟡 Quota UX.** Soft warning at 80% ("running low today"); at 100%: deterministic floor + queueing with a friendly, honest message ("I've hit today's thinking budget — this will go out at midnight, or ask the owner to raise it"), owner notified. No payment rails in v0.2 — quotas are config; the paid-tier hook is a Chiron/Control-Plane matter.

**Q42 ⚪ UI language.** Dashboard and Chat **chrome is English-only in v0.2** (§2d spirit: one exact dialect for the control surfaces); all *content* — conversations, memory, media — is any language, always.

**Q43 ⚪ Legal texts.** Plain-language, versioned in-repo at `/docs/agreements/`: setup Recovery-Kit acknowledgment, member join-policy disclosure (Q36 presets), data-handling one-pager. Shown at setup/join, hash-referenced in the journal when accepted. Owner may append house rules, never replace the floor.

---

**Net effect:** all ten 🔴 gates now have decisions; M0 is startable. Three follow-ups remain open on the owner's side: sign off on this document, pull the two Akita numbers (Q39), and approve §4c temporal-model text for the architecture.
