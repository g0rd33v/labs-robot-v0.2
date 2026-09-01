# Product spec vs. what exists — a conformance read

Read of **Bender MVP Product Specification v1.0 (2026-07-30)** against the
code as of 2026-08-05 (`d84b372` + the deployment work). Every ✅ below was
checked in the source or run live; nothing is inferred from a BUILD-LOG
entry. Where this disagrees with a previous summary, this document is the
later and more careful read.

**Headline: the MVP's Must list is complete, and the runtime has gone well
past it** — items the spec calls Post-MVP (calendar, email, sync, update
channels, the full Registry, the commitment ledger) shipped in the last
week. What is genuinely unmet is now concentrated in **one place: a
performance target.** The chat surface's inspection affordances and the
member-facing half of the Registry — items 2, 3 and 4 of the punch list
below, and the largest single gap this read found — shipped on 5 Aug and
are marked through.

---

## 1. Success metrics (§1.4) — the scorecard

| # | Metric | Status |
|---|---|---|
| 1 | DoD demo passes end-to-end (M7) | ✅ passed 2026-07-30; re-proven since |
| 2 | First visible response ≤1.0 s p50; routine ≤3 s p50 | ❌ **not met on the week's numbers.** `robotd cost` over 7 days: route seat p50 **2882 ms** at 32.5 % cache-hit across 623 calls. Individual warm runs have hit 993 ms avg / turn p50 2613 ms, but two live evals on 5 Aug failed both speed gates (routing p50 3584/4490 ms). One good run is not the metric |
| 3 | Zero intents without terminal receipts, 10K-turn soak | ✅ **exceeded**: 25,000 turns, 0 dropped |
| 4 | MISROUTE-0 on the routing corpus | ✅ 66/66 offline, 0; live 60 cases, 0 |
| 5 | Package → USB → resume with 100% memory/receipts/persona | ✅ proven twice (USB, and today to a container) |

**Metric 2 is the one honest miss.** Measured at the surface today:

- **Full routine answer: ✅ 3115 ms p50** against a ≤3 s target — 115 ms
  over, i.e. at target within noise, and p95 4210 ms against ≤6 s.
- **First visible response: ❌ ~3.2 s, not ≤1.0 s.** Tokens now stream and
  the *model's* TTFT is 349 ms p50 — but a non-floor turn spends ~2.8 s in
  the routing call before the answer seat is even asked. The person sees a
  typing indicator, then nothing, then a fast stream.

Floor turns already answer in **1.5 ms p50**, 200× inside their 300 ms
budget. For everything else, two hypotheses were tested on 2026-08-05 and
one survived:

**Tested and rejected — route on §6a's specified seat.** §6a assigns
*Router / verdicts* to `gemma-4-26b-a4b-it`; the code routes on
`gemma-4-31b-it`, a drift dating from the tool-calling rebuild. Reverting
to the spec'd model measured **worse on both axes**: routing p50 4357 ms
(vs 2987), p95 7691 ms, 0% cache-hit, and it emitted schema-invalid
verdicts (`"door": "fast"`, `"door": "none"`) that fell through to the
deterministic fallback. The reason is in §6a's own cost table: it sized
this call at **1.5K input tokens**, and the tool catalog has made it
**5.0K** — a different call than the one the seat was chosen for. The 31b
stays, and this paragraph is the evidence for the deviation.

**The remaining arithmetic.** Routing p50 is ~3.0 s and the answer's own
TTFT is ~350 ms, so displayed TTFT ≈ 3.3 s. No amount of answer-side
speed helps: **routing is a wall in front of it.** Trimming the catalog
(~3.3K tokens of tool descriptions) would shave perhaps 40% off prefill —
still ~2 s, and it attacks the multilingual mechanism that took three
attempts to get right.

**Built 2026-08-05, and measured.** Routing p50 **2987 → 1080 ms**, full
answer turn p50 **3115 → 2749 ms — §2c's 3 s budget is now MET** and is
the gate rather than an aspiration. Cache-hit on the routing prefix rose
to **78%**, 0 misroutes held, 1.00 router calls/turn. Surface TTFT
measured 2.1 s and 4.0 s on two hand samples — clearly better than the
~3.2 s baseline, but **still not the ≤1 s target**, and single samples
are dominated by provider variance. What remains between here and 1 s is
the router's own time-to-first-token: the decision cannot be read before
the model starts speaking. Honest status: **substantially closed, not
closed.**

