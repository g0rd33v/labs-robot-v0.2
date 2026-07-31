# Soul — design and build plan

Status: **proposed.** Nothing here is implemented yet. Replaces the
reference to a "SOUL-MASTER" document that was named in architecture §5 and
never existed — the owner confirmed no such document was ever written.

This is the design §5 pointed at. §5 says *where Soul plugs in*; this says
*what it does inside*, and in what order to build it.

**On "AEI".** Architecture §5 names it once, expands it nowhere, and no
document defining it exists. It came from the same phantom reference. I
have not invented a meaning — see §12.

---

## 1. What Soul is, and where its output lands

**Soul is the robot's emotional intelligence — the part that makes the
rest of it feel like talking to someone.** It learns how a particular
person behaves and how they like to be met; it speaks in a human register
rather than a system one; it can take on a role or a character when asked.
Without it, prism and mind are a competent machine that nobody wants to
talk to.

It is one of the three things the robot is made of, and they divide
cleanly:

| | | |
| --- | --- | --- |
| **prism** | intent | what the robot *does* — decision, plan, grant, execute, verify, receipt |
| **mind** | IQ | what the robot *knows* — facts, retrieval, the registry, provenance |
| **soul** | EQ | how the robot *relates* — perception, relationship, expression, reflection |

`hub` is the door every byte passes through, `trust` is the floor all three
stand on, `surfaces` is the window.

**The seam between mind and soul matters more than the one between either
and prism.** They are both "things learned about a person", and they must
not bleed:

- A **fact** is what is true about them. It lives in `mind`, carries a
  source pointer, is quotable back, and is theirs to correct or erase.
- A **perception** is a guess about their state right now. It lives in
  `soul`, expires with the turn, and is never quotable.
- A **lesson** is how they like to be spoken to. It lives in `soul`, and
  carries evidence — but it is about *manner*, not about *them*.

"You seemed annoyed on Tuesday" must never become something the robot can
recite in March. That is not a rule the code is asked to remember; it is
why perceptions have no table.

### Where Soul's output lands

Now that the rest of the robot exists, there is a precise answer to where
the expression half of Soul plugs in — and it is what makes §5's boundary
structural rather than aspirational:

> The kernel emits `Rendering { id, slots }` — structure, compiled from
> evidence. The surface turns that into words. **Soul is the policy that
> decides how.**

That is not an analogy — it is where the code goes. And it makes §5's
boundary — *"Soul shapes interpretation and expression; it never overrides
facts, permissions, policy, or verified outcomes"* — true **by
construction** rather than by discipline:

- Soul never sees a decision, only a decided result.
- Soul never sees a capability, only what a capability returned.
- Soul cannot alter a receipt, because the receipt is compiled and stored
  before the renderer is called.
- Soul cannot suppress an action record, because that is built from the
  receipt on the other side of the boundary.

A Soul that wanted to lie would have to lie about `reminder_created` with
`when_ms` and `about` sitting in front of it, downstream of a claim check
that already ran. It has no lever.

### The two paths words come out of

| path | what Soul does |
| --- | --- |
| **kernel renderings** (`reminder_created`, `registry`, …) | shapes the rendering into a sentence — length, register, warmth. The *facts in the slots are untouchable*. |
| **model prose** (`answer.model`, `web.research`) | shapes the expression prompt before generation. Never the routing prompt, never the research prompt. |

**Soul never enters a call that reads untrusted material.** The research
path already gets no tools (§6a); it gets no personality either. A dial
that made the robot more agreeable while reading an attacker's web page is
a dial that makes injection easier.

## 2. Where it runs in a turn

```
intent → decision → plan → grant → execute → verify → receipt
                                                        │
                                        claim-vs-receipt check (Q26)
                                                        │
                                                        ▼
                                      ┌──────── SOUL ────────┐
                                      │ perception (this turn)│
                                      │ dial + active lessons │
                                      └───────────┬───────────┘
                                                  ▼
                                  render → action record → outbox → person
```

Soul sits **between the receipt and the outbox**, inside
`finish_planned_intent`, at the point that today calls
`deps.renderer.render(...)`. Nothing above that line changes.

## 3. State (Q25's four tables, made concrete)

Per member cell. Instance defaults in core.

```sql
soul_persona(dimension TEXT PRIMARY KEY, value INT, floor INT, ceiling INT,
             updated_at INT)

soul_lessons(id TEXT PRIMARY KEY, statement TEXT, dimension TEXT,
             direction INT, evidence_msg_id TEXT NOT NULL REFERENCES messages(id),
             status TEXT, confidence REAL, created_at INT,
             reinforced_count INT, retired_at INT)

soul_revisions(id TEXT PRIMARY KEY, created_at INT, reason TEXT,
               diff_json TEXT, rolls_back_to TEXT, applied INT,
               evaluator_verdict TEXT)

soul_journal(id TEXT PRIMARY KEY, created_at INT, revision_id TEXT, entry TEXT)
```

