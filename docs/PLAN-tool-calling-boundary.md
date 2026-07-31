# Plan — the tool-calling language boundary

Status: **implemented** 2026-07-31. Owner approved all three assumptions in
§9. Supersedes the language-pack approach shipped earlier the same day
(`6d77306`).

**Two deviations from this plan, both deliberate:**

1. **Phases 3 and 5 shipped together.** Writing pack-shaped code for one
   commit and deleting it in the next would have been churn; the
   replacement is proven by the same suite in the same change, which is
   what "deletion comes last" was protecting.
2. **The renderer lives in `robotd/src/render.rs`, not `surfaces/`.**
   `robotd` is the composition crate and already holds the gateway the
   non-English path needs. `surfaces` would have had to grow a dependency
   on `hub` to host it.

---

## 1. The principle

**The kernel speaks data, not prose.**

Everything entering the kernel is a validated structure: an action name from
a fixed list, plus typed arguments. Everything leaving the kernel is a
structure too: an event, its data, and a receipt. The identifiers in those
structures are English, because one language means one thing to read when
auditing what happened.

Sentences — in any language, including English — are a **surface** concern.
They are produced outside the kernel, at the last moment before delivery.

This is the shape OpenAI function calling, Anthropic tool use, and MCP all
converge on. We adopt it rather than invent.

## 2. What this replaces

Language packs answered three questions with hand-authored tables: which
command, which arguments, and what words to say back. The tables were the
industry-standard i18n pattern (gettext/ICU/CLDR lineage), and they are
correct for software with a fixed set of UI strings. They are the wrong
tool for a robot that takes open natural language, because every capability
added means an edit in every pack, and the drift is silent until someone
reads a sentence still in the wrong language.

Under this plan the count of supported languages appears **nowhere in the
code**.

## 3. The tool catalog

`Capability` gains two methods. The registry then *is* the catalog —
generated, never authored.

```rust
pub trait Capability: Send + Sync {
    fn name(&self) -> &'static str;              // exists
    fn effect(&self) -> Effect;                  // exists
    fn description(&self) -> &'static str;       // NEW
    fn schema(&self) -> serde_json::Value;       // NEW — JSON Schema
    fn execute(&self, ctx: &Ctx<'_>, args: &serde_json::Value)
        -> Result<Outcome, PrismError>;
}
```

`description` is what the phrase tables used to be. One English sentence
per capability is what lets a model map `напомни`, `hatırlat`,
`recuérdame` and `思い出させて` onto the same tool. It is the highest-leverage
prose in the system and deserves care.

### The fourteen tools

| tool | args | effect |
|---|---|---|
| `time.now` | — | Read |
| `robot.about` | — | Read |
| `robot.help` | — | Read |
| `reminder.create` | `fire_at` (RFC 3339), `about` (verbatim) | ReversibleWrite |
| `reminder.list` | — | Read |
| `reminder.cancel_last` | — | ReversibleWrite |
| `memory.remember` | `content` (verbatim) | ReversibleWrite |
| `memory.recall` | `query` (verbatim) | Read |
| `registry.list` | — | Read |
| `memory.forget` | `index` (integer ≥ 1) | **Irreversible** |
| `memory.correct` | `index`, `content` (verbatim) | ReversibleWrite |
| `member.invite` | — | ReversibleWrite |
| `telegram.bind_code` | — | ReversibleWrite |
| `web.research` | `query` (verbatim) | Read |

Example schema, showing the two argument classes:

```json
{
  "type": "object",
  "properties": {
    "fire_at": {
      "type": "string", "format": "date-time",
      "description": "When to fire, RFC 3339 with offset. Compute from the
                      current local time given in the system prompt."
    },
    "about": {
      "type": "string",
      "description": "What to remind about, copied VERBATIM from the user's
                      message in their own language. Never translate this."
    }
  },
  "required": ["fire_at", "about"],
  "additionalProperties": false
}
```

**Two argument classes, and the distinction is load-bearing:**

- **Structural** (`fire_at`, `index`, `tier`) — typed, language-free.
- **Content** (`about`, `content`, `query`) — the person's **own words,
  verbatim**. Law 5 requires that a stored fact be what they actually said;
  a translated fact points provenance at words they never wrote. What used
  to be a convention is now enforced by the schema description and checked
  in review.

## 4. One turn, end to end

