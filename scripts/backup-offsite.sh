#!/usr/bin/env bash
#
# Off-site backup: seal the Robot's state and push it to the storage box.
#
# Safe to run while the Robot is live -- cells are snapshotted online with
# VACUUM INTO, and connections carry a busy timeout (a backup during a write
# used to be a coin flip).
#
# WHAT THIS PROTECTS: the encrypted cells, media vault, and core registry.
# WHAT IT DOES NOT: the key. The tarball is sealed under a key derived from
# data/kek.key, so restoring needs that file. Keep a copy of kek.key somewhere
# that is NOT this storage box -- a password manager attachment, another
# machine. Box + key in one place is the same as no encryption; losing both
# is unrecoverable by design (arch sec 13d).
#
# usage:  scripts/backup-offsite.sh [--keep N]

set -euo pipefail

HOST="u639707.your-storagebox.de"
USER_="u639707"
PORT=23
KEY="$HOME/.ssh/labs_hub"
REMOTE_DIR="bender/backups"
KEEP=14; [ "${1:-}" = "--keep" ] && [ -n "${2:-}" ] && KEEP="$2"   # retention, local and remote

cd "$(dirname "$0")/.."

SSH_OPTS=(-p "$PORT" -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=20)
SCP_OPTS=(-P "$PORT" -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=20)

say() { printf '%s\n' "$*"; }

say "==> sealing a backup"
OUT=$(cargo run -q -p robotd -- backup)
LOCAL_PATH=$(printf '%s' "$OUT" | sed -n 's/^backup sealed: //p')
[ -f "$LOCAL_PATH" ] || { say "!! backup did not produce a file"; exit 1; }
NAME=$(basename "$LOCAL_PATH")
SIZE=$(du -h "$LOCAL_PATH" | cut -f1)
say "    $NAME ($SIZE)"

say "==> uploading"
ssh "${SSH_OPTS[@]}" "$USER_@$HOST" "mkdir -p $REMOTE_DIR"
scp "${SCP_OPTS[@]}" "$LOCAL_PATH" "$USER_@$HOST:$REMOTE_DIR/"

say "==> verifying the copy is byte-identical"
LOCAL_SUM=$(shasum -a 256 "$LOCAL_PATH" | awk '{print $1}')
REMOTE_SUM=$(ssh "${SSH_OPTS[@]}" "$USER_@$HOST" "sha256sum $REMOTE_DIR/$NAME" | awk '{print $1}')
if [ "$LOCAL_SUM" != "$REMOTE_SUM" ]; then
    say "!! checksum mismatch -- the remote copy is NOT trustworthy"
    say "   local:  $LOCAL_SUM"
    say "   remote: $REMOTE_SUM"
    exit 1
fi
say "    ok ($LOCAL_SUM)"

say "==> pruning to the newest $KEEP"
# The storage box runs a restricted shell: ls / mkdir / rm / sha256sum are
# available, but wc, xargs and command -v are not. So all selection logic
# runs HERE and only plain `rm` is sent over. Filenames carry a millisecond
# timestamp, so a lexical sort is chronological.
# shellcheck disable=SC2012
ls -t data/backups/*.sealed 2>/dev/null | tail -n +"$((KEEP + 1))" | while read -r old; do
    say "    local:  $(basename "$old")"
    rm -f "$old"
done

REMOTE_LIST=$(ssh -n "${SSH_OPTS[@]}" "$USER_@$HOST" "ls $REMOTE_DIR" 2>/dev/null \
              | grep '\.sealed$' | sort || true)
REMOTE_COUNT=$(printf '%s\n' "$REMOTE_LIST" | grep -c . || true)
if [ "$REMOTE_COUNT" -gt "$KEEP" ]; then
    # Build the whole delete list first and send ONE rm. Looping with ssh
    # inside silently under-deletes: ssh reads stdin, so it swallows the
    # remaining lines of the pipe and only the first file is removed.
    DOOMED=$(printf '%s\n' "$REMOTE_LIST" | head -n "$((REMOTE_COUNT - KEEP))")
    TARGETS=""
    while IFS= read -r old; do
        [ -n "$old" ] || continue
        say "    remote: $old"
        TARGETS="$TARGETS $REMOTE_DIR/$old"
    done <<< "$DOOMED"
    [ -n "$TARGETS" ] && ssh -n "${SSH_OPTS[@]}" "$USER_@$HOST" "rm -f$TARGETS"
    REMOTE_COUNT=$KEEP
fi

say "==> done: $REMOTE_COUNT backup(s) off-site at $HOST:$REMOTE_DIR"
