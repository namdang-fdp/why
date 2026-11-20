# Shared helpers for run-claude.sh / run-codex.sh (source, do not execute).

# Build the bubblewrap prefix in the SANDBOX array.
#   sandbox_build <ws> <writable-state-path>...
# Everything is read-only except: the scratch workspace <ws>, a private tmpfs /tmp, and the
# listed harness state paths (needed for auth and transcripts). A write anywhere else (other
# checkouts, the rest of $HOME) fails with "Read-only file system".
# Residual risk: the agent can still READ anything the user can (e.g. ~/.claude/settings.json)
# and can write inside the writable state paths.
sandbox_build() {
  local ws=$1; shift
  SANDBOX=(bwrap --die-with-parent --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp --bind "$ws" "$ws")
  local p
  for p in "$@"; do
    [ -e "$p" ] && SANDBOX+=(--bind "$p" "$p")
  done
  return 0
}

# Print (to stderr) the NAMES of the user-level hooks that may also fire. Keys only, never values.
warn_user_hooks() {
  python3 -I - "$1" <<'PY' >&2
import json, os, sys, tomllib
h = os.path.expanduser("~")
names = []
if sys.argv[1] == "claude":
    # `--setting-sources project,local` should skip these; listed so the owner can judge.
    try:
        d = json.load(open(f"{h}/.claude/settings.json"))
        names += [f"~/.claude/settings.json hooks.{k}" for k in d.get("hooks", {})]
        n = len(d.get("enabledPlugins", {}))
        if n:
            names.append(f"{n} enabled plugin(s) (may bundle hooks)")
    except Exception:
        pass
else:
    try:
        d = json.load(open(f"{h}/.codex/hooks.json"))
        names += [f"~/.codex/hooks.json hooks.{k}" for k in d.get("hooks", d) if k != "description"]
    except Exception:
        pass
    try:
        d = tomllib.load(open(f"{h}/.codex/config.toml", "rb"))
        names += [f"~/.codex/config.toml hooks.{k}" for k in d.get("hooks", {})]
    except Exception:
        pass
print("[agents-capture] user-level hooks that may ALSO fire in this run (names only): "
      + (", ".join(names) if names else "none found"))
PY
}

# Remember the harness version used (save-fixtures.py cross-checks it). Not an agent session.
record_version() {  # record_version <ws> <claude|codex>
  mkdir -p "$1/versions"
  [ -f "$1/versions/$2" ] || "$2" --version > "$1/versions/$2" 2>/dev/null || true
}
