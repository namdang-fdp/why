#!/usr/bin/env bash
# Command-level proof (NO agent session) that the bwrap line used by run-claude.sh/run-codex.sh
# blocks writes outside the scratch workspace. Usage: sandbox-check.sh [other-dir-to-probe]
set -u
here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"
ws=$(mktemp -d)
other=${1:-$here}   # default probe target: this checkout
sandbox_build "$ws" "$HOME/.claude" "$HOME/.codex"
run() { "${SANDBOX[@]}" sh -c "$1"; }
bad=0
run "touch '$ws/inside.txt'" && echo "PASS write inside scratch ws allowed" || { echo "FAIL inside ws"; bad=1; }
if run "touch '$other/outside-probe'" 2>/dev/null; then echo "FAIL write to $other allowed"; bad=1; else echo "PASS write to $other denied"; fi
if run "touch \"\$HOME/outside-probe\"" 2>/dev/null; then echo "FAIL write to HOME allowed"; bad=1; else echo "PASS write to HOME denied"; fi
if run "touch /usr/outside-probe" 2>/dev/null; then echo "FAIL write to /usr allowed"; bad=1; else echo "PASS write to /usr denied"; fi
run "test -w \"\$HOME/.claude\"" && echo "NOTE ~/.claude is writable by design (auth + transcripts); not probed by writing"
# the host must not have gained any probe file
for f in "$other/outside-probe" "$HOME/outside-probe"; do
  [ -e "$f" ] && { echo "FAIL probe file exists on host: $f"; bad=1; }
done
rm -f "$ws/inside.txt"
rmdir "$ws"
exit $bad
