# Recovery

What to do when the machine holding the Robot is gone.

Every step below has been executed and verified, except where marked
otherwise. Written to be followed under stress, by someone who did not
build this.

---

## What exists, and where

| Thing | Where it lives | Protects |
|---|---|---|
| **Code + specs** | `hub.labs.co/git/labs/robot-bender` (private) | the program |
| **Sealed backups** | Hetzner storage box `u639707.your-storagebox.de:23`, `bender/backups/` | the memory |
| **Sealed backups** | DigitalOcean Spaces `backup-labs-co` (ams3), `bender/backups/` | the memory |
| **`kek.key`** | owner's password manager | *unseals the two above* |

The two backup destinations are independent and hold the same sealed
tarballs. Either one is sufficient.

**The key is the whole story.** Backups are sealed under a key derived from
`kek.key`. With it, either copy restores completely. Without it, both are
unreadable and nothing anyone can do will change that — the same property
that makes "forget this" actually mean forgotten (arch §13d). There is no
backdoor, by design.

---

## Recovering onto a new machine

### 1. Get the toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
```

### 2. Get the code

```bash
git clone ssh://git@hub.labs.co:2222/labs/robot-bender.git
cd robot-bender
```

Gitea's SSH is on **port 2222** — port 22 is the host's own sshd and will
refuse the key. The SSH key must be registered under Gitea → Settings →
SSH Keys.

### 3. Get a backup — either destination

```bash
# Hetzner storage box
scp -P 23 -i ~/.ssh/labs_hub \
    u639707@u639707.your-storagebox.de:bender/backups/<newest>.tar.sealed .
```

```bash
# or DigitalOcean Spaces (needs DO_SPACES_KEY / DO_SPACES_SECRET in the keychain)
./scripts/s3-put.sh --get bender/backups/<newest>.tar.sealed ./<newest>.tar.sealed
```

To list what is available: `ssh -p 23 -i ~/.ssh/labs_hub
u639707@u639707.your-storagebox.de "ls bender/backups"`. Filenames carry a
millisecond timestamp, so the last one alphabetically is the newest.

### 4. Put the key back

```bash
mkdir -p data
# paste the saved kek.key content into data/kek.key, then:
chmod 600 data/kek.key
```

The file is 64 hex characters, no newline needed. This is the step that
cannot be automated and cannot be recovered from anywhere else.

### 5. Restore

```bash
cargo run -p robotd -- backup-restore ./<newest>.tar.sealed ./restored
```

This unseals into `./restored/` — `manifest.json`, `core.db`, `cells/`,
`media/`. Check the manifest names the expected `robot_id` and cell count.

### 6. Run the restored Robot

```bash
rm -rf data.old && mv data data.old 2>/dev/null || true
mkdir -p data
cp -R restored/core.db restored/cells restored/media data/
cp data.old/kek.key data/ 2>/dev/null || true   # or re-place it from step 4
cargo run -p robotd
```

The slug URL is printed at boot. Identity, memory, receipts and reminders
come back as they were; the boundary log verifies at boot and the dashboard
shows the chain status.

> Embeddings: the restored config has `mind.embeddings = false` if it came
> from a *package*. Models are runtime, not essence — set it to `true` and
> the weights re-download on next boot. Until then recall degrades to
> FTS + recency, honestly and without error.

---

## Recovering from a Robot Package instead

`robotd package` produces a **self-contained** artifact: it carries the keys
and is sealed under a one-time code printed at export. If you have a package
and its code, `kek.key` is not needed separately.

```bash
cargo run -p robotd -- restore <file>.pkg --code <code> --into ./bender --port 7777
cd ./bender && robotd
```

Verified: package → restore into a blank directory → the Robot boots there
and remembers. Restore refuses to overwrite an existing robot without
`--force`, and even then moves the old data aside rather than deleting it.

---

## Sanity checks after any recovery

```bash
cargo test --workspace                       # expect all green
cargo run -p robotd -- eval                  # routing, kill-suite, latency
```

Then, in the chat:

- `my facts` — the registry, with sources
- `my reminders` — pending commitments
- open `/dash` — boundary log should say **chain verified**

---

## What is NOT recoverable

- **`kek.key` lost and no package code** → both backup copies are
  permanently unreadable. This is the deliberate trade for real erasure
  (§13d); the product says so at setup rather than pretending otherwise.
- **Model weights** — not in backups. They re-download; they are runtime.
- **API keys** (`OPENROUTER_API_KEY`, `SERPER_API_KEY`, `DO_SPACES_*`) — in
  the macOS keychain, never in backups by design. Re-create them from the
  providers; the Robot runs without them, just with the deterministic floor
  and no web search.

---

## Verified when written (2026-07-31)

- backup taken while the Robot was live, uploaded to both destinations,
  each verified byte-identical
- pulled back from the **storage box**, restored, both cells + media present
- pulled back from **Spaces**, restored, both cells + media present
- one destination failing does not mask the other: the failure is named and
  the run exits non-zero
- package → restore into a blank directory → Robot boots and remembers
  (this is the M7 transferability proof, re-run as `robotd restore`)

Not verified, because it needs a genuinely different machine: steps 1–2 and
6 end to end on fresh hardware. The pieces are each verified; the assembly
on new hardware is not.
