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

> **Closed, 2026-07-31.** All five now exist. Files and email's approval
> gate are demonstrated end to end; calendar and email's *live* Google calls
> await an OAuth client id for this instance — see §4 items 4–6.

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

**4. Calendar.** ✅ **built, gate not yet demonstrated.** (Q29: Google,
native OAuth in Hub, PKCE, loopback redirect.) `calendar.list/create/cancel`
against the Calendar API; OAuth 2.0 + PKCE in `hub::oauth` with the RFC 7636
vector as a test; tokens in the cell as `mind::connections`, leaving it only
as a `Secret` with no `Display` and no `Serialize`, absent from
`merge::export` so they never reach a stick.
*Gate: create, update and delete an event with verified receipts; token in
the vault, never in model context.* — **blocked on a Google OAuth client
id**, which nobody has issued for this instance. Everything that does not
need one is tested: PKCE, the authorization URL, the token exchange form,
single-use `state`, refresh-before-expiry, the event body for timed and
all-day events, and the parse back. Set `GOOGLE_OAUTH_CLIENT_ID` (and the
secret) and the gate is one `/connect` away.

**5. Email.** ✅ **built, and its gate holds.** Search/read/draft
automatic; `email.send` declares `Approval::Required` beside the code that
sends.
*Gate: a send parks for approval and cannot execute without it.* — **met.**
`robotd/tests/approval.rs` counts executions through the real registry: a
send reaches execution zero times before approval, zero after a decline,
zero after a crash replay, and exactly once after a yes. Demonstrated live
in Russian, including across a restart.

**6. Files.** ✅ **done.** `file.save/list/read/delete`, vault-scoped, with
`mind::files` as a name→content index so two names cost one copy. Names are
stripped of separators rather than rejected. Files sync between instances,
conflicting edits both survive, and deletions travel as tombstones.

### Then — the headline feature, completed (§4b)

**7. Procedural memory + instructions store.** ✅ **done.** (§4.6)
`mind::instructions` + `instruction.add/list/revise/retire`, injected
verbatim into both model paths (routing and answering) as one fenced,
class-filtered block. *Versioned*: revision is supersession, never
overwrite. *Reversible*: retire has an undo, and history stays. *Testable*:
a rule is words the models read, nowhere else — which sentences are in
force is one query, and the injection contract is a unit test.
Demonstrated live: "always answer in exactly one sentence" bound the very
next answer, overriding the mentor stance's follow-up habit. Rules sync;
retired beats active across instances.

**8. The Registry, all five categories.** ✅ **done.** `registry.show`
(and `/registry`) renders the five categories with counts;
`registry.export` writes the item-by-item JSON into the person's own vault
— the export right without a boundary crossing; `memory.confirm` completes
the §4 mutation protocol's last rung (owner-confirmed, with a timestamp).
Correct/erase existed per category.
*Gate: met.* `caps::registry_pims::census` maps every table a cell can
contain to a category or to named substrate; the test walks
`sqlite_master` and fails on any table it has never heard of — adding a
store without answering "which category?" is now a failing build.

**9. Commitment ledger proper.** ✅ **done.** (§4.5) `mind::commitments`:
closing **requires** a reason (schema CHECK, like the source FK on facts);
openings are hooks, not conventions — reminder creation, approval parking,
and the zombie sweeper each write their own entries; ids derive from the
backing thing so two instances converge on one ledger. `commitment.list`
(and `/commitments`) is the screen, rendered verbatim, never re-voiced.
*Gate: met.* Proven by counting: an ask enters the ledger the moment it is
deferred, and every ending — approved, declined, cancelled, fired, swept —
closes it with words a person can read. 'promise' is schema-ready and
waits for the background lanes that could make promises.

### Then — prove the numbers the document sells

**10. Speed instrumentation.** ✅ **done, with a recorded deviation.** Per
class in `eval --live`: routing p50/p95 over the 60-case corpus, full
answer turns end-to-end, floor ≤300ms (offline, long green). First
measurement: routing p50 ~3.2–3.9s / p95 ~10.7s, full turn p50 ~4.1–7.0s —
**the §2c budget (3s p50 / 6s p95) was lost that morning** — and mostly
recovered the same day by the speed tranche (see BUILD-LOG): provider
routing, streaming, async verify. After: routing p50 2817 / p95 4844
(budget met), turn p95 4210 (met), turn p50 3115 (115ms over — the
remaining move is speculative verdict/answer overlap), TTFT p50 349ms
(measured at last; budget ≤1s met). The routing p95 gate IS the budget
now.

**11. Cache-stable context layout.** ✅ **built and measured.** Two
cache-killers removed: the routing prompt carried the per-call timestamp
*before* the catalog (everything after the first changed token re-prefills
at full price — the catalog never cached), and the answer path put
per-query recalled facts in the system message, invalidating the whole
conversation every turn. Now: stable prefix → semi-stable (standing rules)
→ volatile (timestamp, recall) dead last.
*Gate: measured and published.* First eval-run figure: **cache-hit ~27% of
input tokens** — under §6's 30–70% claim, expected for an eval of
independent one-shot contexts; session traffic with append-only history is
the favourable case. Numbers in BUILD-LOG; `robotd cost` tracks it per seat
continuously.

**12. Per-turn cost accounting.** ✅ **done.** The meter: every model call
records tokens, cache hits, latency, and **the provider's own cost figure**
(no local price table — prices drift, and a stale table is an estimate in a
measurement's clothes) into `model_calls` in core.db. `robotd cost [--days N]`
reports per seat; `eval --live` prints calls/turn and run cost. First
measurements: **1.10 calls/turn** (§2b's TO-VERIFY assumed ~2 router calls
— Bender's one-call design is confirmed by measurement) and a full live
eval run — 75 calls, 36K input tokens — cost **$0.0043**.

**13. LongMemEval + LoCoMo.** ✅ **harness done; full-dataset numbers
pending.** Two parts. First the prerequisite: recall searched *facts only*,
so a question about an earlier conversation found nothing — §4.3's
"semantic index over everything" now covers messages (FTS + dated snippets
into answer context, with a backfill for older cells). Then the harness:
`robotd eval --memory <file>` speaks LongMemEval's own format — sessions
ingested as real messages, questions answered by the real recall+answer
path, graded by the evaluator seat (never the generator, Q26). Bundled
smoke set: **10/10** across single-hop, multi-session, temporal, update and
abstention. The smoke set proves the harness, it is not the benchmark: the
published-numbers promise closes when the LongMemEval-S / LoCoMo datasets
are downloaded (owner-side: ~1GB, HF/GitHub) and a full run is recorded
here and in BUILD-LOG.

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