`evidence_msg_id` is a real foreign key, exactly as `facts.source_msg_id`
is. **Law 5 applies to lessons**: no lesson without the words that produced
it. When the message is deleted, the lesson is retired — not orphaned, and
not silently kept.

Two-way sync: `soul_*` tables are **knowledge**, so they merge like facts,
with the same rules. Revisions are append-only and union cleanly; the dial
takes the newest value per dimension; lessons follow the fact rules
including tombstones.

## 4. The persona dial

Five dimensions, 0–100, each with an owner-set floor and ceiling.

| dimension | 0 | 100 | default |
| --- | --- | --- | --- |
| `directness` | hedged | blunt | 60 |
| `warmth` | clinical | affectionate | 55 |
| `brevity` | expansive | terse | 70 |
| `initiative` | answers only what was asked | offers and follows up | 35 |
| `formality` | casual, lower-case | formal | 25 |

**Values are Soul's; bounds are the owner's.** Evolution may move `value`
within `[floor, ceiling]`. Only the owner moves `floor`/`ceiling`. Setting
`floor == ceiling` pins a dimension — the intended way to say *stop
changing this*.

The dial renders into the expression prompt as imperative lines, not
adjectives: a number becomes "keep it to two sentences", not "be brief-ish".

**Why five, and why not more.** Each one has to be independently
observable from a person's corrections, or it cannot be learned honestly.
Humour is deliberately absent: I could not define a signal that
distinguishes "they liked the joke" from "they were being polite", and a
dial that adjusts on a misread is worse than no dial.

## 4b. Roles and personas

The dial is *how* this robot speaks. A **role** is *who it is speaking as*
— a character the owner asks for, on purpose, for as long as they want it.

```
soul_persona row: dimension = 'role'   (value/floor/ceiling unused)
role brief:       cell_meta['soul:role'] — free text, the owner's words
```

- A role is **owner-set and owner-cleared**, never inferred. Nothing
  adaptive gives the robot a new character.
- It stacks **on top of** the dial rather than replacing it: a role sets
  voice and manner, the dial still sets directness, brevity and the rest.
- It is visible in `/soul` and travels with sync, like everything else.
- It shapes expression **only**. A role cannot reach facts, permissions or
  receipts — it is downstream of all three, like the rest of Soul.

**A stance change is spoken in the old voice.** The turn's voice is
resolved before the turn runs, so "be my mentor" is answered by whoever the
robot was a moment ago, and the new stance starts with the next message.
That is deliberate rather than a rounding error: the reply to *become
someone else* is the last thing the previous self says.

**In-character speech is not a claim about the robot.** A robot playing a
Victorian butler that says "delighted, sir" is performing a role the owner
asked for. §5's *"never claims to feel"* is about the sincere question —
"do you actually care?" — which must get an honest answer whatever
character is running. The distinction is between a costume and a lie about
what is underneath it, and both people in the room know which is which.

**Where it stops being a style question.** Playing a character in a private
chat is one thing. Sending a message *as a named real person* on an
outbound channel — email, Telegram — is another, because a third party who
never agreed to the game receives it. The robot already treats outbound
send as special (Q28: `email.send` defaults to approval-always). Whether a
role may travel outbound at all is a decision the owner should take before
those capabilities exist, not after. See §12.

## 5. Perception — hypotheses that never become facts

Per turn: `{signal, value, confidence}`, e.g. `{urgency, high, 0.72}`.

- **Never stored as a fact.** No `source_msg_id`, no recall, no appearance
  in the registry. A guess about someone's mood on a Tuesday must not be
  quotable in March.
- Below a confidence floor (proposed 0.6) the perception is **dropped**, not
  down-weighted. A weak guess about a person's emotional state is worse
  than no guess.
- `"Don't infer my mood"` is a hard per-member switch, not a weight.
- Perception may shift expression **within** the dial's bounds for one
  turn. It may never write to the dial.

## 6. Lessons — how this person likes to be talked to

Not what they like. That is a fact, and facts have a home already.

**Born only from explicit signal**: a correction ("shorter"), a `/soul`
instruction, or a pattern the owner confirms when asked. **Never from
inferred satisfaction and never from absence of complaint** — silence is
not consent to a theory about someone.

**Lifecycle.** `proposed → active` on a second independent signal.
Contradiction retires immediately: one clear *"actually, do ask"* outranks
five inferred agreements. No supporting signal for 90 days → retired as
stale.

## 7. The Soul Loop

**Day pass — counting, not thinking.** Per turn, no model call: record
explicit corrections, `/soul` commands, offers accepted or refused, and the
person's own message length and register. Cheap enough to be unconditional.

**Night pass — the reflection.** Quiet-hours lane (§2a), once per night per
active member:

1. Read the day's signals and active lessons.
2. Propose lessons, reinforcements, retirements, dial moves within bounds.
3. **Verify on a different model** (Q26, and §5's evaluator-separation law).
   The evaluator must reject anything the cited evidence does not support.
4. Write a `soul_revision`: diff, reason, rollback reference, verdict.
5. Write a first-person `soul_journal` entry.

**Bounded drift: 5 points per night, 15 per rolling 30 days, per
dimension.** The robot you talk to next week must be recognisably the one
you talked to today. Unbounded adaptation is not personality, it is drift,
and nobody notices until it is large.

**Larger shifts become proposals.** Exceeding the nightly bound, crossing
an owner bound, or retiring more than three lessons at once produces a
proposal that waits for a yes — the same posture as the §6b confirmation
gate. An inference may suggest; an instruction decides.

## 8. Evaluator separation

§5 states it as law: verification never runs on the model that generated.
Two obligations, and today **neither is built**:

- **Sampled expression-verify** — 10% of routine turns, **always** on
  actioned or risky ones, on a different seat than the generator. Post-send
  for routine (audit), pre-send for actioned.
- **The night pass evaluator** — §7 step 3.

Both use a `Role::Evaluator` seat, distinct from `Answer` and `Route`.

## 9. The immutable core

Outside `soul_persona` entirely — so there is no field to move, rather than
a rule saying not to:

the five laws · never claiming an effect without a receipt · never claiming
to feel *when asked sincerely* · honesty about uncertainty · the owner's
own persona directive.

*(Corrected 2026-07-31: an earlier draft of this list included "never
pretending to be human". That was the builder's invention, not the spec's —
the frozen docs say only "it never claims to feel". A robot that may take
on a role needs the narrower constraint, and the narrower one is the one
that was actually decided.)*

## 10. Owner surface

Every one answerable **from stored state, with no model call.** A robot
that needed to ask a model why it was talking a certain way would be
guessing at its own reasons.

| command | behaviour |
| --- | --- |
| `/soul` | the dial, bounds, and whether evolution is on |
| "Why are you speaking this way?" | the dial plus the lessons that moved it, each with its evidence |
| "Show what you've learned about my style" | active lessons, evidence, dates |
| "Don't infer my mood" | perception off for this member until re-enabled |
| "Be less agreeable" | `directness +`, `warmth −` within bounds, as an owner-instructed revision |
| "Restore last month's behaviour" | roll back to the revision active then; the rollback is itself a revision |
| `/soul history` | revisions, diffable |
| `/soul pin <dimension>` | floor = ceiling = current |
| `/soul off` | evolution off; the dial stays where it is |

## 11. Build plan

Each stage ends green and is useful alone. **Adaptation is last on
purpose** — the robot becomes inspectable and controllable before it
becomes adaptive.

| stage | what ships | gate |
| --- | --- | --- |
| **S1 — state and control** | four tables; dial with owner bounds; `/soul`, `pin`, `off`; instance defaults. No adaptation, no perception. | dial persists, survives restart, syncs between instances; bounds cannot be crossed by anything |
| **S2 — expression** | the dial actually shapes both paths; `Role::Evaluator` seat; sampled expression-verify | same turn at opposite dial settings produces visibly different wording and *identical* slots; verify never runs on the generating model |
| **S3 — perception** | per-turn hypotheses, confidence floor, the off switch | a perception never reaches `facts`; the off switch is absolute; low-confidence signals are dropped |
| **S4 — lessons** | day pass, signals, lesson lifecycle, `why`, `learned` | no lesson without evidence; deleting the evidence retires the lesson; contradiction retires immediately |
| **S5 — evolution** | night pass, revisions, journal, bounded drift, proposals | drift bounds hold under a simulated month; a proposal never self-applies; evaluator rejection blocks a revision |
| **S6 — reversibility** | `history`, diffs, restore-to-date | restoring reproduces the earlier dial exactly, and is itself recorded |

**S1–S2 are worth building now.** S3 onward should wait until you have used
S2 for a while — the right defaults for drift bounds are not knowable from
an armchair, and every number in §7 is currently mine rather than yours.

## 12. Open questions

1. **AEI** — named once in §5, defined nowhere, and no document defines it.
   Either tell me what it is, or **strike it from §5**. I would rather
   delete a phantom than implement a guess.
2. **Drift bounds** (5/night, 15/30 days) — invented by me as "slow enough
   to notice". Yours to set.
3. **Lesson staleness at 90 days** — same.
4. **Perception confidence floor at 0.6** — same.
5. **Five dimensions** — right five? Is `initiative` genuinely separable
   from `directness` in practice?
6. **Journal visibility** — shown in `/soul` by default, or on request?
7. **Sampling rate for expression-verify** — Q26 says 10%; confirm it is
   still right now that turns cost more than they did.
8. **May a role travel outbound?** In a private chat, playing a character is
   between the two of you. An email or a Telegram message sent *as* a named
   real person reaches someone who never agreed to it. My proposal: roles
   shape chat freely, and outbound sends always carry the robot's own
   identity regardless of role. Yours to decide, and worth deciding before
   `email.send` exists rather than after.
