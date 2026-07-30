# Labs — The Robot Architecture

**Codename: BENDER · Robot v0.2 · rev-Y (§8a expanded: instant start → play → move → full detach) · 2026-07-29 · Internal source of truth**

> Development versions of the Robot carry alphabetical codenames drawn from famous companions of all kinds — dogs, robots, sidekicks, mentors. **Robot v0.1** (production today: Telegram, Hetzner, container stack, live for one month) keeps its plain name in production; its development line is **Akita** (the loyal dog). **Robot Bender (v0.2)** is the sovereign rebuild specified here: one signed binary, one encrypted file, four organs, evidence-grade receipts — delivering the vision as stated: human, personal, private; a Robot for your life, your work, your family, your kid, your business, running wherever the owner chooses, from a cloud to a USB stick. **Akita and Bender are parallel lines:** Akita keeps running and evolving in production; Bender is built beside it as the next-generation Robot and launches as its own instance — no migration between them. The next development line is **Chiron** (v0.3).

---

## Contents

**PART 0 — FOUNDATION**
- §0. The constitution
- §0a. The product thesis — small, open, fast, upgradeable

**PART I — THE SYSTEM**
- §1. Two planes
- §2. The Robot runtime — one binary, encrypted cells
- §2d. The language boundary (law)
- §3. Prism — the governed execution kernel
- §3a. Anatomy of one turn
- §3b. Long-horizon execution
- §4. Mind — an epistemic memory system
- §4a. Media vault & retention
- §4b. PIMS — the Registry (headline feature)
- §5. Soul — a relationship model, not a prompt
- §6. Hub — capabilities, models, and the Context Compiler
- §6a. The initial cast — inherited from Akita, proven in production
- §7. Trust — the substrate
- §7a. The Boundary Log — total I/O accounting
- §7b. Access tiers, Connect to Labs, and Sync
- §8. Portability — the Robot Package
- §8a. Instant start → Move (the killer flow)
- §9. Premises — four contours, one image
- §10. Surfaces — replaceable by design
- §10b. The built-in Chat
- §10a. The Dashboard — the control room
- §11. Multi-owner readiness

**PART II — PERFORMANCE & ECONOMICS**
- §2a. Scale envelope — one instance, 10,000 users a day
- §2b. Cost model — canonical per-server unit (USD)
- §2c. Speed budget — cloud-first performance targets

**PART III — ASSURANCE**
- §12. Evaluation — built into the runtime, aligned with the promises
- §13a. Risk register — honest and current
- §13d. Failure & recovery matrix
- §13e. Updates & release channels
- §13b. Vision conformance — promise by promise

**PART IV — STRATEGY & PLAN**
- §13. The stack — decisions validated by deep research (July 2026)
- §13c. Position in the market (July 2026)
- §14. V1 — prove one thesis
- §14a. Build plan — what green-light authorizes
- §15. The five hardest problems (in order)
- §16. What Labs owns

**APPENDIX**
- Appendix A — Core contracts (freeze the vocabulary)

---

# PART 0 — FOUNDATION



## 0. The constitution

One idea determines every technical decision:

**A Robot is a sovereign, durable digital entity owned by a person or organization. Models are stateless contractors.**

Five invariants follow. They outrank any database, language, or framework choice:

1. **The owner owns the canonical state.** Identity, memory, permissions, commitments, receipts, and history live inside an owner-controlled boundary. No model provider's conversation history is canonical.
2. **Models are stateless contractors.** A model receives the smallest sufficient context for one cognitive job and returns a proposal. It never directly modifies memory, acquires permissions, executes consequential actions, determines truth, claims something happened, or changes the Robot's personality.
3. **Actions are deterministic and durable.** Anything that touches the world passes through governed code: typed inputs, explicit authority, retries, verification, audit trail.
4. **Every claim has provenance.** The system distinguishes owner-stated, externally verified, model-inferred, preferred, predicted, attempted, verified, and unresolved — and never confuses them.
5. **The Robot is portable.** The same logical Robot moves between cloud, private infrastructure, a personal computer, and a stick without being re-created or retrained.

Two implementation laws sit on top:

- **Never let the model become the architecture.** Models think inside the Robot; they never define what it remembers, owns, may do, or claims to have done.
- **Never let the infrastructure break the promise.** If a component cannot travel to a laptop or a stick, it does not belong in the Robot.

---



## 0a. The product thesis — small, open, fast, upgradeable

The concept, vision, and mission of the product in one stance:

**The Robot runs on a curated set of the most advanced and smallest possible open-source and open-weight models** — each holding its seat for one exact job (§6a), nothing bigger than the task needs. This is not a cost hack; it is the product philosophy, and it delivers three promises at once:

- **Fastest** — small models answer in a breath; the speed budget (§2c) is won by right-sized brains, not bigger ones.
- **Lowest-cost operations** — a fraction of what bloated stacks burn (§2b); low cost is what makes a private Robot affordable to run for a person, a family, a community.
- **Fully manageable** — open weights mean no black boxes, no vendor mood swings, seats defended by golden-set evals (§12), and receipts that name which model did the work.

**Upgrades are one tap.** Any model on the market, for any type of task: a frontier heavyweight on hard reasoning, a specialist on documents, the fast open stack on everything else — chosen per task in the Dashboard (§10a, Models & Routing), with every receipt naming the model that acted. The open stack is the optimized default; the world's biggest models are an option, never a dependency.

**And the whole thing ships as a single binary** — one file that launches on almost any device: a cloud server, a private server, your own machine, eventually a stick in your pocket (§2, §8, §9) — delivering the best experience without compromising on speed, cost, control, or privacy.

**The stance in one line:** *the smallest open-source brains, tuned to deliver the best experience — fast, yours, and honest — with the world's biggest models one tap away. No compromises, no black boxes.*

---



# PART I — THE SYSTEM



## 1. Two planes

**Labs Control Plane** — the shared platform machinery: accounts and organizations, subscriptions, software distribution and signed updates, the skill registry, the model-provider catalog, fleet health, encrypted-backup coordination, marketplace and developer services. It runs on whatever infrastructure suits Labs. It **never requires access to unencrypted Robot state** — this is checkable in the code, not stated in a policy.

**Robot Sovereign Plane** — everything that makes a particular Robot that Robot: owner identity and authority, memory, relationship state, active workflows, credentials, policies, files, receipts, cryptographic identity. It runs wherever the owner chooses.

The split is the technical expression of the business: *Labs builds the platform; the owner owns the Robot.*



## 2. The Robot runtime — one binary, encrypted cells

The entire Sovereign Plane ships as **one signed Rust binary and encrypted SQLite cells**. The unit of storage is the **cell** — one encrypted SQLite file per principal partition. A personal Robot is exactly one cell; a shared Robot is a core file (identity, policies, shared memory) plus a keyring of member cells (§2a). "One binary, one file" holds precisely for the personal Robot — and generalizes, not breaks, at scale.

- **SQLite + FTS5 + sqlite-vec** carry structured memory, full-text search, and vector search in a single file. At personal scale this outperforms client-server stacks by orders of magnitude in latency and by infinity in operability: no ports, no daemons, no connection strings.
- Backup is a file copy. Export is the file. "Transferable" is a property of the artifact, not a feature to maintain.
- The organs — Prism, Soul, Mind, Hub — are **traits (interfaces) inside one process**, with strict code and data boundaries. A modular monolith plus isolated workers for skills; never microservices. Deploying twenty services would slow the company and make self-hosting miserable — and self-hosting is the trust anchor of the whole brand.
- The Control Plane may be heavy; the Robot must stay light enough to live in a pocket.

```
Robot Runtime (one process, encrypted cells)
│
├── PRISM — the kernel
│     intake · policy & permissions · planner · capability router
│     durable journal · verification · receipt compiler
├── SOUL — the relationship
│     perception · relationship state · expression policy · reflection
├── MIND — the substrate
│     event journal · facts & graph · semantic index · working context
│     commitment ledger · procedural memory · memory governance
├── HUB — the reach
│     model gateway & router · context compiler · connectors (MCP)
│     skill runtime · device interfaces
└── TRUST — the substrate
      identity & keys · secrets vault · encryption · data classes
      boundary log · audit · attestation (roadmap)
```



## 2d. The language boundary (law)

**Inside the binary, everything is English. At the surface, everything is the person's language.** One boundary, strictly enforced, so the deterministic machinery never loses context to translation drift:

- **English-only internally:** all system prompts, verdict schemas, plan/step/capability names, enum values, policy text, receipt claims, journal labels, lesson text, and inter-organ instructions. A verdict is `{"action":"reminder.create"}` regardless of what language the person spoke; the deterministic floor, the golden sets, and the evals all operate on one internal language — precision has one dialect.
- **Multilingual at every surface:** input in any language (text, voice, documents), perception detects language per turn, and **Soul renders every outgoing utterance in the person's language and register** — the boundary crossing happens exactly once, at expression, nowhere else.
- **Memory keeps both:** originals are stored **verbatim in their source language** (provenance means the actual words), with English-normalized derivatives (facts, summaries, entity names) alongside for deterministic matching; retrieval runs hybrid across both (FTS tokenizers cover the owner's languages; embeddings are multilingual by model choice). The Registry shows the person their own words, not a translation of them.
- **Why it's law:** every internal translation is a place to silently lose an instruction, a nuance, or a schema match. English-inside is not a preference — it is how the exact, deterministic approach survives contact with sixty languages.

## 3. Prism — the governed execution kernel

Prism is not an "agent orchestrator." It is a kernel: every request becomes an object with a lifecycle, every stage typed, every identifier immutable.

**Intent → Plan → Grant → Execution → Observation → Verification → Memory update → Receipt → Response.**

- **Intent** — what the Robot believes the owner wants: desired outcome, constraints, confidence, ambiguities, risk class.
- **Plan** — a versioned execution graph of steps with declared effects (`read`, `reversible_write`, `irreversible`), dependencies, and approval requirements. Never unstructured model prose.
- **Capability grant** — narrow, time-boxed authority: *may use `calendar.create`, on this calendar, for this event, until Friday, with no permission to invite others.* Never "email access."
- **Durable journal** — the defining mechanism. Every completed step is persisted to an append-only journal in the same database; after a crash the Robot resumes from the last completed step, never repeating a tool call, an outbound message, or a mutation. Implemented **in-process** (the durable-execution pattern, not a workflow cluster): a narrow, Robot-specific engine, small enough to audit, with irreversible effects gated through a transactional outbox.
- **Verification** — evidence that the intended effect occurred, gathered from the provider's response, not the model's confidence.
- **Receipt** — compiled from system evidence, never narrated by the model that acted. Each claim carries its evidence (provider, external object ID, timestamp, result hash). Statuses are honest: *proposed · awaiting approval · submitted · accepted · verified · failed · partial · uncertain.* Receipts are signed and render three ways: conversational ("Done — Tuesday at 10"), inspectable (who, what, when, what data left), machine-verifiable.

**The governing rule:** a sentence claiming an external effect may only be produced from a verified state transition. This kills the classic agent lie where "I've done it" means "I attempted a tool call," and it makes the Robot Laws checkable: an intent without a terminal receipt is a visible bug, not a silent drop.



## 3a. Anatomy of one turn

One real message through the machine, with the §2c clock running. *A member sends a voice note: "напомни мне завтра в 10 позвонить Марку насчёт контракта."*

1. **t=0 ms — Boundary Log (in).** Telegram long-poll delivers the update; the crossing is logged: source, channel, payload hash, size, trust tag `untrusted-by-origin`. The audio lands in the member's media vault (§4a, 7-day clock starts).
2. **t≈10 ms — envelope + journal.** The surface adapter normalizes to one event envelope; Prism opens a turn in the durable journal: `intent_open`.
3. **t≈20 ms — parallel fan-out.** Three things fire concurrently: ffmpeg extracts audio → STT API call (Boundary Log: out — provider, purpose `stt`, hash); the typing indicator streams to the surface (first visible response ≤1 s target met here); the member's cell warms (session context, recent focus).
4. **t≈900 ms — transcript in.** Boundary Log (in, untrusted); transcript journaled as the turn's text.
5. **t≈905 ms — deterministic floor.** Token scan: explicit time + reminder shape → this is the high-signal class the floor captures. No verdict model needed. (Had it been ambiguous: one verdict call, gemma-4-26b-a4b, ~400 ms, mood field included.)
6. **t≈910 ms — plan + grant.** Prism emits a typed plan: `reminder.create{when: tomorrow 10:00 @member_tz, about: "позвонить Марку насчёт контракта"}`, effect `reversible_write`, auto-approved by standing policy; a scoped grant is minted and journaled.
7. **t≈915 ms — execute + verify.** The reminder writes to the member's cell inside a transaction; the outbox confirms the state transition; verification is deterministic here (row exists, fields match). `outcome: verified`.
8. **t≈920 ms — receipt.** Compiled from evidence: claim "reminder set for tomorrow 10:00" ← row ID + timestamp + grant ID. Status: `verified`.
9. **t≈930 ms — Soul render.** Expression policy (member's register: RU, brief, warm) turns the receipt into one sentence: "Готово — напомню завтра в 10:00 позвонить Марку насчёт контракта." Verify pass (different model or deterministic check): the sentence claims exactly what the receipt proves. Sent; Boundary Log (out, surface).
10. **t≈940 ms — close.** Journal: `intent_closed(verified)`; commitment ledger holds the reminder with its fire-time; background lane queues the transcript for fact-extraction later. Total: under one second, every byte accounted for, nothing claimed that didn't happen.

The unhappy paths use the same skeleton: STT timeout → retry → "не расслышал, повтори?" with `intent_closed(failed)` and an honest receipt; a crash between 7 and 9 → replay resumes at the receipt, the reminder is not double-written (journal), the user still gets the confirmation.



## 3b. Long-horizon execution (lessons from the state-machine and deep-agent lines)

Six mechanisms, all journal-and-compiler work, no framework imports:

1. **Journal granularity rule.** Prism journals only at decision and effect boundaries — verdict, grant, tool call, outcome, receipt. Interior work (retrieval, context assembly, drafting) is never durably checkpointed. Durability where lying is possible; zero overhead where it isn't.
2. **Durable interrupts.** A plan step with `approval: required` parks the intent in the journal as `awaiting_approval`; the approval (Telegram button or Dashboard) resumes execution from that checkpoint — minutes or days later, across restarts, from any surface. Human-in-the-loop that survives reboots; no in-memory waiting, ever.
3. **Replay & fork.** Any past turn can be re-run in a shadow context, optionally with altered inputs or tool outputs; forks are read-only and never touch cells. One primitive, three tools: incident forensics, regression debugging, golden-set cases minted from real traffic.
4. **Plan-as-artifact.** For multi-step intents, the Plan lives as a compact todo artifact with progress marks, re-injected into context each step; steps check off against **receipts, not model claims**. Kills long-horizon drift and premature "done" — the two documented failure modes of every deep agent.
5. **Context offloading.** Compiler policy: any observation over a size threshold (fetched page, document, large tool result) is written to the working store; context receives a digest plus a handle; the model pulls ranges via a read tool on demand. Long documents stop blowing context; heavy turns get cheaper; the web READ loop scales to real research.
6. **Sub-intents with narrowed grants.** Prism can spawn a child intent: own journal branch, isolated context, a *subset* of the parent's grants (read-only, no network, etc.); only the summarized result returns. Use when a task exceeds ~10 steps, intermediates pollute context, specialization helps, or permissions should narrow. Deep research and document digestion are the first users.

Placement: 1–2 in M1, 4–5 in M2–M3, 3 and 6 in M4+.

## 4. Mind — an epistemic memory system

Mind answers not just *what do we know* but *who said it, when, how certain, still current, inferred or stated, usable in this context, allowed to leave the contour, contradicted by what, and inspectable/correctable/forgettable by the owner.*

Six stores, one schema, one file:

1. **Immutable event journal** — append-only history of everything significant: messages, tool calls, approvals, observations, memory mutations, workflow transitions, imports/exports. The source for audit and for rebuilding all derived state.
2. **Facts and the entity graph** — people, projects, places, accounts, documents, relationships — as relational tables. Every fact carries: value, type, **source pointer (foreign key to a journal record — "no knowledge without a source" is a constraint, not a guideline)**, confidence, validity interval, sensitivity, sharing policy, contradictions, superseded_by, owner confirmation.
3. **Semantic index** — embeddings and FTS over everything. Vectors are an access method, not the memory itself.
4. **Working context** — short-lived task state, expired and compacted independently of long-term memory.
5. **Commitment ledger** — a first-class subsystem: what the owner asked, what the Robot promised, deadlines, waiting conditions, delegations, why each commitment closed. The Second Law — never silently drop a request — cannot be implemented with chat history; it is implemented here.
6. **Procedural memory** — learned routines ("prepare my Monday brief this way," "above this amount, ask first"), versioned, testable, reversible.

**The mutation protocol:** models *propose* memory; Mind *decides* — nothing, tentative observation, contextual preference, stable preference, or owner-confirmed fact. Retrieval is one hybrid function (FTS + vector + graph + recency + salience, RRF-fused) used identically by answering, dreaming, and verification. Nothing hard-deleted in the journal; everything exportable; owner inspect/correct/forget as first-class operations, surfaced product-side as the Registry (§4b). **Erasure is cryptographic:** every cell is encrypted under its own key; right-to-erasure and member departure are executed by destroying the key (crypto-shredding) — the core file holds references, never content, so a dropped key leaves nothing readable anywhere. "Never forgets" and "can truly delete" coexist because they apply to different keyholders.



## 4a. Media vault & retention

Conversations are not only text. Every principal's cell includes a **media vault** — a per-person media folder holding original files, referenced by hash from the journal and the Boundary Log, encrypted like everything else in the cell.

**Inbound originals** — voice notes, images, videos, documents, any media the person sends — are kept in original form for a **default retention of 7 days** (owner-configurable per instance: shorter, longer, or forever). The point is workability: within the window, the Robot can re-listen, re-look, re-OCR, re-analyze at full fidelity — "what did that contract actually say," "play me back my note from Tuesday." After the window, the original expires but its **derivatives stay**: transcripts, extracted text, descriptions, thumbnails, embeddings live on as sourced records under normal memory rules — the memory survives, the bytes rotate.

**Outbound artifacts** — anything the Robot creates: generated images, videos, voice, podcasts, documents, HTML, code, MD files — are kept in the person's media folder for a **default of 30 days**, so everything produced in a chat remains retrievable and re-editable for a month without re-generation.

**Pinning beats every timer.** The person (or the Robot, on request) can pin any file — inbound or outbound — to keep it forever; pinned media becomes part of the vault proper and travels in the Robot Package (§8). Retention timers are floors for the unpinned, not ceilings for the kept.

**Mechanics.** Expiry is a background job in a low-priority lane; every expiration is journaled (what, when, why — policy, not accident); per-principal storage quotas surface in the Dashboard (People + System panels); departure and erasure follow the cell's crypto-shredding rules (§4). Storage math at canonical scale is small: ~1K DAU with voice+images ≈ tens of GB rolling — the NVMe budget of §2b already covers it.



## 4b. PIMS — the Registry (headline feature)

Everything the Robot stores about a person is governed by a built-in **PIMS — Privacy Information Management System** — the industry-standard term (ISO/IEC 27701 defines a PIMS as the governance layer for personal data), shipped for the first time *inside* a consumer AI product, for the user. Its product surface is **the Registry**: the Dashboard panel where you open the Robot's head.

**What the Registry shows — five categories, item by item:**
1. **Knowledge** — facts and derived conclusions about you, each with its source chain (the exact message or document it was learned from), confidence, and freshness.
2. **Instructions** — your rules and procedures the Robot follows ("above this amount, ask first"), versioned.
3. **Preferences** — style, tone, routing, and behavior settings, stated or inferred (marked which).
4. **Media** — everything in your vault (§4a) with retention state and pins.
5. **Grants** — every authority you've given, scope and expiry.

**What you can do with every item:** read it · see where it came from · correct it · confirm it · export it · erase it (crypto-real, §4). Nothing about you exists outside these five categories — and that sentence is checkable, because the categories are the schema, not a summary of it.

**Why it's a headline and not a settings page:** the proprietary systems structurally cannot offer this — their memory about you lives on their side of the wall: a partial list, a vague toggle, no source chains, no verifiable deletion, no view of how conclusions about you were formed. It matters most exactly where AI is most useful — health, money, family, legal — the topics people hold back because they can't see what's retained. Memory you can audit is the difference between confiding and leaking.

**The line:** *"Other AIs remember you somewhere you can't look. Your Robot ships a PIMS: every item, its source, your control."*



## 5. Soul — a relationship model, not a prompt

Four systems, deliberately not one growing personality prompt:

1. **Perception** — tone, urgency, emotional state, subtext, rhythm — always as *hypotheses with confidence*, never as facts about the owner.
2. **Relationship state** — familiarity, preferred directness, initiative level, formality, trust boundaries, sensitive topics.
3. **Expression policy** — how an already-grounded result is communicated: length, register, warmth, pacing, whether to ask or act, per persona dial and per surface.
4. **Reflection and evolution** — the Soul Loop: by day, every turn's signals (votes, corrections, mood, the member's own style) distill into source-linked *lessons* that feed expression; by night, a reflection pass reviews the day and produces a versioned *Soul revision* — tone parameters tuned within owner-set bounds, lessons retired or reinforced, a first-person journal entry written to the member's cell. Revisions are diffable and reversible (`/soul history`); larger shifts surface as proposals; the immutable core (owner persona, the Laws) is untouchable by evolution. Night work runs in the low-priority lanes (§2a) — the quiet hours are the Soul's study time.

**Scope note:** §5 defines Soul's *runtime contract* only. The full design — the persona dial, AEI framing, the complete Soul Loop and evolution mechanics — is specified in SOUL-MASTER; builders implement that document, through this interface.

Owner commands are product surface: *"Why are you speaking this way?" · "Show what you've learned about my style." · "Don't infer my mood." · "Be less agreeable." · "Restore last month's behavior."*

**The boundary:** Soul shapes interpretation and expression. It never overrides facts, permissions, policy, or verified outcomes — and it never claims to feel.

**Evaluator separation (law).** Verification never runs on the model that generated. Soul's response-verify pass, receipt-compilation checks, and eval grading each use a different model (or deterministic code) than the one that produced the output — because generators reliably grade their own work too generously, and a skeptical standalone evaluator is tractable where self-criticism is not.



## 6. Hub — capabilities, models, and the Context Compiler

**The Intelligence Gateway.** One canonical internal protocol for every model call: cognitive job + privacy classification + capability requirements + quality target + latency ceiling + cost ceiling + jurisdiction → selected endpoint. Endpoints span two LLM tiers — routed (OpenRouter-class aggregators) and frontier APIs — plus a minimal **local tier**: embeddings (bge-m3-class — milliseconds on CPU, faster than any API round-trip) and the deterministic floor. STT is outsourced (Parakeet via the router's audio endpoint): API transcription parallelizes across voice notes and keeps the server's cores free for orchestration — on a cloud-first instance, faster than transcribing locally under load. **Bender v0.2 ships no resident LLM** — every LLM role runs through the gateway to external providers; a resident model is a Chiron-line (v0.3+) option for GPU-capable premises, deliberately excluded here to avoid optionality over-engineering. All endpoints sit behind the same interface. The gateway keeps at least one direct-vendor path configured beside the aggregator as an ordinary fallback-chain entry, and the deterministic floor (§6a) is the declared zero-provider mode; providers are plumbing, and plumbing gets spares. **Layered routing, deterministic first:** eligibility filtering (private data → cloud eliminated; image job → text-only eliminated) → policy filtering → capability matching → learned ranking → fallback chain → outcome evaluation. The canonical record stores exact provider, model revision, parameters, and the context-disclosure manifest — because models change underneath stable product names.

**The Context Compiler — the most valuable proprietary system in the company.** Given a cognitive job, it selects applicable policies, relevant memories, relationship state, workflow state, tool schemas, and constraints; produces a provider-neutral intermediate representation; compiles it to the chosen model's format. Its goal is the *smallest sufficient context with provenance* — not more history in the prompt. It is also **cache-aware by construction**: context is laid out stable-prefix-first (system + policies + persona + long-lived memory, then volatile turn material) so provider prompt-caching hits maximally — realistically cutting input-token cost 30–70% on high-frequency roles, a direct lever on §2b. This is where Labs gets better forever without owning a model, and how the Robot remains recognizably itself while the cast rotates.

**Hub is the sole external gate.** Every external relationship of the Robot — OpenRouter and any model API, Serper, web fetch, MCP servers, connectors, webhooks, *and even the Telegram Bot API token* — is configured, held, and managed in Hub, and nowhere else. There is no second place to enter a key and no code path to the socket that bypasses it: Hub *is* the single instrumented gateway of §7a, so the Boundary Log is literally the record of what Hub did. In the product this is one section — Hub — showing every connection with its inputs and outputs, live. The default state is telling: **a Robot with no connectors configured is fully self-contained on its server** — it talks to no one and is reachable only through the built-in Chat (§10b). Telegram is an option you plug in, not an assumption.

**The Capability System.** Tools are narrow, typed capabilities (`gmail.create_draft`, `bank.prepare_payment`), never generic access. **MCP is the adapter at the boundary — both directions** (the Robot consumes MCP tools and exposes itself as an MCP server) — but never the internal architecture: Labs owns permission semantics, trust levels, capability manifests, approval rules, sandboxing, revocation, and receipts. An open protocol says how systems talk; it does not make a third-party server safe.

**Skills.** Every skill ships with a signed manifest declaring identity, version, capabilities, allowed network domains, data reads/writes, external disclosures, effect classes, and approval rules. Skills run **outside the Prism process** — isolated workers in V1, WebAssembly sandboxes when the marketplace opens — deny-by-default: no Robot database, no owner files, no unrelated network, no credentials, no raw model context. A skill receives only the temporary grants and data for that execution.



## 6a. The initial cast — inherited from Akita, proven in production

Roles are permanent; the cast rotates. Bender opens with Akita's **live production cast** (one OpenAI-compatible base, everything via OpenRouter), consolidated where Bender's design demands it:

| Role | Opening cast | Bender change |
|---|---|---|
| Chat / answers | `google/gemma-4-31b-it` | unchanged |
| Router / verdicts | `google/gemma-4-26b-a4b-it` | doorman + stage2 + plan **merge into one typed verdict call** |
| Extract / essence | `google/gemma-4-31b-it` | unchanged; runs in low-priority lanes |
| Escalation "super" | `nvidia/nemotron-3-super-120b-a12b` | unchanged; gated by task type |
| Escalation "ultra" | `nvidia/nemotron-3-ultra-550b-a55b` | quota-capped ≤2%; receipts name the tier used |
| Vision / OCR | `qwen/qwen3-vl-30b-a3b-instruct` | unchanged (no separate OCR API) |
| STT | `nvidia/parakeet-tdt-0.6b-v3` via audio API | stays outsourced — parallel transcription, zero core contention; ffmpeg extraction local |
| Embeddings | `openai/text-embedding-3-large` (3072-d) | **moves local** — small embedded model on CPU, into sqlite-vec |

**Non-LLM:** Serper for SERP (Exa path exists, inactive) · web fetch two-step: direct `fetch_page_text()` → Jina Reader fallback · Telegram long-polling · Qdrant retires in favor of **sqlite-vec inside the cell**. Keys live in the root-only secrets store, never in env files — carried into Bender's vault (§7).

**Reliability lessons Akita paid for, now law in Bender:**
1. **Hard timeouts on every network call** — the P1 hung-turn bug (a voice "what time is it?" that never got answered because an LLM call hung forever) is the founding trauma: connect + total timeouts everywhere; verdict ceiling ~2–3 s first attempt → one 5 s retry → deterministic fallback. The doorman may be wrong; it may never hang.
2. **Watchdog:** any turn "in" with no "out" within 60 s → error trace + owner alert. In Bender this graduates into the journal: an intent without a terminal receipt is an alarm by construction.
3. **Tolerant JSON handling:** strip fences, salvage the largest valid object, schema-validate; provider structured-output where available, salvage as fallback; invalid → one retry → fallback verdict.
4. **Deterministic floor:** zero-cost deterministic answers (time, self-questions, explicit reminders) run before any model call — the cheapest call is no call — and remain the offline floor when providers blink.
5. **Model swaps are gated by golden-set eval, never by vibes** — generalized by §12 to every cast change. Tier escalation moves from keyword/length heuristics to task-type rules with receipts naming the tier.



## 7. Trust — the substrate

- **Cryptographic identity.** Every Robot has a stable logical ID, an asymmetric identity key, rotating operational keys, signed releases, signed receipts, encrypted export packages. The owner can verify: this is my Robot, running approved code, and this receipt came from it.
- **Secrets.** OAuth tokens, API keys, credentials live in an isolated keystore and are injected directly into authorized tool execution. **They never enter model context.**
- **Data classes.** Every object carries classification: public · owner-private · sensitive · restricted · local-only · credential · derived · org-confidential.
- **Context packaging + egress ledger.** Before any external call, Prism assembles a temporary context package (task, selected evidence, redactions, pseudonyms, purpose, provider, retention, expiry). Every transmission outside the contour lands in a durable **egress ledger**: recipient, purpose, exact data categories, originating objects, redactions, authorization, result, timestamp. The owner can ask: *"For this reply, what left, to whom, and why?"* — and get an answer. "You control the communication" as an implemented right.
- **Networking.** Outbound-only everywhere on the ladder: long-polling, tunnels, Tailscale for owner access. Nothing listens to the internet.
- **Roadmap: confidential computing.** Labs Cloud Robots eventually run in attested enclaves — owner keys released only to approved runtime measurements, so Labs staff cannot inspect plaintext state. Designed-for now, shipped later.
- **Three record types, never conflated:** telemetry (engineering, sampled, PII-stripped before leaving the contour) · audit events (security, unsampled) · receipts (owner-facing evidence, unsampled).



## 7a. The Boundary Log — total I/O accounting

The privacy promise is not credible as a claim; it is credible as a ledger. So the binary keeps one: **every crossing of the process boundary is logged — both directions, no exceptions.** Not sampled telemetry, not selected receipts: the complete accounting of what entered and what left.

**Every OUTBOUND crossing** — model calls, tool calls, web fetches, search queries, Telegram sends, sync traffic, update checks, DNS-implied destinations — records: timestamp · direction · channel · counterparty (host, provider, model ID) · purpose (which intent/step required it) · authorizing grant · exact data categories and originating objects · redactions applied · payload reference + content hash · size · result. This is the egress ledger (§7) generalized to *everything*: if a byte left the contour, the log says where, why, under whose authority, and what it contained.

**Every INBOUND crossing** — user messages, tool results, fetched pages, SERP results, model responses, sync merges, update payloads — records the mirror: source · channel · payload reference + hash · size · and a **trust tag: everything inbound from the open world is marked untrusted-by-origin in the journal.** This is the injection defense made auditable: when a fetched page tries to steer the Robot, the log shows exactly what text arrived, from where, and what the Robot did next — provenance is the antidote to both leaking *and* injecting.

**Mechanics.** The log is append-only and **hash-chained** (each entry commits to the previous — tampering is detectable), stored inside the owner's contour like everything else; payloads live in the cell, the log holds references and hashes, so the log itself cannot become a second copy of your life. Retention is owner-policy. Enforcement is structural: all network I/O passes through one instrumented gateway in the binary — there is no code path to the socket that bypasses the log, and this is verifiable in the open source. The Dashboard renders it (panel 6); the owner's question "what has this thing sent and received today?" is answered exhaustively, not representatively.

**Why it earns trust:** anyone can claim "no leaks." Bender's claim is falsifiable: run it, open the Boundary Log, compare against your firewall — byte-for-byte accounting or it's a bug. Controlled, secured, private — as evidence, not adjectives.



## 7b. Access tiers, Connect to Labs, and Sync

### Three access tiers — security grows with attachment

- **Tier 3 — Local Slug (instant start, zero account).** At first boot the Robot prints a secret **slug URL** (`http://localhost:7777/a/<high-entropy-token>`) — opening it *is* authentication (the capability-URL pattern: Jupyter tokens, Plex claims). Guardrails: unguessable entropy, first-open binds a session cookie, rotatable/single-use from the Dashboard, and honestly scoped to **local networks and local launches** — not the open internet. Launch, click, talking in ten seconds.
- **Tier 2 — Standard (default).** **Magic link per device:** every device or server that should reach the system — a Labs Cloud tenant, your own machine, a stick's host — receives a link to the account email and enrolls a device credential. No passwords stored anywhere; recovery is the same flow from any new device. The honest trade, stated once at setup: email custody is the perimeter.
- **Tier 1 — Protected (most secure).** Magic link **+ a 6-digit PIN verified inside an HSM** under hardware attempt limits: the escrowed key releases only when both proofs pass. Even Labs, even under compulsion, cannot open the data — the iCloud-Keychain/WhatsApp custody model. For Robots that hold a life.

Upgrading is one click and nothing is reinstalled: slug → attach email → add PIN. The offline Recovery Kit (§13d) remains the zero-Labs recovery path at every tier.

### Key custody (how the tiers work underneath)

Every Robot generates its master key (KEK) locally, wherever it lives. The KEK is wrapped twice: locally (passphrase/credential — normal unlock) and, from Tier 2 up, **escrowed as ciphertext** with the Labs Key Service, held in an HSM with strict release rules (Tier 2: email proof; Tier 1: email proof + in-HSM PIN check, rate-limited in hardware). Labs holds *wrapped* keys and can never read them alone. Backups and sync payloads are encrypted client-side under the KEK before any byte leaves the contour — Labs stores opaque blobs.

### Connect to Labs — one button, and your Robot gets an address

Any Robot, anywhere — cloud, private server, local machine, USB stick — can attach to a Labs Account with one action: **Connect to Labs.** Technically the Robot opens a persistent **outbound** tunnel to the Labs relay (zero inbound ports, unchanged); the account binding makes that tunnel routable. What it buys:

- **robot.labs.co** (and per-account routes) reaches *your* Robot wherever it physically lives — laptop behind NAT, stick in a café, box at home.
- **@LabsRobot on Telegram** and future official Labs apps route the same way: Labs terminates the platform connection and forwards over your tunnel — your Robot needs no bot token of its own unless you prefer one (both modes live in Hub).
- The relay is a **router, not a reader**: session payloads pass end-to-end encrypted to the Robot; the relay sees routing metadata only, and the connection appears in the Boundary Log like any other channel.

Disconnecting is equally one action: the route dies, the Robot keeps working on its own surfaces.

### Sync — not backup: your data, ready for what's next

A connected Robot can enable **Sync**: periodic (hourly/daily — a policy dial, never per-turn) client-side-encrypted synchronization of the Robot's state to Labs storage, using the same changeset mechanics as premises sync (§9). Three things it unlocks: restore-anywhere (new device, magic link, resume), continuity across your premises with Labs as a rendezvous, and — the strategic one — **your data standing ready for future Labs services**: when a new service appears (Memory, Index, an app), you grant it access through your account and it reads from your synced state **under an explicit, revocable grant** — never by default, never raw, always in the Boundary Log. Sync is the bridge from one Robot to an ecosystem, with the keys staying yours.

## 8. Portability — the Robot Package

An open, versioned, documented format — the deep answer to "transferable":

```
robot.manifest
identity/        policies/       memory/
relationships/   commitments/    procedures/
artifacts/       receipts/       connector-config/
model-preferences/               migration-history/
```

Encrypted, signed, incrementally exportable, recoverable **without Labs' hosted service**. It separates three things: **essence** (identity, memory, procedures, receipts — travels), **environment bindings** (provider tokens, device config — excluded or rewrapped for the destination), and **runtime** (the signed binary, obtained per platform). With the one-file runtime, the package is nearly free — which is precisely why the single-file bet matters. "Robot on a stick" means the essence, the runtime, an optional local model, and honest re-binding of credentials on arrival.



## 8a. Instant start → Move (the killer flow)

The Robot Package (§8) is not only an export format — it powers the product's signature flow:

1. **Start in seconds.** Open **labs.co** or message **@LabsRobot** on Telegram — a Robot launches instantly on Labs Cloud: no install, no card, no setup. Try it, test it, live with it.
2. **Play with it the way you like — premises don't matter.** Everything is tweakable from day one, cloud or not: expand the mode (**solo → family → community → business** — invite people, set roles, stay solo, whatever fits); tweak the **Hub** — add and remove connectors, models, MCP servers, keys; shape the Soul, the quotas, the policies. The same Robot, the same controls, wherever it runs.
3. **Move whenever you want.** One button — **Move** — packs everything (memory, files, receipts, persona, revision history) into the encrypted Robot Package and restores it on **your own server, your local machine, or a USB stick**: the self-hosted binary imports the package, credentials rewrap (§8), and — if you stay connected — the route re-binds so @LabsRobot and robot.labs.co keep reaching *the same Robot at its new home*. Nothing re-taught, nothing left behind, the relationship uninterrupted.
4. **Or detach completely.** Full sovereignty is one more step: disconnect from Labs entirely — your own Telegram bot token, your own credentials, your own MCP access, all entered in *your* Hub — and the Robot runs with **zero connection to Labs**: no account, no relay, no sync, reachable through its own surfaces and the built-in Chat. Reconnect later if you ever want the routes and Sync back — the door swings both ways.
5. **Move back, or onward, anytime.** Cloud → server → laptop → stick → cloud: the flow is symmetric. The Cloud copy is deleted on move-out (crypto-erased, receipted) unless the owner keeps Sync on as an encrypted rendezvous.

This is openness and transferability delivered as a *flow*, not a settings page: the exit door is always unlocked, and walking through it takes minutes. It also resolves the classic hosted-vs-sovereign dilemma into a ramp: everyone starts where it's effortless; anyone can end where it's theirs — life, work, family, kid, community, business — wherever it feels right.

**Engineering notes:** Move = quiesce turns → final Sync changeset → package build → integrity check on the target (journal replay + one synthetic turn) → route re-bind → cloud crypto-erase with receipt. Gate: a Move completes in ≤15 minutes for a 10 GB Robot and is resumable if interrupted.

## 9. Premises — four contours, one image

- **Labs Cloud** — zero setup, managed updates, encrypted tenant isolation, optional confidential execution, owner-visible egress, full export. The mainstream door.
- **Private Cloud** — org-owned keys, data residency, private networking, IdP integration, governance. The business door.
- **Local Robot** — desktop or home-server runtime, local memory and files, API-routed intelligence by policy, encrypted remote backup.
- **Portable Robot** — the package plus runtime on removable storage. In v0.2, offline means the deterministic floor plus full access to memory and the vault (read, search, review — no model calls); LLM work resumes on reconnection. Portability and controlled execution — never the claim that a flash drive supplies inference.

Continuity between contours: SQLite-native encrypted replication (CRDT-based, offline-first) — stick ↔ VPS ↔ box, no central server required. Selective-sync contexts are also the clean mechanical basis for Atom and for "two loops, never mixed": a rented soul is a separate database with no sync path to the owner's Mind.



## 10. Surfaces — replaceable by design

Telegram, web, mobile, desktop, voice, devices are surfaces, not the Robot. A surface handles authentication, sensory I/O, streaming, presentation, approvals, receipt display, optional caching — and is never the only store of conversation or identity. Every message normalizes into one event envelope (surface, actor, modality, content, timestamp, device trust) before it reaches Prism. The Robot adapts expression per surface while remaining the same entity.



## 10b. The built-in Chat

Because the zero-connector Robot must still be reachable, the binary ships a **built-in web Chat**: a simple, fast conversation window served by the Robot itself (same embedded server as the Dashboard), with streaming replies, voice-note upload, file drop, and full media rendering. Access without inbound ports, same as §10a: localhost, the owner's tailnet, or the Cloud proxy; login via Labs Account magic link; every session end-to-end inside the contour. This is the Robot's **native surface** — the one that exists before and without any external platform: secure by construction, dependent on nobody's API, and always available for fast, private communication with your Robot. Telegram and every other surface are additions through Hub; the Chat is the floor.

## 10a. The Dashboard — the control room

A Robot without a window into itself is a black box, and a black box cannot honor "open, transparent, adjustable, transferable." The Dashboard is the owner's control room: the full overview of one Robot instance and the management surface for everything in it — including all the people who use it.

**Architecture: served by the Robot itself.** The Dashboard is an embedded web UI compiled into the same binary (static assets + local API), not a separate service — the single-binary promise extends to administration. Access without opening inbound ports: `localhost` on the host machine, the owner's tailnet (Tailscale) for remote access, and — on Labs Cloud — the Control Plane acts as an authenticated reverse proxy to the tenant's Robot. Login via Labs Account (@username, magic link); every Dashboard action is itself journaled and produces receipts, because administration is action too.

**Solo by default, shared by invitation — the owner commands everything.** Every Robot instance — on a local computer, a stick, a server, in the cloud — starts in solo mode. The owner may then open it to others: a team, a family, a community; the positioning doesn't matter, the authority model does. **The owner holds all administrative capability**: the full overview of everything happening on the instance, user management end to end (invite, quota, suspend, remove), keys, premises, policies, deletion, export. Members command their own partitions; the owner commands the Robot. The Dashboard is the UI over this principal/access-policy model (§11):

- **Owner** — full control: users, keys, premises, deletion, export.
- **Admin** — user management, connectors, quotas; no access to others' private partitions unless policy grants it.
- **Member** — a person using the Robot; sees and manages *their own* partition: their conversations, their memory, their grants.
- **Guest** — time-boxed, quota-boxed access; no persistent memory unless promoted.

Per-principal: invite (link/QR through the surface itself), suspend, remove-with-data-separation (their partition exports and departs cleanly), quotas (messages, model spend, tool calls), and per-user model policy. The owner's overview is total at the *operational* level — every user, every metric, every receipt, every byte of egress, all spend — while member *content* visibility follows the instance's declared policy, set by the owner and shown to every member on join, so authority is absolute and honest at the same time. The privacy rule is structural: **every conversation and memory object belongs to a principal's partition; cross-partition visibility exists only where an explicit policy says so** — a family Robot never leaks one member's conversation to another by context accident, and the Dashboard renders exactly what the viewer's role permits, nothing more.

**The panels (lightweight — ten screens, no more):**

1. **Overview** — health, uptime, queue depth, today's activity, model spend versus caps, sync status across premises, pending items needing the owner (approvals, proposals).
2. **People** — the principal roster above: roles, quotas, activity, invitations.
3. **Conversations** — review messages per partition (self always; others by policy), search, flag, annotate; every view logged.
4. **Registry (PIMS)** — the §4b surface: knowledge, instructions, preferences, media, grants — item by item with source chains, confidence, and freshness; read, correct, confirm, export, or erase inline. The owner rights of §4 as buttons, under the industry-standard name.
5. **Commitments** — the ledger live: open, waiting, done, dropped-and-why; the Second Law as a screen.
6. **Receipts & Boundary** — every action's evidence plus the full Boundary Log (§7a): every byte in and out of the binary — counterparty, purpose, grant, hashes. "What did you send, what did you receive, and why?" answered exhaustively, not representatively.
7. **Hub** — the single management surface for all external communication (§6): every connector with its live inputs/outputs, API keys and OAuth tokens (masked, vault-stored, never displayed post-creation), rotate/revoke, per-connector grants and disclosure policies, MCP servers, the Telegram token, skill manifests with declared capabilities.
8. **Models & Routing** — the cast behind the tiers, routing policy, spend caps, fallbacks, per-provider disclosure manifests; swap a model and the evaluation replay (§12) gates the change.
9. **Soul** — persona dial, bounds, lesson list, revision history with diffs and one-click rollback; "why are you speaking this way" made visual.
10. **System** — premises and sync pairing, encrypted backups, media-retention policy and vault usage (§4a), updates (signed releases), export (the Robot Package, one button), audit log, danger zone.

**Insights, honestly scoped.** Usage curves, retrieval quality, vote rates, commitment completion, egress volume — the evaluation metrics of §12 rendered for the owner. Aggregates across users are visible to owner/admin; drill-down into a person's content still obeys partitions. No engagement-maximizing analytics, ever — the Dashboard measures whether the Robot serves, not whether it addicts.

**What the Dashboard is not:** not a required surface (the Robot remains fully usable from chat alone — `/soul`, `/memory`, approvals all work in-conversation), not a separate product, and not a hole in the trust model — it is the trust model, made visible.



## 11. Multi-owner readiness

V1 ships single-owner, but **every memory and capability object carries a principal and an access policy from day one** — because a family, team, or business Robot is not "more users," it is governance: roles, delegated authority, information partitions, shared vs. private memory, guardian controls, departure procedures. Retrofitting principals later would mean rewriting Mind; carrying them from the start is cheap.



# PART II — PERFORMANCE & ECONOMICS



## 2a. Scale envelope — one instance, 10,000 users a day

This section sets the **Gear-2 ceiling** — what one instance must be able to carry; the *planning unit* is §2b (1K DAU). A shared instance (one Telegram Robot serving a team, a community, a business) must be able to carry **10,000 users/day × ~100 turns ≈ 1M turns/day** without abandoning the one-binary design. The math first: 1M turns/day averages ~12 turns/second with realistic peaks near ~100/second. That is not big data — it is a load profile a single well-built binary on one strong machine handles, if the storage layout is right. The design is **cellular**:

**Cell = partition = file.** Every principal's partition (§10a) is its own encrypted SQLite file: their conversations, their memory, their grants. The Robot core keeps one shared file: identity, shared memory, policies, the commitment ledger for shared work, the egress ledger. Consequences, all good:

- **Parallel writes.** SQLite's single-writer limit applies per file; 10,000 files means 10,000 independent write lanes. Per-partition writes are serialized (correct anyway — one person's turn order matters); cross-partition traffic runs concurrently. With WAL mode and batched transactions, each lane sustains thousands of writes/second on NVMe — three orders of magnitude above need.
- **Privacy becomes physics.** The partition boundary from §10a stops being a policy check and becomes a file boundary. A member's departure is "hand them their file." A subpoena-grade audit of one partition touches one file.
- **The pocket promise survives.** A personal Robot is simply an instance with one partition. Same binary, same schema, one cell.

**Runtime mechanics.** Async runtime (tokio); a connection pool with LRU-managed open handles (hot working set open, cold partitions opened on demand — the filesystem shrugs at 10K small files); per-partition actor serialization; a global concurrency budget with backpressure and fair queueing so one heavy user cannot starve 9,999 others; the durable journal shards with the partitions (each cell replays independently after a crash).

**The honest bottleneck is model throughput, not storage.** ~12–100 turns/second means 25–200+ concurrent model calls (verdict + answer call per turn). The gateway (§6) is built for it: per-provider concurrency limits and token budgets, hedged fallbacks, priority classes (interactive > background > dreaming), and per-user spend quotas enforced by the Dashboard (§10a). Dreaming and indexing run in low-priority lanes that yield to live traffic.

**Three gears, one architecture.** Gear 1 — personal: one cell, any hardware down to a stick. Gear 2 — shared: the cellular layout above, one strong machine, to ~1M turns/day. Gear 3 — beyond (multi-instance fleets, 100K+ users): the same traits backed by server-grade storage in a cloud contour, orchestrated by the Control Plane — a premises choice, not a rewrite. The gear is set per Robot; the promises hold in every gear.



## 2b. Cost model — canonical per-server unit (USD)

**Canonical assumptions, fixed for all estimations:** one server handles **3,000 registered · 2,000 weekly active · 1,000 daily active** users at ~100 interactions per DAU ≈ **100K turns/day**, 80% inside a 16-hour window → ~1.4 turns/sec sustained, ~3–5/sec peak. Per turn: one verdict call, one answer call, ~30 DB ops, one embedding; ~15% of turns trigger web search; escalation super ~7% / ultra ~3% (Bender target: ultra ≤2%). Prices: current OpenRouter list, rounded. **TO-VERIFY against Akita logs before budgeting: (a) real router-model calls per turn (assumed ~2), (b) real super/ultra escalation shares — the vendor total swings ±40% on these two.**

**Inference on the live cast (all-API):**

| Role | Model | Volume/day | Est. $/day |
|---|---|---|---|
| Router / verdicts | `gemma-4-26b-a4b-it` ($0.045/$0.15 per M) | ~2 calls/turn × 1.5K in / 150 out | ~$18 |
| Chat answers (~85%) | `gemma-4-31b-it` ($0.10/$0.30) | 85K × 3K in / 350 out | ~$35 |
| Extract + essence | `gemma-4-31b-it` | ~0.5 call/turn × 2K / 200 | ~$13 |
| Escalation "super" (~7%) | `nemotron-3-super-120b-a12b` ($0.30/$1.20) | 7K × 3K / 500 | ~$10 |
| Escalation "ultra" (~3%) | `nemotron-3-ultra-550b-a55b` ($1.50/$6) | 3K × 3K / 600 | ~$25 |
| Vision / OCR (~5%) | `qwen3-vl-30b-a3b-instruct` | 5K calls | ~$2.5 |
| STT (~20% voice) | `parakeet-tdt-0.6b-v3` via API | ~10K audio-min | $5–30 |
| Embeddings | `text-embedding-3-large` ($0.13/M) | ~80M tokens | ~$10 |
| **Inference total** | | | **~$120–145/day → $3.6–4.4K/mo** |

**Non-LLM APIs:** Serper ~15K queries/day ≈ **$5–15/day ($150–450/mo)**; Jina Reader fetch fallback free; Telegram free; Tailscale ~free.

**Server-side & operations:**

| Item | Spec | $/mo |
|---|---|---|
| App server | AX52-class (8c / 64 GB / NVMe) is sufficient (<10% CPU at peak); AX102-class for headroom | $65–120 |
| Staging / shadow deploys | shared small box across the fleet | ~$30 (share) |
| Backups | encrypted snapshots + offsite object storage (cells + journal, ~50–300 GB) | $15–30 |
| Monitoring / logs | self-hosted | ~$0 |
| **Infra total** | | **~$110–180/mo** |

**Bender's cuts (before any GPU):** one verdict call instead of ~2 router calls (–$9/day) · embeddings local on CPU (–$10/day) · ultra capped ≤2% by quota (–$8–10/day) · prompt-cache-aware context layout (§6) on high-frequency roles (input-token cost –30–70% where caching applies). STT stays API for speed ($5–30/day accepted). **All-API Bender: ~$90–115/day ≈ $2.7–3.5K/mo.**

**Canonical unit economics:** all-in (inference + search + infra) ≈ **$3–3.8K/mo per server** → **~$3.0–3.8 per DAU per month**, or ~$1.5–1.9 per weekly-active, ~$1.0–1.3 per registered. Cost is ~97% tokens: the two dials are the ultra-escalation share and the answer-tier model choice.

**The GPU lever at this scale — optional, not mandatory.** The open-weight Gemma tier is ~$60/day (~$1.8K/mo); a single owned RTX 5090-class card (~$2.5–3K, amortized ~$130/mo) absorbs it at 3–5 turns/sec peak → total drops to **~$1.3–1.8K/mo (~$1.3–1.8/DAU)**, payback ~2 months. Worth it per-server only if the fleet standardizes on GPU hosts; at 10× scale (10K DAU/server-cluster) it becomes mandatory economics — costs scale linearly with turns, so multiply this table by DAU/1,000.

**Carry in the head:** one canonical server ≈ **$150/mo of iron carrying ~$3K/mo of tokens** — the machine is 5% of the bill; the escalation dial and the Gemma tier's home decide the rest.

**Vendor monthly total, per canonical server (the planning number):**

| Configuration | $/mo all-in |
|---|---|
| Today's Akita stack, no cuts | ~$3,900–5,000 |
| **Bender, all-API (after the four cuts)** | **~$2,900–3,700 → plan on ~$3.5K** |
| Bender + one owned 5090 (Gemma tier local) | ~$1,500–2,000 |

~95% of the number is tokens. Planning consequences: vendor breakeven ≈ **$3.5/DAU/mo**; any paid tier above ~$5–7 per active user per month is margin-positive even all-API; the GPU option roughly halves the vendor cost once the fleet standardizes on GPU hosts. Fleet math is linear: N servers ≈ N × $3.5K (all-API) or N × $1.75K (GPU-absorbed).



## 2c. Speed budget — cloud-first performance targets

Bender v0.2 runs fully in the cloud for the user; the server is a **pure orchestrator** — it computes nothing heavy and coordinates everything, so perceived speed is an orchestration problem. Targets, measured at the surface (what the person feels in Telegram/web):

| Metric | Target |
|---|---|
| First visible response (typing indicator / first streamed tokens) | **≤ 1.0 s p50** |
| Full answer, routine turn | **≤ 3 s p50 · ≤ 6 s p95** |
| Voice note (transcription + answer) | + 1–2 s over text |
| Deterministic-floor turns (time, self, explicit reminders) | ≤ 300 ms |
| Escalated turns (super/ultra) | streamed, first tokens ≤ 2 s, labeled in-flight |

The techniques, all orchestration-side:

1. **Stream everything.** Tokens render to the surface as they arrive (Telegram draft-edit streaming; SSE on web). Perceived latency is time-to-first-token, and TTFT is won by streaming, not by faster totals.
2. **Parallel fan-out.** On message arrival: verdict call, embedding, memory retrieval, and (when the verdict is predictable) a speculative answer-context build run **concurrently**, not sequentially. The critical path is max(), not sum().
3. **Cache-stable prefixes.** The Context Compiler's stable-prefix layout (§6) makes provider prompt-caching hit on every turn of a session — caching cuts cost *and* prefill latency.
4. **Latency-aware provider routing.** The gateway selects provider variants by measured p95 (fast-throughput endpoints where the router offers them), keeps warm HTTP/2 pools per provider, and **hedges the p99**: if the primary hasn't produced first tokens by a deadline, fire the schema-identical fallback and take the first responder.
5. **Deterministic floor first.** The cheapest call is no call — the ≤300 ms class of turns never touches a model.
6. **DB off the critical path.** Journal writes that must precede an utterance (receipts law) are batched single transactions on NVMe (<1 ms); everything else — derivatives, rollups, embeddings persistence — lands in low-priority lanes after the reply is already streaming.
7. **Speed is a §12 metric.** TTFT and p95 turn latency sit in the eval suite next to MISROUTE; a model or provider swap that wins quality but loses the latency budget does not ship.

Proof-of-concept consequence: the fastest private AI is the honest demo — a Robot that answers in a breath *while* every byte is grant-checked, journaled, and receipted is the concept proven; the Boundary Log costs microseconds, and the speed budget makes that visible.



# PART III — ASSURANCE



## 12. Evaluation — built into the runtime, aligned with the promises

- **Human:** tone match, conversational repair, verbosity match, emotional overreach rate, cross-surface continuity.
- **Personal:** retrieval precision, contradiction rate, correction persistence, false-memory rate, commitment completion, creepy-context rate.
- **Private:** unauthorized-disclosure rate, unnecessary-context rate, secret exposure, deletion effectiveness, provenance coverage.
- **Reliable:** verified task success, false-completion claims, duplicate effects, crash recovery, abandoned commitments.

**Public benchmarks.** Mind runs LongMemEval and LoCoMo — the field's de facto memory stress tests, with temporal reasoning as the axis that separates leaders — on every release, and the numbers are published. Discipline and marketing in one artifact: "inspect our memory scores" is a sentence no closed competitor can say.

**Harness security evals.** The suite tests Prism itself, not just models: prompt-injection through tool results and fetched pages, over-tooling (the agent reaching for capabilities the task doesn't need), timeout behavior under provider brownouts, permission-escalation attempts across partitions, and secret-leakage probes. The harness is attack surface; it gets its own red team.

Every release — **including model swaps and harness-level changes (prompt wording, cache headers, default parameters — each documented in the field as capable of compounding into visible regressions)** — replays against a consented, sanitized corpus before rollout.



## 13a. Risk register — honest and current

1. **SQLite write concurrency.** One writer at a time per file — answered structurally by the cellular design (§2a): partition-per-file gives one write lane per user, WAL + batched transactions per lane, background jobs (dreaming, indexing) in yielding low-priority lanes. Verified envelope: ~1M turns/day per instance on one strong machine; beyond that, Gear 3 (server-grade storage behind the same traits) is a premises choice, not a rewrite.
2. **Wasmtime CVEs.** Real but well-managed (rapid coordinated patches; Rust memory safety; Miri-tested unsafe). Mitigation: auto-update channel for the runtime, manifest-level grants as the second wall, fuel limits, no raw model context in guests.
3. **SQLCipher build friction on Windows.** Vendored-OpenSSL feature or SQLite3MultipleCiphers fallback; CI builds all three OS targets from day one.
4. **CRDT sync semantics.** LWW columns can silently drop concurrent edits. Mitigation: sync is scoped to CRDT-suitable tables (facts, items, journal — append-heavy); the step journal itself is grow-only; contested fields use multi-value registers surfaced to the owner, per the memory governance protocol.
5. **Model-behavior drift.** Models change under stable names; the Context Compiler + evaluation replay treat every model swap as a runtime change (Section 12) — this is a process risk, closed by discipline, not code.
6. **Aggregator dependence.** The opening cast routes through one aggregator; an outage would mute the routed tier. Ordinary engineering, not a feature: a direct-vendor fallback in the chain, and the deterministic floor as zero-provider mode.
7. **Labs Cloud key custody, pre-enclave.** Until confidential computing ships, cloud-hosted cells are decrypted inside Labs-operated runtimes — policy trust, said out loud. Self-hosted contours never have this gap; the enclave roadmap (§7) closes it in Cloud. Honesty here strengthens the trust story.
8. **Host hygiene on portable premises.** A hostile host can observe a running session; stated openly, mitigated by the paired-contour pattern (stick = master copy, brain at home over Tailscale) — sovereignty grades are honest: Flash < local < LPC.



## 13d. Failure & recovery matrix

| Failure | Detection | Behavior | User experience | Recovery |
|---|---|---|---|---|
| Router/provider outage | first-token deadline missed | hedge to fallback vendor; else deterministic floor | slower turn, or honest "having trouble thinking" | automatic on provider return |
| Telegram outage | long-poll errors | queue outbound in outbox; web surface unaffected | delayed delivery, nothing lost | drain outbox on reconnect |
| Crash mid-turn | journal replay on boot | resume from last completed step; no repeated effects | reply arrives late, once | automatic |
| Disk full | write-error + watermark alarm | shed background lanes first; refuse new media; never corrupt cells | "storage full" notice to owner | owner expands / prunes via Dashboard |
| Cell corruption | SQLite integrity check on open | quarantine cell; restore from last snapshot; journal replays the gap | that member paused briefly | automatic + owner notified |
| Sync conflict | CRDT multi-value register | both values kept, surfaced per memory governance | Registry shows "conflicting — pick one" | owner/member resolves |
| Host dies | uptime monitor | restore snapshot + package on new host | downtime, no data loss inside backup interval | documented runbook, one binary + files |
| **Owner loses passphrase** | — | **stated loudly: no backdoor exists** | — | **Recovery Kit only** (below) |

**The Recovery Kit — the key-loss decision, made explicit.** At setup, the Robot generates a one-page Recovery Kit: a recovery code (second key wrapping the cell keys) the owner prints or stores offline, and optional Shamir 2-of-3 shares for family/team instances. **If the passphrase and the Kit are both lost, the data is gone — by design, and the product says so at setup in plain words.** Crypto-shredding that can erase for real is the same property that cannot un-lose keys; Bender chooses honesty over a backdoor, and the Kit is the mitigation.



## 13e. Updates & release channels

For a self-hosted fleet, updates are the security model — the CVE mitigations in §13a assume they flow. The spec:

- **Channels:** `stable` (default) and `canary` (owner opt-in). Control Plane serves signed releases; the binary verifies signatures before applying — unsigned or tampered updates do not install, anywhere.
- **Staged rollout:** canary → percentage waves on stable; fleet health (crash rate, turn success) gates each wave automatically.
- **Self-update with self-rollback:** the binary updates itself atomically (new version alongside, switch on success); a failed post-upgrade health check (boot + journal replay + one synthetic turn) rolls back automatically and reports.
- **Schema migrations** are versioned, forward-only, journaled, and run per-cell on first open after upgrade — a half-migrated fleet is a normal state, not an emergency.
- **The owner's rights:** pin a version, defer updates, read every release's signed changelog — with the trade stated plainly: pinning means owning your own patch latency. Security-critical releases are flagged as such.
- **Every update is a journaled, receipted action** like any other — the Dashboard System panel shows what changed, when, and by whose policy.



## 13b. Vision conformance — promise by promise

- **Human (no learning curve):** Soul's four systems + the INVARIANT + surfaces meeting people in apps/chats/devices/messengers. Nothing in the runtime requires syntax, forms, or setup ceremony; Telegram long-polling means "say hello and it works."
- **Personal (never introduce yourself twice, never forgets):** Mind's provenance-constrained facts, commitment ledger, procedural memory, hybrid retrieval; the mutation protocol keeps "never forgets" accurate rather than creepy; the Robot Package makes the accumulated self portable forever.
- **Private (you control data, communication, storage):** SQLCipher file the owner holds · the Boundary Log (§7a): hash-chained, byte-complete I/O accounting in both directions · egress ledger + context packaging on every external call · capability grants + secrets never in model context · outbound-only networking · selective-sync contexts · four premises down to a stick. Each control is an artifact or a ledger entry — inspectable, not promised.
- **The Laws:** First (owner's life better) — evaluation suite is the Laws made measurable; Second (never silently drop) — commitment ledger + journal make drops visible; Third (adapt to its human) — Soul evolution within bounds + procedural memory.



# PART IV — STRATEGY & PLAN



## 13. The stack — decisions validated by deep research (July 2026)

Every load-bearing bet was checked against the current state of the field. Verdicts:

**Storage: SQLite + FTS5 + sqlite-vec — CONFIRMED, industry-converged.** The 2026 personal-agent ecosystem (OpenClaw and its forks, Engram, sqlite-memory) standardized on exactly this: single-file, zero-ops, hybrid FTS+vector retrieval, sub-millisecond recall at personal scale. The named boundary where SQLite stops fitting — shared recall across many users, cross-device continuity handled by hand-rolled sync glue — is precisely what our CRDT layer and one-DB-per-Robot design avoid. Personal scale (≤1M records) is comfortably inside the envelope.

**Encryption at rest: SQLCipher via rusqlite's `bundled-sqlcipher` — CONFIRMED, with a fallback.** Transparent AES-256 over the whole file (tables, indexes, journals); queries unchanged; key supplied at open from the OS keyring / owner passphrase; the on-disk file is opaque without it. Known friction: OpenSSL linkage on Windows builds — mitigate with `bundled-sqlcipher-vendored-openssl`. Fallback candidate if licensing or build pain grows: SQLite3MultipleCiphers (VFS-based, tracks upstream SQLite closely). Decision: SQLCipher now; the storage trait keeps the cipher swappable.

**Durable execution: our own narrow journal, in-process, in the same SQLite file — CONFIRMED as the 2026 consensus for self-contained agents.** The field now states our position explicitly: start with SQLite-backed durable workflows and "skip Temporal until you need it"; DBOS itself — the reference for library-style durable execution — added a SQLite backend and integrations with the OpenAI Agents SDK and Pydantic AI ("embeds as a lightweight library, no external orchestrator required"). Public reference implementations (e.g. Morling's SQLite DE engine) validate the design for exactly our case: "a self-contained agentic system." We build the ~2K-line Robot-specific engine: append-only step log, deterministic replay, transactional outbox for irreversible effects. No cluster, ever, in the Sovereign Plane.

**Premises sync: SQLite-native CRDT replication — CONFIRMED, now production-grade.** Local-first sync matured decisively in 2026: cr-sqlite established column-clock CRDTs for SQLite; sqlite-sync (SQLiteAI) ships causal-length/add-wins/delete-wins/grow-only semantics, offline-first merges with no central coordinator, explicitly marketed for AI-agent memory — and its **selective sync contexts** (sync only a named context, keep private memory separate) are the exact mechanical primitive for Atom and for "two loops, never mixed." Decision: adopt the cr-sqlite/sqlite-sync approach behind a Labs sync trait; encrypt transport with the Robot's keys; pair stick ↔ VPS ↔ box.

**Skill sandbox: Wasmtime (WASM Components + WASI) — CONFIRMED, with eyes open.** Wasmtime is the reference secure runtime: deny-by-default imports, memory isolation, fuel metering and epoch interruption for untrusted code, Component Model with typed WIT interfaces. Microsoft's **Wassette** (a security-oriented runtime that runs WebAssembly Components *via MCP*, with capability permissions) independently validates our exact pattern — skills as WASM components exposed through MCP with per-capability grants. Sober note: April 2026 advisories showed LLM-assisted auditing uncovering real Wasmtime vulnerabilities (promptly remediated, strong security process) — so sandbox ≠ absolution: keep the manifest/grant layer and treat WASM as one wall of several. V1 ships trusted first-party skills in isolated processes; the WASM runtime lands with the marketplace.

**Resident LLM: NOT in v0.2 — deferred to Chiron.** The research verdict stands (mistral.rs — pure-Rust, embeddable, GGUF, near-llama.cpp performance — is the right vehicle when the time comes), but the decision for Bender is *no resident LLM at all*: the canonical server is CPU-only, where a chat-class model cannot serve concurrent load (CPU inference doesn't batch; ~2 concurrent turns would saturate it), and carrying the option for premises that could run one adds a second inference stack, a model-distribution channel, and a test matrix — over-engineering for a version whose thesis is API-routed intelligence. What runs locally in v0.2 is only what is *faster* local: the embedding model (single-digit ms on CPU vs. 100–300 ms API round-trip) and the deterministic floor. STT stays outsourced — API transcription parallelizes and keeps cores free. The v0.2 server is a pure orchestrator: it computes nothing heavy, it coordinates everything. Ollama remains usable as an ordinary *external* endpoint for users who already run it; nothing ships inside.

**The rest, unchanged:** Rust core; trait-based organs; routed tier (OpenRouter/Fireworks-class) + frontier APIs behind one gateway; MCP adapters both directions; TypeScript for Control-Plane services; per-Robot keys, hardware-backed where available; OpenTelemetry internally, PII-stripped before leaving the contour.

**Explicitly not built:** microservices, Kubernetes in the Sovereign Plane, a custom vector DB, a workflow cluster, a plugin DSL (MCP won), model training, and any memory write path that skips provenance.



## 13c. Position in the market (July 2026)

The category Bender enters was proven — and its gaps were named — by the OpenClaw wave: a self-hosted assistant living in your messaging apps reached 380K+ GitHub stars in months, spawned an ecosystem of forks and managed clones, and then demonstrated the failure mode: hundreds of unresolved security issues, unvetted community skills, broad tool permissions, weak sandbox boundaries — the market's own conclusion being that autonomous-without-controlled is dangerous and the winners will combine autonomy with safety. The consolidation forecast (a handful of managed players plus an open-source foundation by 2027; memory and context replacing model size as the moat; an efficiency race already producing sub-megabyte agents) plays directly to Bender's bets: one binary, cells, provenance, grants, receipts.

**The positioning sentence: the category leader proved the demand and shipped the security crisis; Bender is the same category with the authority model built in.** Every OpenClaw gap is a Bender design decision: broad permissions → scoped time-boxed grants; unvetted skills → signed manifests + sandbox; silent actions → evidence-grade receipts; one shared context → encrypted per-principal cells; "local but leaking" → the egress ledger and honest sovereignty grades. And where the field's memory tools are libraries a developer bolts on, Mind is the substrate the Robot lives in — benchmarked publicly (§12), governed by a built-in PIMS the user can open (§4b), portable by format (§8). No proprietary assistant ships an inspectable registry of what it knows about you; Bender leads with one.



## 14. V1 — prove one thesis

*After several weeks, my Robot understands me, remembers correctly, completes work reliably, and I trust it with more of my life — not less.*

Ship: **one Robot, one owner, many members** — solo by default on any premises, shareable by invitation as a team / family / community instance; the owner holds full administrative command (overview of everything, user management, keys, policies) per §10a · two surfaces (Telegram + web, voice within both) · four capabilities done excellently — personal memory, web research, calendar, email (files as fifth) · core Prism (intents, typed plans, grants, durable journal, commitment ledger, verified receipts) · core Mind (journal, sourced facts, hybrid retrieval, corrections, export/inspect/forget) · initial Soul (communication profile, gradual bounded adaptation, inspectable) · gateway (the §6a cast + one direct-vendor fallback + one local path, deterministic policies) · two deployment modes: **Labs Cloud and a self-hosted single binary** — the same Robot Package runs in both · **Dashboard v1** — the embedded control room with five panels first: Overview, People, Receipts & Boundary, Hub, Registry (PIMS).

**Postponed until retention is proven:** open marketplace, Index, proprietary hardware, consumer stick SKU, multi-*owner* governance (families with guardians, org admin, delegated authority — members are in, co-owners are not), autonomous financial actions, child accounts, enterprise admin, agent-building environments, custom models.



## 14a. Build plan — what green-light authorizes

Bender is built beside Akita, not from it: no migration, no cutover; it launches as its own instance while Akita keeps evolving. Milestones, each with a gate:

- **M0 — Skeleton.** One binary boots; cell open/create (SQLCipher); the durable journal writes and replays through a kill-test. *Gate: crash mid-turn, resume without repeat.*
- **M1 — Prism lifecycle.** Intent → plan → grant → execute → verify → receipt on one capability (reminders). *Gate: no utterance without a terminal receipt, proven by test.*
- **M2 — Mind.** Journal + sourced facts + hybrid retrieval (FTS + sqlite-vec + RRF) + commitment ledger. *Gate: retrieval quality ≥ Akita's on the golden set; zero unsourced facts by constraint.*
- **M3 — Gateway + cast.** §6a cast wired, one verdict call, timeouts, fallback chain, deterministic floor. *Gate: Akita's routing corpus at MISROUTE-0; p95 turn latency ≤ Akita.*
- **M4 — Soul + Dashboard v1.** Communication profile, bounded adaptation; five panels. *Gate: §12 Human/Private metrics baselined.*
- **M5 — Members + launch.** Partitions, roles, quotas; Telegram + web live. *Gate: canonical-server load test (100K synthetic turns/day sustained 48h, zero dropped intents).*

Discipline throughout, inherited from Akita: golden-set evals gate every milestone and every cast change; shadow-then-flip applies to Bender-internal changes; design-doc-per-epic; tests colocated; the corpus grows before the feature does.



## 15. The five hardest problems (in order)

1. **Reliable personalization over time** — not storing memory, but deciding what deserves to become memory, when it applies, when it's stale, and when using it would feel wrong.
2. **Trustworthy action** — exactly-once effects are impossible across arbitrary APIs; engineer idempotency, reconciliation, approvals, uncertainty states, truthful receipts.
3. **Behavioral continuity across models** — the Robot stays itself while the cast changes: owned state + Context Compiler + evaluation + Soul's realization, never a provider's system prompt.
4. **Privacy with external intelligence** — selective context assembly, redaction, purpose limitation, egress visibility as central product systems.
5. **Portability without lowest-common-denominator design** — identity and state preserved everywhere; performance allowed to differ.



## 16. What Labs owns

Not intelligence. Five things model vendors cannot supply: **Identity** (who this Robot is) · **Context** (what it knows and why) · **Authority** (what it may do) · **Continuity** (how it remains itself over years) · **Accountability** (proof of what it did).

---



# APPENDIX



## Appendix A — Core contracts (freeze the vocabulary)

The four organ traits (signatures abridged; canonical form lives in the repo):

```rust
trait Prism { fn intake(&self, env: Envelope) -> IntentId;
  fn plan(&self, i: &Intent) -> Plan;
  fn execute(&self, p: &Plan, g: &[Grant]) -> Vec<Outcome>;
  fn verify(&self, o: &Outcome) -> Verification;
  fn receipt(&self, i: &Intent) -> Receipt; }

trait Soul { fn perceive(&self, env: &Envelope) -> Signals;        // hypotheses + confidence
  fn render(&self, r: &Receipt, s: &Signals) -> Utterance;         // grounded → human
  fn verify_expression(&self, u: &Utterance, r: &Receipt) -> Verdict;
  fn reflect(&self, day: DayLog) -> Vec<Proposal>; }               // bounded, versioned

trait Mind { fn recall(&self, q: Query) -> Context;                // FTS+vec+graph, RRF
  fn absorb(&self, ev: &Event) -> Vec<MemoryProposal>;             // proposes, never writes
  fn commitments(&self) -> CommitmentLedger;
  fn registry(&self, principal: PrincipalId) -> RegistryView; }    // §4b PIMS

trait Hub { fn tools(&self) -> Registry;
  fn call(&self, t: ToolId, args: Args, g: &Grant) -> Outcome;     // via Boundary Log
  fn infer(&self, job: CognitiveJob) -> ModelResponse; }           // via Context Compiler
```

The five core objects (canonical JSON, abridged):

```json
Intent   {"intent_id","principal","desired_outcome","constraints":[],"confidence","risk_class","status"}
Plan     {"plan_id","intent_id","steps":[{"step_id","capability","effect":"read|reversible_write|irreversible","approval","deps":[]}]}
Grant    {"grant_id","capability","scope":{...},"principal","expires_at","issued_by"}
Receipt  {"receipt_id","intent_id","status":"proposed|submitted|accepted|verified|failed|partial|uncertain",
          "claims":[{"claim","evidence":[{"type","provider","external_id","hash","ts"}]}],
          "models_used":[],"data_disclosures":[],"signature"}
Boundary {"entry_id","direction":"in|out","channel","counterparty","purpose","grant_id",
          "categories":[],"payload_hash","size","trust":"owner|granted|untrusted","prev_hash","ts"}
```

Rule: these names are the vocabulary of the codebase, the Dashboard, and the docs — one dialect, everywhere.

---

**The architecture in one sentence:** *Labs is a portable, owner-sovereign AI runtime — one signed binary and encrypted cells — in which Prism governs every action through a durable journal, scoped grants, verification, and evidence-grade receipts; Soul maintains the relationship within bounds; Mind maintains knowledge with provenance; and Hub supplies replaceable intelligence through a context compiler.*