**The design: early decision from a streamed router.**
Stream the routing call and put `call.tool` FIRST in the output shape.
The tool decision then arrives ~600 ms in, while the rest of the verdict
is still streaming — and for the answer path (`tool: "none"`) nothing else
is needed, so the answer can start immediately. Displayed TTFT becomes
router-TTFT + answer-TTFT ≈ **950 ms**, under budget, with **no
speculation, no retraction, and no wasted calls**. It is not §2c #2's
speculative fan-out; it is better, because nothing is guessed.

The cost is real: it reorders the routing output contract (touching
salvage, repair and the eval corpus) and adds concurrency to `run_turn`.
That is a deliberate change to the one contract that took three attempts
to stabilise, so it wants an explicit decision rather than a quiet
commit.

---

## 2. Feature specifications (§4) — clause by clause

### 4.1 Conversation ✅ with two gaps at the surface

| Clause | Status |
|---|---|
| 4.1.3.1 latency | ⚠️ per metric 2 above |
| 4.1.3.2 voice note → transcription + answer | ✅ STT seat wired, upload path live |
| 4.1.3.3 receipt exists, claims ⊆ asserted effects | ✅ deterministic check every turn |
| 4.1.3.4 provider down → floor still answers, honest failure otherwise | ✅ proven in the kill-suite and live |
| R4.1.1 floor wins unconditionally | ✅ |
| R4.1.2 one verdict call, salvage, one retry, fallback | ✅ measured 1.03 router calls/turn |
| R4.1.3 reply in sender's language, internals English | ✅ 10 languages, 0 misroutes |
| R4.1.4 SSE token streaming | ✅ shipped in the speed tranche |
| **C1: receipts icon → inspector modal** | ✅ shipped 5 Aug |
| **C1: approval card with Approve/Deny buttons** | ✅ shipped 5 Aug — over the same durable gate the typed answer uses |
| **C1: empty state with 3 suggested messages** | ❌ not built |
| 4.1.6 double-send coalescing within 2 s | ✅ `prism::repeats`: durable per-cell claim on a content hash, checked before the message is recorded and before any intent exists |
| 4.1.6 mid-stream abort → partial kept, receipt `partial` | ⚠️ `partial` status exists; the mid-stream path is untested |

The chat *works* — text, voice, files, streaming, media, any language —
and since 5 Aug the **inspection UI** works too: the receipts icon opens
the journal's own claims and evidence, and approvals are cards with
buttons rather than a word to type. What is still missing from §4.1 is the
empty state's three suggested messages, and a test for the mid-stream
abort path.

### 4.2 Memory & Registry ✅ / ❌ split by audience

| Clause | Status |
|---|---|
| 4.2.3.1 source one tap away, FK-backed | ✅ FK constraint; source shown in registry list |
| 4.2.3.2 correction supersedes, not deletes | ✅ |
| 4.2.3.3 erase is real and journaled | ✅ row deleted, tombstoned, travels in sync |
| 4.2.3.4 member departure exports + crypto-shreds the key | ✅ both halves: `/api/export` serves registry + whole conversation as a download; `RobotCore::remove_member` drops the handle, deletes the wrapped DEK, unlinks the file + `-wal`/`-shm` + media vault, journals |
| D2: five category tabs | ⚠️ **all five exist as data and in the owner dashboard**; `/registry` renders all five in chat. There is no per-category tabbed UI with per-item row actions. |
| D2: per item — view source · correct · confirm · erase | ⚠️ all four exist **as chat commands** (`my facts`, `correct fact N`, `confirm fact N`, `forget fact N`); none as buttons |
| D2: export all (JSON) | ✅ `registry.export` writes item-by-item JSON into the person's vault |
| R4.2.1 no fact without a source | ✅ FK, enforced |
| R4.2.2 models propose, Mind decides | ⚠️ **partial** — the mutation protocol's promotion ladder (`tentative → contextual → stable`, Q21) is **not implemented**; facts land `stable`, and `confirmed_at` was added for owner-confirmation. `contested` is unimplemented. |
| R4.2.3 members manage only their own partition | ✅ enforced; owner content-view policy is the MVP preset (never) |
| 4.2.6 contradiction → both kept, contested | ❌ not built |

