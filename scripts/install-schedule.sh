#!/usr/bin/env bash
#
# Install (or remove) the daily off-site backup as a launchd agent.
#
#   scripts/install-schedule.sh            install / reinstall
#   scripts/install-schedule.sh --remove   uninstall
#   scripts/install-schedule.sh --status   is it loaded, when did it last run
#
# launchd was chosen over cron because it survives reboots, runs missed jobs
# after a wake, and is the supported mechanism on macOS.

set -euo pipefail

LABEL="co.labs.bender.backup"
PROJECT="$(cd "$(dirname "$0")/.." && pwd)"
PLIST_SRC="$PROJECT/scripts/$LABEL.plist"
PLIST_DST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"

case "${1:-install}" in
--remove)
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
    rm -f "$PLIST_DST"
    echo "removed: $LABEL"
    exit 0
    ;;
--status)
    if launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; then
        echo "loaded: $LABEL"
        launchctl print "$DOMAIN/$LABEL" | grep -E "state|last exit code|runs" | sed 's/^/  /'
    else
        echo "not loaded"
    fi
    echo "--- last log lines ---"
    tail -n 12 "$HOME/Library/Logs/bender-backup.log" 2>/dev/null || echo "  (no log yet)"
    exit 0
    ;;
esac

# a release binary means launchd never needs cargo on its PATH
if [ ! -x "$PROJECT/target/release/robotd" ]; then
    echo "==> building the release binary (launchd has no cargo)"
    (cd "$PROJECT" && cargo build --release -p robotd)
fi

mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
sed -e "s|__PROJECT__|$PROJECT|g" -e "s|__HOME__|$HOME|g" "$PLIST_SRC" > "$PLIST_DST"

launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$PLIST_DST"

echo "installed: $LABEL"
echo "  runs daily at 03:20 (and after wake if the mac was asleep)"
echo "  log:     ~/Library/Logs/bender-backup.log"
echo "  status:  scripts/install-schedule.sh --status"
echo "  run now: launchctl kickstart -k $DOMAIN/$LABEL"
echo "  remove:  scripts/install-schedule.sh --remove"