```
  message (any language)
      │
      ├─► [1] English floor ──match──► plan ──► … ──► receipt      (2 ms, free, offline)
      │        no match
      ▼
  [2] Hub: route call  ─── message + catalog + now + tz ───►  model
      │                ◄── { verdict: {Q16…}, call: {tool,args}|null }
      ▼
  [3] validate against the registry  ──invalid──► honest refusal
      │
      ▼
  [4] plan → grant → execute → verify → receipt        (UNCHANGED)
      │
      ▼
  [5] render: structured result ──► sentence + action record
```

**[1] The English floor stays.** Deterministic, offline, free, wins
unconditionally (Q17). It is not a language feature — it is the fast path
for the kernel's own language, and the only path that works with no network.

**[2] One model call, not two.** The response carries the frozen Q16
verdict object *and* an optional tool call as siblings:

```json
{
  "verdict": { "action": "task", "domain": "reminder", "door": "followup",
               "tier": "fast", "lang": "ru", "mood": {...},
               "confidence": 0.9 },
  "call": { "tool": "reminder.create",
            "args": { "fire_at": "2026-07-31T18:30:00+01:00",
                      "about": "позвонить марку" } }
}
```

Q16 is untouched — it is still exactly that object. The envelope around it
gains a sibling. See §9.

**[3] Validation is the whole safety story.** The kernel checks: the tool
exists in the registry; the arguments validate against its schema; the
declared effect class matches what the registry declares; `fire_at` parses,
is in the future, and is inside the existing horizon guard. Any failure is
an honest refusal, never a guess. **The model is an input device, not an
authority.**

**[4] Nothing changes.** Plan, grant, execute, verify, receipt, outbox,
crash replay — the governed core does not care where the plan came from.
This is deliberate: the riskiest part of the system is the part this plan
does not touch.

**[5] Rendering** — see §5.

## 5. Output: the kernel emits structure

The kernel's product is:

```rust
pub struct TurnResult {
    pub receipt: Receipt,
    pub actions: Vec<ActionRecord>,   // from the receipt's evidence
    pub utterance: Option<String>,    // model prose, when a model spoke
}
```

The renderer lives at the **surface**, not in the kernel:

- **English** → deterministic templates, in code, at the surface. Free,
  instant, exact. English sentences are still sentences, so they live
  outside the kernel like every other sentence.
- **Any other language** → one model call: here is what happened, say it in
  `<lang>`. Cached is a later optimisation, not part of this plan.
- **Always** → the **action record**, rendered from the receipt.

### Law 1 under tool calling

The standard tool-calling loop ends with the model narrating the result.
That makes a model the author of sentences like "I've set your reminder" —
exactly what the receipts law forbids, and what the deleted phrase lists
were defending against.

The structural answer, which is also the industry's: **show the tool calls.**
Prose from the model sits beside a kernel-rendered record of what actually
ran:

```
  ⏱  reminder.create  ✓ verified   fires 18:30 · "позвонить марку"
```

If the prose claims an effect and no action record appears beside it, the
lie is visible — with no text analysis, in any language. The receipt stays
the record of truth; the surface stops hiding it.

This replaces `effect_claims` entirely and is strictly stronger: the phrase
scan only ever caught wordings someone had anticipated.

## 6. Two safety rules that are not optional

**6a. Tool calls come only from the routing call.** The routing call sees
the person's message and nothing else. The research call — which sees
fetched web pages, i.e. untrusted material — is given **no tools at all**.
Tool calling raises the stakes of prompt injection from "the model says
something wrong" to "the model *acts*", and the only robust answer is that
the code path which touches untrusted data has no ability to act. The M6
injection suite gets a new class of case: a page that tries to induce a
tool call must not be able to, because there is no tool to call.

**6b. Model-proposed irreversible effects require confirmation.**
`memory.forget` from the deterministic English floor is an explicit
instruction and executes immediately. The same tool proposed by a model is
an inference, and inferences should not delete things. Such steps are
planned with `Approval::Required`, which parks the intent in the journal
and resumes on confirmation — a mechanism the architecture already
specifies (§ durable interrupts) and that nothing currently uses.

## 7. What changes, file by file

**Added**

- `hub/src/tools.rs` — catalog serialisation, the route-call request and its
  strict response type.
- `robotd/src/caps/*.rs` — `description()` + `schema()` on all fourteen.
- `prism` — `CapabilityRouter` gains `describe()` and `validate(tool, args)`
  so the kernel can validate without depending on `robotd`.
