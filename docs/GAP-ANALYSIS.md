# Architecture vs. what exists — a gap analysis

Read of `labs-robot-v0_2-bender-architecture.md` (761 lines) against the
code as it stands, 2026-07-31. Every "built" below was checked in the
source, not inferred from a BUILD-LOG entry.

**Headline.** The spine is real and the promises that are hardest to retrofit
are the ones already keeping. What is missing is mostly *reach* — the
capabilities, surfaces and platform layer that turn a proven kernel into a
product someone can live in. That is the good failure mode: the expensive
foundations are down, and the remaining work is mostly additive.

Rough proportion, by weight rather than line count: **the runtime is ~70%
there; the product around it is ~25%.**

---

## 1. What is built, and holds under test

| Area | Evidence |
| --- | --- |
| **One binary, encrypted cells** (§2) | SQLCipher per principal, WAL, one process. Verified by the two-location test. |
| **Prism lifecycle** (§3) | intent → decision → plan → grant → execute → verify → receipt, journaled. |
| **Durable journal + replay** (§3, §3b.1) | 12/12 kill-test across every boundary; no repeated effects. |
| **Receipts law** (§3) | Claims compiled from evidence; model prose never becomes a claim; deterministic claim-vs-receipt check restored structurally. |
| **Transactional outbox** (§3) | UNIQUE dedupe key; enqueue-before-send. |
| **Mind: sourced facts** (§4.2) | `source_msg_id` is a foreign key. Zero unsourced facts *by constraint* — the M2 gate, still holding, now across sync too. |
| **Hybrid retrieval** (§4) | FTS5 + sqlite-vec + graph-ish + recency, RRF-fused. |
| **Crypto-shredding** (§4) | Per-cell keys; dropping the key drops the content. |
| **Media vault** (§4a) | Content-addressed, encrypted, `pinned` column present. |
| **Boundary Log** (§7a) | Hash-chained, append-only triggers, verified at boot, **now anchored off-site** so a rewrite is detectable. |
| **Hub as sole gate** (§6) | Every external call goes through it; boundary-logged both directions. |
| **The §6a cast** | All nine roles wired, including the evaluator seat. |
| **Reliability lessons** (§6a.1–5) | Hard timeouts, watchdog, tolerant JSON (with truncation repair), deterministic floor, eval-gated changes. |
| **Deterministic floor** (§6a.4) | p95 **1.5 ms** against a ≤300 ms budget — 200× inside target. |
| **Robot Package** (§8) | Sealed, one-time code, integrity-checked restore, identity preserved. |
| **Tier 3 slug** (§7b) | Capability URL, session cookie, stable across restarts. |
| **Built-in Chat** (§10b) | Streaming via SSE, voice upload, file drop. |
| **Language boundary** (§2d) | Rebuilt as tool-calling. Kernel holds English identifiers only; ten languages at 0 misroutes; content stored verbatim. |
| **Soul S1–S2** (§5.2–3) | Stance and dial, bounds, pinning, expression on both paths. |
| **Evaluator separation** (§5, law) | Own seat, no fallback to the generator, unavailable ≠ passed. |
| **Injection defence** (§12) | 23 cases × 3 trials, 0 leaks; tools structurally absent from the untrusted path. |
| **Two-way sync** (§9) | Beyond spec: convergence and no-resurrection, ours rather than cr-sqlite. |

**Four promises are kept structurally rather than by discipline**, which is
the part worth protecting: no fact without a source (FK), no reply without a
receipt (journal), no byte unlogged (single gate), no tool on the untrusted
path (no catalog in that prompt).

---

## 2. Gaps that matter, in order of consequence

### A. The Robot cannot yet *do* very much (§14: "four capabilities done excellently")

V1 names **memory, web research, calendar, email**, files fifth. Built:
memory and web research. **Calendar and email do not exist** — no OAuth, no
connectors, no `email.send` approval path.

This is the largest single gap between the document and the product. The
kernel that governs actions is finished; the actions are two of five.

### B. Grants are minted but never checked (§3)

`Grant` objects are created, journaled, given an `expires_at` — and nothing
reads them. Authority is de facto "the capability registry allowed it".

The architecture's sentence is *"narrow, time-boxed authority… never 'email
access'"*, and that is precisely the property that must exist **before**
calendar and email land, not after. Today the blast radius is small because
every capability is local; the moment a capability can send mail, an
unchecked grant is the difference between a scoped authority model and a
decorative one.

**This is the highest-leverage fix in the document.**

### C. Approvals do not exist (§3b.2, §14)

