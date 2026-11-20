"""Implementation of log-hook.sh (kept separate so the hook's own stdin is the payload)."""
import json, os, sys, time
label = sys.argv[1] if len(sys.argv) > 1 else "unknown"
raw = sys.stdin.read()
try:
    payload = json.loads(raw)
except Exception:
    payload = {"_unparsed": raw}
# Env capture: names of every variable that looks harness-related, but VALUES only
# for an allowlist of known-harmless ones (others may hold tokens or identities).
SAFE = {"CLAUDE_PROJECT_DIR", "CLAUDE_ENV_FILE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDECODE",
        "CLAUDE_EFFORT", "CLAUDE_CODE_SESSION_ID", "CLAUDE_PLUGIN_DATA", "CODEX_HOME",
        "CODEX_THREAD_ID", "CODEX_SESSION_ID", "CODEX_CI", "CODEX_SANDBOX",
        "CODEX_SANDBOX_NETWORK_DISABLED", "PWD", "OLDPWD"}
env = {k: (v if k in SAFE else "<redacted>") for k, v in sorted(os.environ.items())
       if k.startswith(("CLAUDE", "CODEX", "PLUGIN", "WHY_CAPTURE", "GIT_")) or k in SAFE}
rec = {
    "event": payload.get("hook_event_name") if isinstance(payload, dict) else None,
    "harness": label,
    "argv": sys.argv[1:],
    "env": env,
    "cwd": os.getcwd(),
    "mono_ns": time.monotonic_ns(),
    "wall": time.time(),
    "stdin": payload,
}
log = os.environ.get("WHY_CAPTURE_LOG")
if log:
    with open(log, "a") as f:
        f.write(json.dumps(rec) + "\n")
# WHY_CAPTURE_ONLY=<Event>[,<Event>] limits the three knobs below to those events.
only = os.environ.get("WHY_CAPTURE_ONLY")
if only and rec["event"] not in only.split(","):
    sys.exit(0)
out = os.environ.get("WHY_CAPTURE_STDOUT")
if out:
    sys.stdout.write(out.replace("{event}", str(rec["event"])))
    sys.stdout.flush()
err = os.environ.get("WHY_CAPTURE_STDERR")
if err:
    sys.stderr.write(err.replace("{event}", str(rec["event"])))
    sys.stderr.flush()
s = os.environ.get("WHY_CAPTURE_SLEEP")
if s:
    time.sleep(float(s))
sys.exit(int(os.environ.get("WHY_CAPTURE_EXIT", "0")))
