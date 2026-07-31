# Plan — two-way sync between instances

Status: **implemented** 2026-07-31. Owner chose: tombstones that propagate
and are then collected; conflicting edits both survive; automatic whenever
the peer is present.
Extends: §8 Robot Package, which is transferability (one-way, point-in-time).
Two-way convergence is a new mechanism and a change of scope, not an
implementation detail of the existing one.

---

## 1. What can and cannot merge

The robot holds two different kinds of thing, and only one of them can be
merged at all.

**Knowledge — what the robot knows.** Messages, facts, reminders, media.
These are per-person state, and merging them is meaningful: a fact learned
on the stick is a fact the robot knows.

**History of doing — what an instance did.** Journal, receipts, outbox,
pending confirmations, and the Boundary Log. These are records of turns
that happened on a particular machine at a particular time.

**The Boundary Log settles the argument.** It is a hash chain: each entry
commits to `prev_hash`, and `verify_chain` walks it. Two instances that
both appended have two chains, and there is no merge of two hash chains
that is still a hash chain. You can concatenate them and lose
verifiability, or keep them separate and keep it.

So: **knowledge syncs, history does not.** Each instance keeps its own
journal, receipts and chain, and each remains independently verifiable.
That is not a limitation to apologise for — "here is what *this* robot did,
and here is the unbroken chain proving it" stays true on both machines. A
merged history would be a claim neither machine could support.

## 2. What syncs, precisely

| table | merge | why |
| --- | --- | --- |
| `messages` | union by `id` | append-only; ids are unique per instance |
| `facts` | union by `id`, plus supersession chains | see §3 |
| `reminders` | union by `id`, status by precedence | see §4 |
| `media` | union by content hash | content-addressed already |
| `cell_meta` | last write wins per key | preferences, not facts |

Never synced: `journal`, `receipts`, `outbox`, `pending_calls`,
`boundary_log`, `invites`, sessions.

Provenance survives by construction: `facts.source_msg_id` points at a
message, and messages sync too. A fact never arrives without the words it
came from — law 5 holds across the boundary as well as inside it.

## 3. Facts: the tension worth naming

The robot promises: *"forget fact N deletes for real — the row is deleted,
not hidden."* That promise and replication pull against each other.

For a deletion to propagate, the other side must learn it happened — a
**tombstone**, a record that fact `X` was deleted. Without one, sync
resurrects deleted facts, and the erase right is silently broken: the
person deletes something, the stick brings it back, and nothing announced
it.

Three ways out, and this is a decision, not a detail:

- **(a) Tombstones, garbage-collected.** A deletion records `id` and
  timestamp — never the content — the other side applies it, and once both
  sides have acknowledged it, the tombstone is deleted too. Deletion
  propagates; the trace is transient. *Recommended.*
- **(b) Tombstones, permanent.** Simpler, but the robot keeps a permanent
  list of things it was told to forget, which is a weaker version of the
  promise.
- **(c) No tombstones.** Deletes never propagate. Anything deleted on one
  machine returns from the other on the next sync. This defeats the erase
  right, and I would not ship it.

**Conflicting edits.** The registry already models correction as
supersession rather than overwrite, so two machines correcting the same
fact produce two chains over one ancestor. Merging them by keeping *both*
loses nothing and stays inspectable — which is what the registry is for.
The newest is current; the rest are visible as superseded. The alternative,
last-writer-wins on wall-clock time, silently discards one edit and depends
on two machines agreeing about the time.

## 4. Reminders: precedence, not timestamps

A reminder is a small state machine: `active → cancelled | fired`. Two
machines can disagree, and wall clocks are not a safe tiebreak.

**Terminal beats active.** If either side has `cancelled` or `fired`, that
wins. Resurrecting a cancelled reminder is worse than dropping a
resurrection: one nags about something called off, the other does nothing.
Between `cancelled` and `fired`, take the earlier — both are done.

## 5. Transport

A **sealed delta**, in the shape the package already uses:

```
robotd sync --with <path>       # e.g. the stick, or a mounted volume
```

1. Read the watermark for that peer (`last_synced_at`, per instance id).
2. Export everything newer into one file, encrypted under a key derived
   from the KEK — a stick left in a taxi leaks nothing.
3. Import the peer's file the same way.
4. Advance both watermarks, boundary-log both directions, and write a
   receipt: rows in, rows out, conflicts resolved, tombstones applied.

Both instances share a `robot_id` (restore preserves it) but need a
distinct `instance_id`, minted at restore, so watermarks and origins are
attributable. Sync refuses across different `robot_id`s — merging two
different robots' memories is never what anyone meant.

The delta is a file, so this works for anything that can hold one: a USB
stick, a shared folder, an object store. Nothing here is USB-specific.

## 6. Decisions, as taken

1. **Deletion** — tombstones that propagate and are then collected once both
   sides have applied them. This is why the pass is two-way in one call: a
   one-way push could never know the other side had applied anything, so it
   could never safely forget that it had deleted something.
2. **Conflicting edits** — both survive as supersession chains, newest
   current. Nothing is discarded because two clocks disagreed.
3. **When** — automatic whenever the peer is present, every
   `sync.every_minutes`. `robotd sync --with <path>` always works too.
   A peer that is not plugged in is skipped **silently**: absence is the
   normal state of a removable disk, and a robot that complains every ten
   minutes about a drawer is one people stop reading. A peer that is
   present but unusable *is* reported in chat.

## 7. What this costs

Roughly a milestone: schema for tombstones, instance ids and watermarks; a
sealed delta format; merge logic per table with the conflict rules above;
the `sync` subcommand; receipts and boundary logging; and a test suite that
diverges two instances deliberately and proves convergence — including the
awkward cases (delete vs edit, cancel vs fire, the same fact corrected on
both sides, a sync interrupted halfway).

Two properties I want the tests to hold, not just the code:

- **Convergence.** Sync twice in either order and both instances agree.
- **No resurrection.** Something deleted on either side is gone on both,
  and stays gone across further syncs.