`Approval::Required` is defined and never emitted. Every plan step is
`Auto`. The confirmation gate built for Soul's deletions is a narrower,
bespoke mechanism — good, but not the general one.

`email.send` defaults to approval-always (Q28). Without durable interrupts,
that decision is unimplementable.

### D. The Registry is one-fifth of the headline feature (§4b)

§4b promises five categories: **knowledge · instructions · preferences ·
media · grants**, each item readable, correctable, confirmable, exportable,
erasable — and says *"nothing about you exists outside these five
categories, and that sentence is checkable because the categories are the
schema."*

Built: **knowledge only.** Media exists in a vault but is not in the
Registry; grants are journaled but not listed; instructions and preferences
have no store at all (procedural memory is absent entirely).

This is called "the headline feature" and "the positioning sentence". It is
currently a fact list.

### E. Mind is missing two of its six stores (§4)

- **Procedural memory** — *"above this amount, ask first"*, versioned,
  testable, reversible. Absent. This is also where §4b's "instructions"
  category would live, so D and E are one gap wearing two hats.
- **Working context** — short-lived task state, expired independently.
  Absent; context is assembled per turn and thrown away.

The commitment ledger exists **only as reminders**. §4.5 asks for what was
asked, what was promised, deadlines, waiting conditions, delegations, and
why each commitment closed — the Second Law's home. A reminders table is
the easy third of it.

### F. The Dashboard is 4 panels of 10 (§10a)

Built: overview, people, registry, boundary log. Missing: **Conversations ·
Commitments · Hub · Models & Routing · Soul · System.**

The Soul panel matters more than the count suggests: revision history with
diffs and one-click rollback is how §5's "restore last month's behaviour"
becomes real, and it is the visible half of the work just done.

### G. Trust is missing its classification and signing layers (§7)

- **Data classes** — every object carrying `public · owner-private ·
  sensitive · restricted · local-only · credential · derived ·
  org-confidential`. Absent. Nothing is classified, so nothing can be
  filtered on classification — and §6's "eligibility filtering (private
  data → cloud eliminated)" has nothing to filter on.
- **Signed receipts and signed releases** (§7). Receipts are compiled and
  stored, never signed. "This receipt came from my Robot" is not yet
  verifiable by anyone but the Robot.
- **Context packaging manifest** — the per-call disclosure record. The
  Boundary Log records that a call happened and its size; it does not
  record the *categories and originating objects* §7 specifies.

### H. Platform layer, entirely absent (§7b, §13e)

Tier 2 magic link · Tier 1 PIN/HSM · Connect to Labs · the relay ·
Labs-side Sync · signed update channels with staged rollout and
self-rollback · the Recovery Kit.

Fair, since none of it is buildable without the Control Plane — but §13e
says plainly *"for a self-hosted fleet, updates are the security model"*,
and there is currently no update path at all.

### I. Long-horizon execution: 2 of 6 (§3b)

Built: journal granularity, and (arguably) plan-as-artifact in a shallow
form. Missing: durable interrupts, replay & fork, context offloading,
sub-intents with narrowed grants. The document schedules 3 and 6 for "M4+",
so this is on time rather than late — but context offloading is what makes
the web READ loop scale, and that loop exists now.

### J. The Context Compiler barely exists (§6)

Called *"the most valuable proprietary system in the company"*. What exists
is a context assembler: persona + recalled facts + recent turns. Missing:
the provider-neutral intermediate representation, and — concretely
measurable — the **cache-aware stable-prefix layout**, which §2b claims cuts
input cost 30–70% and §2c claims cuts prefill latency.

That is a costed promise nobody has collected on.

### K. Nothing is measured against the numbers the document commits to

- **§2c speed budget** — TTFT ≤1 s p50, full turn ≤3 s p50 / ≤6 s p95. The
  floor is measured (1.5 ms). The model paths are not. Observed
  informally: a non-floor turn ran **9.2 s**, which is outside p95.
- **§2b cost model** — no per-turn cost accounting exists; §2b's own
  "TO-VERIFY" note is still open.
- **§2a scale envelope** — no load test. M5's gate (100K synthetic turns,
  48 h) has never run.
- **§12 metrics** — routing, injection, receipts and multilingual are
  covered. The Human, Personal and Private metric families are not
  measured at all, and **LongMemEval / LoCoMo have never been run**, which
  §12 says are published every release.

### L. Multi-owner readiness is partial (§11)

Principals and cells exist; roles are owner/member with an owner check on
two capabilities. Quotas exist for ultra spend only. Guest tier, suspend,
remove-with-data-separation, per-user model policy: absent.

---

## 3. Where the code has gone *beyond* the document

