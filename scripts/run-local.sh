#!/usr/bin/env bash
# Run the Robot locally with its keys.
#
# The keys live in the macOS Keychain and reach the process only as
# environment variables, held in memory -- never written to disk, never in
# model context. No key simply means the deterministic English floor, which
# is a working robot, not a broken one.
set -uo pipefail
cd "$(dirname "$0")/.."

export OPENROUTER_API_KEY="$(security find-generic-password -a bender -s OPENROUTER_API_KEY -w 2>/dev/null || true)"
export SERPER_API_KEY="$(security find-generic-password -a bender -s SERPER_API_KEY -w 2>/dev/null || true)"

[ -n "${OPENROUTER_API_KEY:-}" ] || echo "note: no OPENROUTER_API_KEY -- floor only" >&2
[ -n "${SERPER_API_KEY:-}" ]     || echo "note: no SERPER_API_KEY -- no web search" >&2

exec ./target/debug/robotd "$@"