- `prism/src/lifecycle.rs` — `Decision::Call { tool, args }`; plan built
  from a validated call.
- `surfaces/src/render.rs` — English templates, model rendering, action
  records.

**Deleted**

- `prism/src/lang/en.toml`, `ru.toml`, `README.md`
- `prism/src/lexicon.rs`
- the table-driven multi-language machinery in `floor.rs` (English tables
  return to code)
- `effect_claims` and the lexical claim scan
- `[signals]` / `matches_signal` in `hub/src/escalation.rs` (English
  literals return; non-English tier comes from the verdict, which already
  carries it)
- the pack-maintenance tests

Net: roughly a thousand lines out, a few hundred in.

**Kept and strengthened**

- `no_surface_vocabulary_lives_in_code` — under this design it should catch
  *any* user-facing prose in the kernel, not only non-Latin script.
- `cell_meta.lang` (BCP 47) — the renderer needs it for lanes that speak
  without being asked: a reminder firing at 03:00, a backup failure.

## 8. Tests and the gate

**Catalog**
- every registered capability has a non-empty description and a schema that
  parses as valid JSON Schema
- every schema's `required` fields are exactly the args `execute` reads
  (asserted per capability with a sample call)

**Validation — the safety surface**
- unknown tool → refused
- wrong argument type, missing required, extra property → refused
- effect class disagreeing with the registry → refused
- `fire_at` in the past, or beyond the horizon → refused
- a refusal is a terminal receipt with an honest reply, never a 500

**Laws**
- law 1: the action record is rendered from the receipt, always; a turn with
  no effects renders no action record
- law 4: no user-facing prose in the kernel, in any language
- law 5: a `content` argument that differs from a verbatim span of the
  user's message is a review finding (checked live, see below)

**Live** (`robotd eval --live`)
- **multilingual routing corpus**: 10 languages × 6 intents = 60 cases, bar
  0 misroutes, asserting both the tool chosen and that content arguments
  came back verbatim
- **injection**: existing 20 cases × 3 trials, plus new cases where fetched
  content attempts to induce a tool call
- offline: these skip, and the English floor suite runs as today

**Gate:** `cargo test --workspace` green, `cargo clippy --workspace
--all-targets -- -D warnings` clean, `robotd eval --live` PASS, BUILD-LOG
entry, board updated, one commit per phase.

## 9. Assumptions needing owner sign-off

1. **The Q16 envelope gains a sibling field.** The verdict object itself is
   unchanged and still validates against the frozen schema; the response
   that carries it gains `call`. This is an extension around a frozen
   decision, not a reopening of it — but it touches frozen text, so it is
   yours to approve.
2. **Non-English loses the offline path.** With no network, English works
   fully and other languages do not work at all. Today the Russian pack
   works offline. This is the real price of the design and it is accepted
   deliberately.
3. **Confirmation on model-proposed irreversible actions** changes
   behaviour: "forget fact 2" in Russian will ask before deleting, where
   English via the floor still deletes immediately.

## 10. Sequencing

Each phase ends green, and the deletion comes **last** — the replacement
must work before the old thing goes, which is the lesson from the review.

| phase | what | reversible? |
|---|---|---|
| 1 | tool catalog: `description()` + `schema()` on all fourteen; catalog tests. No behaviour change. | yes |
| 2 | route call returns verdict + optional call; kernel validates and plans from it. Non-English gains every capability. Live routing corpus. | yes |
| 3 | rendering split: kernel emits `TurnResult`; English templates and model rendering at the surface; action records in chat. | yes |
| 4 | injection cases for tool induction; approval gate on model-proposed irreversible effects. | yes |
| 5 | **delete** packs, lexicon, phrase lists; floor returns to English-only tables in code. | the point of no return |
| 6 | full gate, BUILD-LOG, board, commits. | — |

Estimate: phases 1–2 are the substance, 3–4 are careful, 5 is an afternoon.
Call it two working days with the gate run properly at each step.

## 11. Against the four properties

- **Light** — no per-language artifacts; the tool catalog is generated from
  code that already exists.
- **Robust** — every model output is schema-validated before it can do
  anything; untrusted content has no tools; irreversible inferences ask.
- **Transferable** — one binary, identical everywhere, no locale data to
  carry.
- **Expendable** — a capability is a self-describing unit. Removing one
  removes its tool, its schema and its language coverage in a single
  deletion; nothing else needs to know.
