# SOUL-MASTER — **DRAFT, NOT ACCEPTED**

Status: **proposed by the builder, awaiting the owner's acceptance, edit or
rejection.** Nothing in here is frozen and nothing has been implemented
from it. The filename says `-draft` deliberately; if you accept it, rename
it and it becomes spec.

Why it exists: architecture §5 defines Soul's *runtime contract* and says
the full design — the persona dial, the AEI framing, the Soul Loop and
evolution mechanics — "is specified in SOUL-MASTER". That document is not
in `docs/`, so M4 Soul is blocked on semantics nobody has written down.
This is an attempt to write them down so you have something concrete to
argue with, which is faster than describing it from scratch.

**One thing I could not do honestly.** The frozen docs reference **AEI**
once, never expand it, and define nothing about it. I have not guessed. §6
below states what I think the slot is *for* and asks you to fill it. If AEI
means something specific to you, that section is wrong and should be
replaced wholesale.

---

## 1. What Soul is, and the line it must not cross

Soul decides **how** something already true is said. It never decides
**what** is true.

Everything Soul touches is downstream of the receipt. A fact is a fact, a
permission is a permission, an outcome is an outcome — Soul cannot soften a
refusal into a maybe, cannot warm a failure into a success, cannot decline
to mention an effect because the mood was wrong. §5 states this as "it never
overrides facts, permissions, policy, or verified outcomes — and it never
claims to feel", and everything below is written to keep that true even
when the machinery gets clever.

Two consequences worth stating up front:

- **Soul runs after verification, never before.** It shapes expression of a
  result that is already compiled from evidence. It is not consulted about
  whether to perform an effect.
- **Soul may not author effect claims.** The claim-vs-receipt check and the
  action record both stand between Soul and the person. If a Soul-shaped
  sentence asserts a change no step performed, the existing machinery flags
  it exactly as it would flag a model's.

## 2. The persona dial

Five dimensions. Each is an integer 0–100 with an owner-set floor and
ceiling; Soul may move within the bounds, never outside them.

| dimension | 0 | 100 | default | why it exists |
| --- | --- | --- | --- | --- |
| `directness` | hedged, softened | blunt, no cushioning | 60 | the single biggest source of "why is it talking like that" |
| `warmth` | clinical | affectionate | 55 | separable from directness; blunt and kind is a real combination |
| `brevity` | expansive | terse | 70 | Bender's voice is short by default |
| `initiative` | answers only what was asked | offers, suggests, follows up | 35 | the dimension most likely to annoy; starts low |
| `formality` | casual, lower-case | formal register | 25 | matches the existing voice |

**Bounds are the owner's, not Soul's.** `soul_persona` stores
`(dimension, value, floor, ceiling)`. Evolution may adjust `value`; only
the owner may adjust `floor`/`ceiling`. Setting `floor == ceiling` pins a
dimension and switches evolution off for it — that is the intended way to
say "stop changing this".

**Defaults are instance-level, in core; live values are per member cell.**
Two people talking to the same robot get different dials, and neither can
see the other's — that is law 2, and it applies to relationship state as
much as to facts.

### What the dial actually does

It renders into the expression prompt as explicit instruction, not vibes.
A dial of `directness 80, warmth 40, brevity 90` becomes a handful of
imperative lines. The dial never enters the routing call, the research
call, or any call that reads untrusted material — those have a job that
personality can only corrupt.

## 3. Perception: hypotheses, never facts

§5 says perception is "always as *hypotheses with confidence*, never as
facts about the owner". Made concrete:

- Perception produces `{signal, value, confidence}` for a turn — e.g.
  `{urgency, high, 0.7}`. It is **per-turn state, not stored knowledge**.
- **Perceptions never become facts.** They do not enter `facts`, they do
  not get a `source_msg_id`, they are not recalled weeks later. A guess
  about someone's mood on a Tuesday is not something a robot should be able
  to quote back in March.
- Below a confidence floor (proposed: 0.6) a perception is dropped rather
  than acted on. A weak guess about someone's emotional state is worse than
  no guess.
- `"Don't infer my mood"` sets a per-member switch that disables perception
  entirely. It must be a real switch, not a lowered weight.

## 4. Lessons

A lesson is a **source-linked statement about how this person prefers to be
communicated with.** Not what they like — that is a fact, and facts already
have a home with provenance.

```
soul_lessons: id, statement, dimension, direction, evidence_msg_id,
              status, confidence, created_at, reinforced_count, retired_at
```

- `statement` — "prefers no follow-up questions on quick factual answers"
- `dimension` — which dial dimension it bears on, or `none`
- `direction` — `+1` / `-1` / `0`, how it would push that dimension
- `evidence_msg_id` — **required**, FK to `messages`. Law 5 applies here as
  it applies everywhere: no lesson without the words that produced it. A
  lesson whose evidence is deleted is retired, not orphaned.