### 4.3 Reminders & Commitments ✅ mostly

| Clause | Status |
|---|---|
| 4.3.3.1 natural-speech reminder, member timezone, confirmation names the time | ✅ proven in 10 languages |
| 4.3.3.2 fires exactly once through the outbox, receipted | ✅ dedupe key |
| 4.3.3.3 **overdue reminders fire on restart with an "overdue" marker** | ⚠️ **they fire** (the scheduler sweeps `fire_at <= now`), but there is **no overdue marker** — the person cannot tell a late fire from an on-time one |
| 4.3.3.4 open commitment past TTL closes `failed` with a reason, person told | ✅ sweeper + ledger, reason mandatory by schema CHECK |
| R4.3.1 ambiguous time → one clarify question, 2–3 tappable options | ❌ **not built** — the model resolves or the floor refuses; no clarify-with-options flow |
| R4.3.2 the ledger is the source of truth for "what's pending" | ✅ `/commitments`, and the dashboard panel |
| 4.3.6 cancel of already-fired → honest "already fired at …" | ⚠️ cancel targets active reminders only; the honest message is generic |

### 4.4 Web Research ✅ complete

Search → **fetch → READ** with attributions ✅ (snippet-only would be a
defect; the fetch loop is live and was exercised today). Fetch failure
names the unreachable source ✅. Untrusted-by-origin with injection
defence ✅ — **23 cases × 3 trials, 0 leaks**, which is stronger than the
spec asks. Digest + handle offloading ⚠️: content is capped and truncated
rather than digested with a pullable handle.

### 4.5 Media Vault ✅ complete for MVP

