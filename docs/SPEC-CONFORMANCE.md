# Product spec vs. what exists — a conformance read

Read of **Bender MVP Product Specification v1.0 (2026-07-30)** against the
code as of 2026-08-05 (`d84b372` + the deployment work). Every ✅ below was
checked in the source or run live; nothing is inferred from a BUILD-LOG
entry. Where this disagrees with a previous summary, this document is the
later and more careful read.

**Headline: the MVP's Must list is complete, and the runtime has gone well
past it** — items the spec calls Post-MVP (calendar, email, sync, update
channels, the full Registry, the commitment ledger) shipped in the last
week. What is genuinely unmet is concentrated in three places: **one
performance target, the chat surface's inspection affordances, and the
member-facing half of the Registry.**

---

## 1. Success metrics (§1.4) — the scorecard

| # | Metric | Status |
|---|---|---|
| 1 | DoD demo passes end-to-end (M7) | ✅ passed 2026-07-30; re-proven since |
| 2 | First visible response ≤1.0 s p50; routine ≤3 s p50 | ⚠️ **partly met — see below** |
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

The fix is named and unbuilt: §2c #2's speculative fan-out — start the
answer-context build (and, for the predictable classes, the answer itself)
concurrently with routing, so the critical path is max() not sum(). Floor
turns already answer in **1.5 ms p50**, 200× inside their 300 ms budget.

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
| **C1: receipts icon → inspector modal** | ❌ **not built** |
| **C1: approval card with Approve/Deny buttons** | ❌ not built — approvals answered by typing "yes"/"no" |
| **C1: empty state with 3 suggested messages** | ❌ not built |
| 4.1.6 double-send coalescing within 2 s | ❌ not built |
| 4.1.6 mid-stream abort → partial kept, receipt `partial` | ⚠️ `partial` status exists; the mid-stream path is untested |

The chat *works* — text, voice, files, streaming, media, any language. What
is missing is the **inspection UI**: the spec's receipts icon and approval
cards are how a non-technical member sees the receipts law working. Today
that evidence lives in the dashboard (owner-only) and in the `— ✓ tool`
action records under each reply.

### 4.2 Memory & Registry ✅ / ❌ split by audience

| Clause | Status |
|---|---|
| 4.2.3.1 source one tap away, FK-backed | ✅ FK constraint; source shown in registry list |
| 4.2.3.2 correction supersedes, not deletes | ✅ |
| 4.2.3.3 erase is real and journaled | ✅ row deleted, tombstoned, travels in sync |
| 4.2.3.4 member departure exports + crypto-shreds the key | ❌ **not built** — no member-removal command exists |
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

### 4.6 People ✅ except departure

Invite links → own cell → own chat ✅. **Zero cross-cell reads** ✅,
verified by the harness. Telegram pending-approval flow ✅ (Q2). Roles
Owner/Member ✅. Join policy Private preset ✅. **Member removal with
crypto-shred: ❌ not built** — the mechanism exists (per-cell keys) but no
command invokes it.

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
2. **Receipt inspector + approval cards in chat (§4.1.4)** — the receipts
   law is real but invisible to a member; this is the feature that makes
   "evidence-grade" legible to someone who never opens a dashboard.
3. **Registry as a member-facing screen (§4.2.4)** — five tabs, per-item
   buttons, and a member self-view. Today every capability exists as a
   chat command and an owner-only panel.
4. **Member removal + crypto-shred (§4.2.3.4, §5.3)** — a promised right
   with no way to exercise it.
5. **Fact promotion ladder and `contested` (§5.2, R4.2.2)** — the mutation
   protocol is half-built; contradictions are not surfaced.
6. **Effect retry backoff (§5.2)** — 1 s/5 s/25 s, absent.
7. **Overdue marker on late reminders (§4.3.3.3)**.
8. **Clarify-with-options for ambiguous times (R4.3.1)**.
9. **Spend vs cap on Overview (§4.7.3.3)**.
10. **Double-send coalescing (§4.1.6)**; **resumable `package`
    (§4.8.3.1)**; **accessibility pass (§8.3)**.

Items 2, 3 and 4 are one coherent piece of work: **the member-facing
surface of the governance the runtime already enforces.** That is the
largest single gap between this specification and this product — and it is
UI work over mechanisms that are already built and tested, not new
machinery.

## 7. Open questions from §10.1, still open

1. **Akita log numbers** (Q39) — still owner-side. Partly overtaken:
   router calls/turn is now *measured* at 1.03 rather than assumed ~2.
2. **Voice replies (TTS)** — still out of MVP; unconfirmed.
3. **Member self-view scope** — unresolved, and now blocking punch-list
   item 3.