- `status` — `proposed → active → retired`

**How one is born.** Only from an explicit signal in a turn: a correction
("shorter, please"), a `/soul` instruction, or a repeated pattern the owner
confirms. **Not** from inferred satisfaction, and not from the absence of
complaint. Silence is not consent to a theory about someone.

**Reinforcement and decay.** A lesson reinforced by a second independent
signal moves `proposed → active`. A lesson contradicted is retired
immediately — one clear "actually, do ask" outranks five inferred
agreements. A lesson with no supporting signal for 90 days is retired as
stale rather than kept forever on the strength of one remark in March.

## 5. The Soul Loop

### Day pass — cheap, per turn

Collect signals: explicit corrections, `/soul` commands, refusal or
acceptance of offers, and the person's own message length and register.
Store as raw signals. **No model call.** This is counting, not thinking.

### Night pass — the reflection

In the quiet-hours lane (§2a). Once per night, per member with activity:

1. Read the day's signals and the active lessons.
2. Propose: new lessons, reinforcements, retirements, and dial adjustments
   within bounds.
3. **Verify on a different model** (Q26's evaluator seat, and §5's
   evaluator-separation law): the proposal is reviewed by a model that did
   not generate it, which must reject anything unsupported by the cited
   evidence.
4. Write a `soul_revision` — the diff, the reason, a rollback reference.
5. Write a first-person `soul_journal` entry in the member's cell.

**Bounded per night.** No dimension may move more than **5 points** in one
revision, and no more than **15** in a rolling 30 days. The robot you talk
to next week should be recognisably the one you talked to today; drift is
the failure mode nobody notices until it is large.

### Larger shifts become proposals

A change that would exceed the nightly bound, cross an owner bound, or
retire more than three lessons at once is written as a **proposal**, not
applied. It appears in `/soul` and the Dashboard and waits. This is the
same posture as §6b's confirmation gate: an inference may suggest, an
instruction decides.

## 6. AEI — **not specified; needs you**

The frozen docs name AEI once, in the §5 scope note, and define nothing.
I have deliberately not invented a meaning.

What I believe the slot is *for*, from context: a framing for how the robot
models the person's state and its own conduct toward it — the thing that
makes "perception → relationship → expression" coherent rather than three
unrelated subsystems.

**Please fill this in, or tell me it is not needed and should be struck
from §5's scope note.** Everything else in this document stands without it;
this section is the one place I would be guessing.

## 7. The immutable core

Untouchable by evolution, at any dial setting, in any revision:

- The five laws.
- Never claiming an effect without a receipt.
- Never claiming to feel.
- Never pretending to be human.
- Honesty about uncertainty — no dial setting makes "i don't know" go away.
- The owner's own persona directive.

Attempting to write these is a bug, not a low-probability event: they live
outside `soul_persona` entirely so there is no field to move.

## 8. Owner commands

§5 lists them; here is what each does.

| command | behaviour |
| --- | --- |
| "Why are you speaking this way?" | the active dial, plus the lessons that moved it, each with the message it came from |
| "Show what you've learned about my style" | active lessons with evidence and dates |
| "Don't infer my mood" | perception off for this member, permanently, until re-enabled |
| "Be less agreeable" | `directness +`, `warmth −` within bounds, recorded as an owner-instructed revision |
| "Restore last month's behaviour" | roll back to the revision active at that date; the rollback is itself a revision |
| `/soul history` | the revision list, diffable |
| `/soul pin <dimension>` | sets floor = ceiling = current, freezing it |

Every one of these is answerable from stored state without a model call.
"Why are you speaking this way" that needed a model to answer would be a
robot guessing at its own reasons.

## 9. What Soul must never do

- Store a perception as a fact.
- Create a lesson without evidence.
- Move a dial outside owner bounds.
- Change its own bounds.
- Soften, delay or omit a failure, a refusal, or an effect.
- Apply a revision that the evaluator did not pass.
- Claim to feel anything.

## 10. Open questions for you

1. **AEI** (§6) — what is it?
2. **Nightly bound of 5 points, 15 per 30 days** — invented by me as a
   plausible "slow enough to notice". Yours to set.
3. **90-day lesson staleness** — same.
4. **Perception confidence floor of 0.6** — same.
5. **Five dimensions** — is `initiative` really separable from
   `directness` in practice, and is anything missing? Humour, for instance,
   is deliberately absent: I could not see how to bound it safely.
6. **Journal visibility** — the journal is first-person and in the member's
   cell. Is it shown by default in `/soul`, or only on request?