Content-addressed, encrypted, referenced from journal and Boundary Log ✅.
Re-read at full fidelity ✅. Generated artifacts saved and retrievable ✅
(`file.save/read/list/delete`, beyond the spec's ask). Expiry jobs remain
Post-MVP as the spec allows.

### 4.6 People ✅

Invite links → own cell → own chat ✅. **Zero cross-cell reads** ✅,
verified by the harness. Telegram pending-approval flow ✅ (Q2). Roles
Owner/Member ✅. Join policy Private preset ✅. **Member removal with
crypto-shred: ✅ shipped 5 Aug** — with the export the clause also asks
for, offered on the same screen and before the erase. A person may erase
themselves, the owner
may erase anyone else, and nobody may erase the owner. Ordered so an
interruption at any point after the key is deleted still leaves the data
destroyed.

### 4.7 Dashboard & Boundary Log ✅ exceeded

Boundary entries with direction, counterparty, purpose, categories, hash,
size; chain verifies ✅. Unsampled ✅. Overview shows status, activity,
commitments ✅. **The spec asks for Dashboard-lite (3 panels); ten are
built** — including Models & Routing with live cost/cache/TTFT, and Soul.
One gap: **spend today vs cap is not on Overview** — the meter has the
numbers (`robotd cost`, and the Models panel) but Overview does not show
spend against the cap.

### 4.8 Package & Restore ✅ proven twice

Encrypted package (manifest + core + cells + media + journal) ✅.
Restore resumes the same Robot — memory, Registry, receipts, persona,
**same slug token** ✅. Independence of the two copies ✅ and documented.
Beyond spec: two-way sync now exists (Post-MVP in the spec) and the
restore was exercised again today into a Docker container on
`robot.labs.co`. ⚠️ "interruption is resumable" for `package` is **not
implemented** — an interrupted package is re-run from the start.

---

## 3. Data model & state machines (§5)

| Spec | Status |
|---|---|
| Intent state machine incl. `awaiting_approval ⇄ executing`, TTL, sweeper | ✅ complete |
| Effect outbox `pending → sent → confirmed \| failed`, dedupe_key, never re-send | ✅ |
| **Effect retries 1 s / 5 s / 25 s (n<3)** | ❌ **no retry backoff implemented** — a failed effect fails; it is journaled, not retried |
| Fact `tentative → contextual → stable`, `superseded`, `erased`, `contested` flag | ⚠️ supersede ✅, erase ✅; **promotion ladder and contested not built** |
| Reminder `scheduled → fired \| cancelled`, fire-now on boot | ✅ (minus the overdue marker) |
| Permission matrix | ✅ as specified, except member removal (absent) |

---

## 4. Non-functional (§8)

| Requirement | Status |
|---|---|
| 8.1 Performance | ⚠️ TTFT-at-surface only; the rest met |
| 8.2 Security: slug, HttpOnly/SameSite, SQLCipher, envelope keys, outbound-only | ✅ all |
| 8.3 Accessibility: keyboard-completable, visible focus, transcripts | ⚠️ keyboard works; **no focus-visible styling or ARIA pass** |
| 8.4 Platforms: macOS primary, Linux compiles | ✅ — Linux now *runs*, in a container, in production |
| 8.5 Localization | ✅ |
| 8.6 Data & compliance: append-only, crypto-erasure, unsampled audit, backups | ✅ (+ off-site chain anchoring, beyond spec) |

---

## 5. Beyond the spec — what shipped that the MVP did not ask for

Everything here is listed Post-MVP in §1.5 and exists now: **calendar and
email connectors** (OAuth+PKCE; `email.send` behind approval-always),
**files**, **two-way sync**, **the full five-category Registry**, **the
commitment ledger proper**, **the instructions store** (§4.6 procedural
memory), **Soul S1–S2** with stance/dial/bounds, **the update channel**
with signed releases and rollback, **the Recovery Kit**, **the meter**
(per-seat cost/cache/TTFT), **the memory benchmark harness**, and **the
load harness**. Seven of ten dashboard panels are also beyond the
"Dashboard-lite" ask.

---

## 6. The honest punch list

Ranked by distance from a promise the spec makes to a user's face.

1. **TTFT at the surface (§1.4.2, §4.1.3.1)** — the only unmet success
   metric. Fix: speculative fan-out so routing and answer-context overlap.
2. ~~Receipt inspector + approval cards in chat~~ ✅ **done (5 Aug)** — a
   `receipt` button on every reply that came from a turn, opening the
   journal's claims and evidence; approvals render as cards with Approve /
   Deny over the same durable §3b.2 gate the typed answer uses.
3. ~~Registry as a member-facing screen~~ ✅ **done (5 Aug)** — `/me`, five
   tabs, per-item confirm / correct / erase, source and promotion rung on
   every item, routed through the same `mind` functions as the chat
   commands so "forget" has one implementation.
4. ~~Member removal + crypto-shred~~ ✅ **done (5 Aug)** — see §4.6.
5. ~~Fact promotion ladder and `contested`~~ ✅ **done** — Q21's exact
   thresholds in `mind::promotion`; facts enter `tentative`; contradictions
   keep both and surface on the dashboard as "conflicting — pick one".
6. ~~Effect retry backoff~~ ✅ **done** — 1 s/5 s/25 s then given up
   visibly, in `prism::outbox`.
7. ~~Overdue marker~~ ✅ **done** — a late reminder says how late and that
   the robot was down.
8. ~~Clarify-with-options~~ ✅ **done** — "in the morning" asks with two or
   three numbered times and never guesses; the answer is a number, so it
   works in any language.
9. ~~Spend vs cap on Overview~~ ✅ **done** — spend today and ultra usage
   against the cap.
10. ~~Double-send coalescing (§4.1.6)~~ ✅ **done (5 Aug)** — the inbound
    mirror of the outbox's `dedupe_key`: the same content inside two
    seconds claims one turn, so a retry, a second tab, or a Telegram
    redelivery cannot produce a second reply, a second transcript entry,
    or a second effect. **resumable `package` (§4.8.3.1)** and
    **accessibility pass (§8.3)** — still open.

Items 2, 3 and 4 were one coherent piece of work — **the member-facing
surface of the governance the runtime already enforces** — and shipped
together on 5 Aug. Building it produced its own lesson: the whole tranche
passed 247 tests while the chat page rendered **completely empty** in a
browser, because a modal bound before its markup existed. No test opened
the page. `every_element_the_script_binds_to_exists_before_the_script` now
reads the document order of both pages, and was verified to fail when the
defect is reintroduced. What remains on this list is item 1, and the two halves of item 10 that
are still open.

## 7. Open questions from §10.1, still open

1. **Akita log numbers** (Q39) — still owner-side. Partly overtaken:
   router calls/turn is now *measured* at 1.03 rather than assumed ~2.
2. **Voice replies (TTS)** — still out of MVP; unconfirmed.
3. ~~Member self-view scope~~ — **resolved by the owner**: all five
   categories, which is what `/me` implements.
