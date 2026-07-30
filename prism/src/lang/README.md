# Language packs

Everything the Robot *says* and every phrase it *understands* lives here.
Nothing in the code does. Adding a language is adding a file — no Rust
change, ever.

`en.toml` is canonical: it defines the command ids and reply ids every other
pack maps onto.

## Adding a language

1. Copy `en.toml` to `xx.toml` (ISO 639-1 code) and set `code` / `name`.
2. Declare the `scripts` the language is written in (`Cyrillic`, `Latin`,
   `Han`, `Arabic`, …). Latin-script languages are chosen by phrase match,
   not by script, so `Latin` alone decides nothing.
3. Translate. Keep every id exactly as it is — the ids are the interface;
   only the values are the language.
4. Register it in `EMBEDDED` in `../lexicon.rs` (one line).
5. `cargo test -p prism`. The suite refuses a pack that is missing a reply,
   invents a command id, or ships without effect-claim phrases.

## What each section is for

| Section | What it holds |
| --- | --- |
| `[commands]` | whole utterances, matched **exactly** — never substrings |
| `[heads]` | phrases that introduce an argument (`remind me …`), with `skip` = how many whitespace tokens the head occupies |
| `greetings` | openers that route to chitchat, not to a capability |
| `effect_claims` | first-person "i saved it" phrases — **safety**, see below |
| `[signals]` | handling cues (`escalate_super`, `escalate_ultra`) |
| `[particles]` | grammar words the structured parsers consume |
| `[calendar]` | weekday and month names, Monday first, January first |
| `[formats]` | datetime layouts: `{hh} {mm} {weekday} {weekday_short} {day} {month} {month_short} {year}` |
| `[replies]` | everything the Robot says on the deterministic path |

## Two lists that are not cosmetic

`effect_claims` feeds the §5/Q26 claim-vs-receipt check: if an utterance
asserts the Robot changed something and the turn executed no such step, the
receipt goes `uncertain` and the person is told. A pack that ships without
these phrases is a language in which the Robot could claim an effect it
never performed and nothing would catch it. Both lists are matched against
**every** pack, not just the turn's own, so a reply that drifts into another
language is still checked.

## A language with no pack is not an error

The floor declines — it does not guess — and the turn goes to the verdict,
which is multilingual by nature; the model answers in the person's language.
Coverage degrades from "instant and free" to "one model call". Nothing
breaks. That is tested, not assumed:
`an_unpacked_language_still_completes_a_governed_turn`.
