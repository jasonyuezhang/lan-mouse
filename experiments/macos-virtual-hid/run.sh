#!/bin/bash
# Run in Terminal on the Mac connected to the iPad. No persistent installation.
set -euo pipefail
mode=${1:-check}
case "$mode" in check|keyboard|left|right|up|down) ;; *) echo "Unknown test mode"; exit 2;; esac
[[ $# -le 1 ]] || exit 2
[[ $EUID -eq 0 ]] || { echo "Run with sudo: sudo $0 $mode"; exit 2; }
here=$(python3 -c 'import pathlib,sys; print(pathlib.Path(sys.argv[1]).resolve().parent)' "$0")
probe="$here/virtual-hid-probe"
daemon='/Library/Application Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice/Applications/Karabiner-VirtualHIDDevice-Daemon.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Daemon'
[[ -x "$probe" && -x "$daemon" ]] || { echo "Missing helper or installed Karabiner daemon"; exit 1; }
uid=$(stat -f %u /dev/console)
[[ $uid -ne 0 ]] || { echo "Log into the Mac desktop first"; exit 1; }
user_home=$(dscl . -read "/Users/$(stat -f %Su /dev/console)" NFSHomeDirectory | sed 's/^NFSHomeDirectory: //')
agent="gui/$uid/de.feschber.lan-mouse"
plist="$user_home/Library/LaunchAgents/de.feschber.lan-mouse.plist"
restore_agent=0
daemon_pid=''
logfile=$(mktemp /tmp/lan-mouse-hid-daemon.XXXXXX)
cleanup() {
  status=$?
  trap - EXIT
  if [[ -n "$daemon_pid" ]]; then
    kill -TERM "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  if [[ $restore_agent -eq 1 ]]; then
    launchctl bootstrap "gui/$uid" "$plist" || { echo "Restore failed: run launchctl bootstrap gui/$uid '$plist'"; status=1; }
  fi
  echo "Daemon log: $logfile"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
# Stop Lan Mouse during the isolated test so its screen edge does not intercept
# the virtual mouse. Re-load the exact same LaunchAgent on every normal exit.
if launchctl print "$agent" >/dev/null 2>&1; then
  [[ -f "$plist" ]] || { echo "Cannot restore Lan Mouse: LaunchAgent plist missing"; exit 1; }
  launchctl bootout "$agent"
  restore_agent=1
fi
if ! pgrep -f '^/Library/Application Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice/Applications/Karabiner-VirtualHIDDevice-Daemon.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Daemon$' >/dev/null; then
  "$daemon" >"$logfile" 2>&1 &
  daemon_pid=$!
fi
"$probe" "$mode"