Worth recording, because these should be folded back into the spec rather
than left as undocumented behaviour:

1. **Tool-calling boundary.** §2d's language law is implemented through an
   English tool catalog with JSON schemas — a mechanism the document does
   not describe. It is strictly better than the phrase tables it replaced.
2. **Two-way sync between instances.** §9 mentions CRDT replication for
   premises; what exists is a working merge with tombstones, conflict rules
   and convergence tests. Ours, not cr-sqlite.
3. **Boundary-chain anchoring.** Not in the document. Answers the "unkeyed
   chain" risk without the circular HMAC.
4. **Action records.** The receipts law made visible in the surface.
5. **The confirmation gate for inferred destructive actions.** A narrower
   §3b.2 that arrived early because tool calling needed it.

---

## 4. What to do, in order

Sequenced by *what unblocks what*, not by size. Each has a gate.

### Now — close the authority model before adding reach

**1. Enforce grants.** (§3) Check scope and expiry at execution; refuse and
journal on violation. ~1 day.
*Gate: an expired or out-of-scope grant refuses the step, proven by test.*

**2. Durable interrupts.** (§3b.2) `Approval::Required` parks the intent as
`awaiting_approval`; approval from any surface resumes it, across restarts.
~2 days.
*Gate: park a step, restart the process, approve, watch it complete once.*

**3. Data classes.** (§7) Classification on every object; the field the
eligibility filter needs. ~1–2 days.
*Gate: a `restricted` object cannot enter an external call's context.*

These three are the prerequisites for anything that touches the world.
Doing them after email would mean retrofitting authority around a live
capability.

### Next — make the Robot useful (§14's four capabilities)

**4. Calendar.** (Q29: Google, native OAuth in Hub, PKCE, loopback
redirect.) ~4–5 days.
*Gate: create, update and delete an event with verified receipts; token in
the vault, never in model context.*

**5. Email.** Search/read/draft auto; **`send` behind approval-always** —
which is why #2 comes first. ~4–5 days.
*Gate: a send parks for approval and cannot execute without it.*

**6. Files.** Vault-scoped save/read/list. ~2 days.

### Then — the headline feature, completed (§4b)

**7. Procedural memory + instructions store.** (§4.6) Versioned, testable,
reversible. ~3 days.

**8. The Registry, all five categories.** Knowledge · instructions ·
preferences · media · grants, each with read/correct/confirm/export/erase.
~3–4 days.
*Gate: the claim "nothing about you exists outside these five categories"
is checkable against the schema.*

**9. Commitment ledger proper.** (§4.5) Beyond reminders: promises,
waiting conditions, why each closed. ~2–3 days.
*Gate: the Second Law is a screen — nothing asked for is silently dropped.*

### Then — prove the numbers the document sells

**10. Speed instrumentation.** (§2c) TTFT and turn latency into the eval
suite as gates, not observations. ~2 days.
*Gate: p50/p95 measured per class; a change that loses the budget fails.*

**11. Cache-stable context layout.** (§6, §2b) The 30–70% input-cost claim,
collected. ~2–3 days.
*Gate: measured cache-hit rate and cost delta, published in BUILD-LOG.*

**12. Per-turn cost accounting.** (§2b) Close the "TO-VERIFY".

**13. LongMemEval + LoCoMo.** (§12) The published-numbers promise. ~3 days.

### Then — the rest of the control room and the platform

**14. Dashboard panels 5–10**, Soul first (revision history, diffs,
rollback). ~4–5 days.
**15. Soul S3–S6**, once the owner has lived with S2 and set the numbers.
**16. Load test** to M5's gate. **17. Update channel** with signed releases
and self-rollback. **18. Recovery Kit.** **19. Tiers 2/1 and Connect to
Labs** — gated on the Control Plane existing.

---

## 5. The honest summary

The document describes a product whose *hard* half is governance —
provenance, receipts, grants, boundary accounting, portability — and whose
*visible* half is capability and control surface. The hard half is largely
built and tested. The visible half is a chat window, four capabilities of
which two exist, and four dashboard panels of ten.

**The single most important sequencing point:** grants, approvals and data
classes are cheap now and expensive later. Every one of them is a
cross-cutting property of the action path, and the action path is about to
grow calendar, email and files. Build the authority model while there are
two capabilities to retrofit, not five.

**The most under-collected promise:** the cache-stable context layout. It is
named the company's most valuable proprietary system, it has a costed
number attached, and it has never been built or measured.

**The most exposed claim:** §12's published memory benchmarks. The document
says "inspect our memory scores" is a sentence no closed competitor can
say. It is currently a sentence we cannot say either.
