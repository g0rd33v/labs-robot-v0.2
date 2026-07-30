#!/usr/bin/env bash
#
# Off-site backup: seal the Robot's state and push it to every destination.
#
# Safe to run while the Robot is live -- cells are snapshotted online with
# VACUUM INTO, and connections carry a busy timeout (a backup during a write
# used to be a coin flip).
#
# DESTINATIONS ARE INDEPENDENT by design. Two providers only help if one
# failing is visible, so every destination is attempted, each is verified
# separately, and the script exits non-zero if ANY failed -- naming which.
# A success at one never masks a failure at the other.
#
#   1. hetzner storage box  (ssh/scp, port 23)
#   2. digitalocean spaces  (s3 sigv4 via scripts/s3-put.sh)
#
# WHAT THIS PROTECTS: the encrypted cells, media vault, and core registry.
# WHAT IT DOES NOT: the key. Tarballs are sealed under a key derived from
# data/kek.key. Keep a copy of kek.key somewhere that is NEITHER destination
# -- key and ciphertext in one place is the same as no encryption, and
# losing both is unrecoverable by design (arch sec 13d).
#
# usage:  scripts/backup-offsite.sh [--keep N]

# NOT -e: one failing destination must not abort the others.
set -uo pipefail

KEEP=14; [ "${1:-}" = "--keep" ] && [ -n "${2:-}" ] && KEEP="$2"

# --- destination 1: hetzner storage box ---
SB_HOST="u639707.your-storagebox.de"
SB_USER="u639707"
SB_PORT=23
SB_KEY="$HOME/.ssh/labs_hub"
SB_DIR="bender/backups"

# --- destination 2: digitalocean spaces ---
S3_PREFIX="bender/backups"

cd "$(dirname "$0")/.." || exit 1
PROJECT="$(pwd)"

SSH_OPTS=(-n -p "$SB_PORT" -i "$SB_KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=20)
SCP_OPTS=(-P "$SB_PORT" -i "$SB_KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=20)

say() { printf '%s\n' "$*"; }
FAILURES=()

# launchd/PATH-independent: resolve a real binary rather than shelling out
# to the build tool.
ROBOTD=""
for cand in target/release/robotd target/debug/robotd; do
    [ -x "$cand" ] && ROBOTD="$PROJECT/$cand" && break
done
if [ -z "$ROBOTD" ]; then
    command -v cargo >/dev/null 2>&1 || { say "!! FAILED: no robotd binary and no cargo"; exit 1; }
    ROBOTD="cargo run -q -p robotd --"
fi

# ------------------------------------------------------------- seal

say "==> sealing a backup"
OUT=$($ROBOTD backup) || { say "!! FAILED to seal a backup"; exit 1; }
LOCAL_PATH=$(printf '%s' "$OUT" | sed -n 's/^backup sealed: //p')
[ -f "$LOCAL_PATH" ] || { say "!! FAILED: backup produced no file"; exit 1; }
NAME=$(basename "$LOCAL_PATH")
LOCAL_SUM=$(shasum -a 256 "$LOCAL_PATH" | awk '{print $1}')
say "    $NAME ($(du -h "$LOCAL_PATH" | cut -f1)), sha256 ${LOCAL_SUM:0:16}..."

# ------------------------------------------ destination 1: storage box

say "==> [storage box] uploading"
if ssh "${SSH_OPTS[@]}" "$SB_USER@$SB_HOST" "mkdir -p $SB_DIR" \
   && scp "${SCP_OPTS[@]}" "$LOCAL_PATH" "$SB_USER@$SB_HOST:$SB_DIR/"; then
    REMOTE_SUM=$(ssh "${SSH_OPTS[@]}" "$SB_USER@$SB_HOST" "sha256sum $SB_DIR/$NAME" 2>/dev/null | awk '{print $1}')
    if [ "$LOCAL_SUM" = "$REMOTE_SUM" ]; then
        say "    verified byte-identical"
        # The box's restricted shell has ls/rm but no wc or xargs, so all
        # selection happens here and only a plain rm goes over the wire.
        # (ssh inside a read-loop eats stdin, so the deletes are batched.)
        REMOTE_LIST=$(ssh "${SSH_OPTS[@]}" "$SB_USER@$SB_HOST" "ls $SB_DIR" 2>/dev/null | grep '\.sealed$' | sort)
        REMOTE_COUNT=$(printf '%s\n' "$REMOTE_LIST" | grep -c .)
        if [ "$REMOTE_COUNT" -gt "$KEEP" ]; then
            TARGETS=""
            while IFS= read -r old; do
                [ -n "$old" ] && TARGETS="$TARGETS $SB_DIR/$old"
            done <<< "$(printf '%s\n' "$REMOTE_LIST" | head -n "$((REMOTE_COUNT - KEEP))")"
            [ -n "$TARGETS" ] && ssh "${SSH_OPTS[@]}" "$SB_USER@$SB_HOST" "rm -f$TARGETS"
            say "    pruned to $KEEP"
        fi
    else
        say "!! FAILED [storage box]: checksum mismatch -- the remote copy is NOT trustworthy"
        FAILURES+=("storage box: checksum mismatch")
    fi
else
    say "!! FAILED [storage box]: could not upload (host unreachable or key rejected)"
    FAILURES+=("storage box: upload failed")
fi

# ---------------------------------------------- destination 2: spaces

say "==> [spaces] uploading"
if ./scripts/s3-put.sh "$LOCAL_PATH" "$S3_PREFIX/$NAME" >/dev/null 2>&1; then
    # verify by reading back what the bucket actually holds, not by
    # trusting the upload's exit code
    TMP_BACK=$(mktemp)
    if ./scripts/s3-put.sh --get "$S3_PREFIX/$NAME" "$TMP_BACK" >/dev/null 2>&1; then
        BACK_SUM=$(shasum -a 256 "$TMP_BACK" | awk '{print $1}')
        if [ "$LOCAL_SUM" = "$BACK_SUM" ]; then
            say "    verified byte-identical (read back from the bucket)"
        else
            say "!! FAILED [spaces]: what came back does not match what went up"
            FAILURES+=("spaces: checksum mismatch")
        fi
    else
        say "!! FAILED [spaces]: uploaded but could not read it back"
        FAILURES+=("spaces: verification read failed")
    fi
    rm -f "$TMP_BACK"
else
    say "!! FAILED [spaces]: upload rejected (credentials or bucket)"
    FAILURES+=("spaces: upload failed")
fi

# --------------------------------------------------------- local prune

# shellcheck disable=SC2012
ls -t data/backups/*.sealed 2>/dev/null | tail -n +"$((KEEP + 1))" | while read -r old; do
    rm -f "$old"
done

# -------------------------------------------------------------- verdict

if [ ${#FAILURES[@]} -eq 0 ]; then
    say "==> done: $NAME is on 2 independent destinations"
    exit 0
fi
say "!! FAILED at ${#FAILURES[@]} of 2 destinations: ${FAILURES[*]}"
exit 1
