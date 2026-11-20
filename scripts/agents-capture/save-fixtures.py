#!/usr/bin/env python3
"""Turn raw capture logs into scrubbed, committable fixtures.

Usage: save-fixtures.py [--strict] [--also-scan TEXT]... <ws> <out-root> <repo-path>

  <ws>         scratch dir the run-*.sh scripts wrote to (contains logs/ and fixture-repo)
  <out-root>   src-tauri/tests/fixtures/agents
  <repo-path>  absolute path of the WHY checkout the scripts ran from (it shows up in the
               recorded hook command lines and must not leak)
  --strict     abort if any scenario in the table has no recorded input (default: skip it)
  --also-scan  extra literal text the leak scan must not find (repeatable)

Safety properties (the repository is public):
  * Everything is built in a staging directory first. Nothing under <out-root> is touched
    unless every check passed; then only the scenario folders that were generated are
    replaced (a scenario with no recorded input is skipped, never written as empty files).
  * Harness versions and models come from the recorded data (Claude transcript `version`,
    Codex rollout `cli_version`), never from constants. Missing or mixed versions abort the
    export. <ws>/versions/<harness> (written by the run scripts) is cross-checked if present.
  * Scrubbing: home -> /HOME, scratch dir -> /TMP, WHY checkout -> /REPO; e-mail addresses,
    account/organisation ids and the timezone; values of secret-looking keys (api_key, token,
    password, authorization, cookie, ...) and common token shapes (sk-, ghp_, xox*-, AKIA,
    JWT, private-key blocks, Bearer ...); harness-internal bulk that mirrors the owner's setup
    (Claude `api-request*` / `session-owner` / most attachment bodies; Codex base
    instructions, developer messages, AGENTS.md text, tool catalogues, world_state,
    rate_limits). Dropped or replaced records say so (`redacted`).
  * A final leak scan over everything staged aborts the export (exit 1, nothing written) on
    any hit. It is a backstop, not a review: free-form prompts and tool output of a
    re-recorded session still need a human read before commit.
"""
import getpass
import json
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path

argv, EXTRA_TERMS, args = sys.argv[1:], [], []
STRICT = "--strict" in argv
it = iter(argv)
for a in it:
    if a == "--also-scan":  # extra literal to fail the leak scan on (e.g. the owner's name)
        EXTRA_TERMS.append(next(it))
    elif not a.startswith("--"):
        args.append(a)
if len(args) != 3:
    sys.exit(__doc__)
ws, out_root, repo_path = Path(args[0]).resolve(), Path(args[1]).resolve(), args[2].rstrip("/")
HOME = os.path.expanduser("~")


class Abort(Exception):
    pass


PATH_SUBS = [
    (repo_path, "/REPO"),
    (str(ws / "fixture-repo"), "/TMP/fixture-repo"),
    (str(ws / "fixture-wt"), "/TMP/fixture-wt"),
    (str(ws), "/TMP"),
    ("-tmp-" + ws.name.replace("_", "-") + "-", "-TMP-"),  # ~/.claude/projects/<mangled cwd>
    (os.path.basename(repo_path), "<worktree>"),  # bare checkout name in model prose
    (HOME, "/HOME"),
]
EMAIL = re.compile(r"[\w.+-]+@[\w-]+\.[\w.-]+")
ID_KEYS = {"accountUuid", "organizationUuid", "creator_user_id", "creator_account_id",
           "account_id", "user_id", "userEmail"}
# Keys whose string value is a credential. Deliberately not a bare /token/ match: that would
# also hit `input_tokens`, `token_usage`, ... which are harmless and useful.
SECRET_KEY = re.compile(
    r"(?i)^(api[_-]?key|.*secret.*|passw(or)?d|authorization|(set-)?cookie|bearer|private[_-]?key"
    r"|token|.*_token|access[_-]?token|refresh[_-]?token|id[_-]?token|x-api-key)$")
# Common credential shapes inside free text (tool output, prompts, command lines).
TOKEN_SHAPES = [
    re.compile(r"\bsk-(?:ant-)?[A-Za-z0-9_-]{20,}"),
    re.compile(r"\bgh[pousr]_[A-Za-z0-9]{20,}"),
    re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}"),
    re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{10,}"),
    re.compile(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
    re.compile(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{5,}"),
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)"),
    re.compile(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{20,}"),
]


def scrub_str(s: str) -> str:
    for a, b in PATH_SUBS:
        s = s.replace(a, b)
    s = EMAIL.sub("<email>", s)
    s = re.sub(r"<timezone>[^<]*</timezone>", "<timezone>UTC</timezone>", s)
    for rx in TOKEN_SHAPES:
        s = rx.sub("<redacted-secret>", s)
    return s


def scrub(x):
    if isinstance(x, str):
        return scrub_str(x)
    if isinstance(x, list):
        return [scrub(i) for i in x]
    if isinstance(x, dict):
        out = {}
        for k, v in x.items():
            if k in ID_KEYS:
                v = "<redacted-id>"
            elif isinstance(v, str) and SECRET_KEY.match(k):
                v = "<redacted>"
            else:
                v = scrub(v)
            out[scrub_str(k)] = v
        return out
    return x


def dumps(o) -> str:
    return json.dumps(o, ensure_ascii=False, separators=(",", ":"))


def read_jsonl(p):
    return [json.loads(l) for l in Path(p).read_text().splitlines() if l.strip()]


def write_jsonl(p, rows):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("".join(dumps(scrub(r)) + "\n" for r in rows))


# ---- leak scan: a backstop over the staged output ---------------------------------------

def leak_patterns():
    terms = [HOME, "/home/", str(ws), ws.name, repo_path, os.path.basename(repo_path), *EXTRA_TERMS]
    try:
        terms.append(getpass.getuser())
    except Exception:
        pass
    pats = [re.compile(re.escape(t)) for t in terms if t and len(t) > 3]
    pats.append(EMAIL)
    pats += TOKEN_SHAPES
    pats.append(re.compile(r'"(?:' + "|".join(sorted(ID_KEYS)) + r')":"(?!<redacted-id>)'))
    return pats


def leak_scan(root: Path):
    hits = []
    pats = leak_patterns()
    for f in sorted(root.rglob("*")):
        if not f.is_file():
            continue
        text = f.read_text(errors="replace")
        for rx in pats:
            m = rx.search(text)
            if m:
                hits.append(f"{f.relative_to(root)}: /{rx.pattern[:40]}/ matched {m.group(0)[:30]!r}")
    return hits


# ---- transcript slimming ---------------------------------------------------------------

DROP_CLAUDE = {"api-request", "api-request-shape", "api-request-blob", "session-owner", "atis-latch"}
KEEP_ATTACHMENT = {"environment", "date", "model"}


def slim_claude(rows):
    out = []
    for r in rows:
        t = r.get("type")
        if t in DROP_CLAUDE:
            continue
        if t == "attachment":
            a = r.get("attachment", {})
            if a.get("type") in KEEP_ATTACHMENT:
                r = {k: v for k, v in r.items() if k != "rendered"}
            else:
                r = {**{k: v for k, v in r.items() if k not in ("attachment", "rendered")},
                     "attachment": {"type": a.get("type"), "redacted": "harness-internal context, not part of the protocol"}}
        out.append(r)
    return out


def red(n):
    return f"<redacted: {n} chars of harness/owner context>"


def slim_codex(rows):
    out = []
    for r in rows:
        t, p = r.get("type"), r.get("payload", {})
        if t == "session_meta":
            p = dict(p)
            bi = p.get("base_instructions", {})
            if isinstance(bi, dict) and "text" in bi:
                p["base_instructions"] = {**bi, "text": red(len(bi["text"]))}
            r = {**r, "payload": p}
        elif t == "response_item" and p.get("type") == "additional_tools":
            r = {**r, "payload": {**p, "tools": "<redacted: tool catalogue>"}}
        elif t == "response_item" and p.get("type") == "message":
            blocks = []
            for c in p.get("content", []):
                txt = c.get("text", "")
                if p.get("role") == "developer" or txt.startswith(("# AGENTS.md", "<skills_instructions>")):
                    c = {**c, "text": red(len(txt))}
                blocks.append(c)
            r = {**r, "payload": {**p, "content": blocks}}
        elif t == "event_msg" and p.get("type") == "token_count" and "rate_limits" in p:
            # plan type, usage percentages and reset times describe the owner's account
            r = {**r, "payload": {**p, "rate_limits": "<redacted: account rate limits>"}}
        elif t == "world_state":
            r = {**r, "payload": {"full": p.get("full"), "state": "<redacted: keys " + ",".join(sorted((p.get("state") or {}).keys())) + ">"}}
        out.append(r)
    return out



# Exact prompts, per scenario log name (a list = successive turns of one session).
T_WRITE = "Use the Write tool to create the file src/e.py containing the line E = 1. Then reply with the single word done."
T_PATCH = "Use apply_patch to create src/cxe.py containing the line E = 1, then reply with the single word done."
PROMPTS = {
    "s1-main-basic": [
        'Do these steps in order using tools: 1) Read src/calc.py. 2) Create a new file src/greet.py containing a function greet(name) that returns "Hello, " + name. 3) Edit src/calc.py to add a function subtract(a, b). 4) Run the tests with: python3 -m unittest discover -s tests . Then say done in one sentence.',
        'Now: 1) Use the Edit tool to replace the exact text "DOES_NOT_EXIST_XYZ" with "y" in src/calc.py (it is not there; just try). 2) Use the MultiEdit tool if you have it (otherwise say you lack it) to make two edits in src/greet.py. 3) Edit tests/test_calc.py so test_add asserts add(1, 2) == 4, then run: python3 -m unittest discover -s tests . Report the result in one sentence.  [run with --resume <session-id>]'],
    "s5-failures": ['Call these tools exactly, one after the other, and do not skip any even if you expect an error: (a) Bash with the exact command: python3 -m unittest discover -s tests   (b) Edit on src/calc.py with old_string "NOPE_NOT_PRESENT" and new_string "x" without reading the file first (c) Edit on src/calc.py replacing "return a + b" with "return a + b" is not allowed, so instead Read src/calc.py then Edit with old_string "NOPE_NOT_PRESENT" new_string "x" (d) Bash with the exact command: exit 3 . Then reply with the single word done.'],
    "s2-subagent": ['Use the Agent tool (subagent_type general-purpose) with a prompt telling it to create the file src/sub.py containing one function sub_helper() returning 1 using the Write tool, then to reply "created". After it returns, say done in one sentence.'],
    "s3-worktree": ['Create the file src/wt_note.py containing the line X = 1 using the Write tool, then say done.'],
    "s4-subdir": ['Read calc.py and say how many functions it defines, in one sentence.'],
    "c1-continue": ['What was the last file you were asked to create? One short sentence, no tools.'],
    "s6-write-update-bash": ['Do exactly these tool calls in order: (a) Read src/greet.py, then Write src/greet.py with new content "def greet(name):\\n    return \\"Hi, \\" + name\\n" (overwrite). (b) Bash with the exact command: rm src/old.py && printf "B = 2\\n" > src/bshell.py . Then reply done.  [src/old.py was pre-created by hand]'],
    "e1-stdout": ['First use the Read tool on README.md. Then, without more tools, list every distinct token in your context that starts with MARKER_ (tokens only, comma separated).'],
    "e2-exit1-tool": [T_WRITE], "e2-exit2-pre": [T_WRITE], "e2-exit2-post": [T_WRITE], "e2-exit2-stop": [T_WRITE],
    "e3-timeout": [T_WRITE], "e4-missing": [T_WRITE], "e2-exit2-prompt": [T_WRITE],
    "cx-e2-exit2-prompt-stderr": ["Use apply_patch to create src/cxe2.py containing the line E = 1, then reply with the single word done."],
    "w0-shared-in-main": ["Say ok. No tools."], "w1-shared-main-only": ["Say ok. No tools."],
    "w3-local-main-only": ["Say ok. No tools."],
    "cx1-main-basic": [
        'Do these steps in order, using the apply_patch tool for every file change: 1) create src/notes.py containing the line NOTE = 1. 2) edit src/calc.py to add a function subtract(a, b). 3) delete the file src/sub.py. 4) run the tests with the shell command: python3 -m unittest discover -s tests . 5) run the shell command: sh -c "exit 3" . Then reply with one sentence.',
        'Run the shell command python3 -m unittest discover -s tests and report the result in one sentence. Do not change any files.  [RESUME_ID=<session-id>]'],
    "cx2-subagent": ['Spawn a subagent (use the spawn_agent tool) and ask it to create the file src/cxsub.py containing the line S = 1 via apply_patch, wait for it to finish, then reply done.'],
    "cx3-worktree": ['Use apply_patch to create src/cx_wt.py containing the line W = 1, then reply done.'],
    "cx4-subdir": ['Read calc.py with a shell command and say in one sentence how many functions it defines.'],
    "cx5-failing-patch": ['First use apply_patch to update src/calc.py replacing the line "    return a * 99" with "    return a" (that line does not exist, just try it). Then use a shell command: printf "C = 3\\n" > src/cshell.py . Then reply done.'],
    "cx-e1-stdout": ['First run the shell command: echo hi . Then, without more tools, list every distinct token in your context that starts with MARKER_ (tokens only, comma separated).'],
    "cx-e2-exit1-pre": [T_PATCH], "cx-e2-exit2-pre": [T_PATCH], "cx-e2-exit2-pre-stderr": [T_PATCH], "cx-e2-exit2-post": [T_PATCH],
    "cx-e2-exit2-stop": [T_PATCH],
    "cx-e2-exit2-stop-stderr": ["Reply with the single word done. Use no tools."],
    "cx-e3-timeout": [T_PATCH], "cx-e4-missing": [T_PATCH],
}

# ---- scenario table ---------------------------------------------------------------------

C_ENV = "env -i HOME=$HOME PATH=$PATH"  # what run-claude.sh / run-codex.sh do internally
CLAUDE = [
    # (dir, [log names], transcript?, command, description)
    ("basic-main", ["s1-main-basic"], True,
     "run-claude.sh <ws> s1-main-basic <ws>/fixture-repo '<prompt 1>'; then the same with '<prompt 2>' --resume <session-id>",
     "Turn 1 (fresh session): Read, Write (new file), Edit, Bash (tests pass). Turn 2 (`--resume`, same session id): "
     "ToolSearch for MultiEdit (not available), Edit, Bash (tests fail but the command is wrapped in `; echo exit=$?`, so no PostToolUseFailure)."),
    ("failures", ["s5-failures"], True,
     "run-claude.sh <ws> s5-failures <ws>/fixture-repo '<prompt>'",
     "Bash that exits non-zero (PostToolUseFailure) and Edits the harness rejects before running (no Pre/PostToolUse at all)."),
    ("subagent", ["s2-subagent"], True,
     "run-claude.sh <ws> s2-subagent <ws>/fixture-repo '<prompt>'",
     "Agent tool (general-purpose subagent writes a file). In `-p` mode the subagent was launched asynchronously: the parent's `Stop` fires before the subagent's tool calls."),
    ("write-update-and-bash-edits", ["s6-write-update-bash"], True,
     "run-claude.sh <ws> s6-write-update-bash <ws>/fixture-repo '<prompt>' --include-hook-events",
     "Write over an existing file (tool_response type `update` with originalFile + structuredPatch) and a Bash command that deletes/creates files (the hook payload says nothing about those file changes)."),
    ("worktree", ["s3-worktree"], True,
     "run-claude.sh <ws> s3-worktree <ws>/fixture-wt '<prompt>'",
     "Session started in the linked worktree: Write."),
    ("subdir", ["s4-subdir"], True,
     "run-claude.sh <ws> s4-subdir <ws>/fixture-repo/src '<prompt>'",
     "Session started in a subdirectory of the main checkout: Read."),
    ("continue", ["c1-continue"], False,
     "run-claude.sh <ws> c1-continue <ws>/fixture-repo '<prompt>' --continue",
     "`--continue` picks the most recent session of that cwd; SessionStart `source` is `resume` and the session id is unchanged."),
]
CLAUDE_BEHAVIOR = [
    ("stdout-injection", ["e1-stdout"], "EXTRA_ENV='WHY_CAPTURE_STDOUT=MARKER_{event}' run-claude.sh ... --include-hook-events",
     "Every hook prints MARKER_<event> on stdout. The model was asked to list the markers it sees."),
    ("exit1-on-tool-hooks", ["e2-exit1-tool"], "EXTRA_ENV='WHY_CAPTURE_EXIT=1 WHY_CAPTURE_ONLY=PreToolUse,PostToolUse' run-claude.sh ... --include-hook-events", "Pre/PostToolUse hooks exit 1."),
    ("exit2-on-pretooluse", ["e2-exit2-pre"], "EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=PreToolUse' run-claude.sh ... --include-hook-events", "PreToolUse hook exits 2 with empty stderr."),
    ("exit2-on-posttooluse", ["e2-exit2-post"], "EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=PostToolUse' run-claude.sh ... --include-hook-events", "PostToolUse hook exits 2."),
    ("exit2-on-stop", ["e2-exit2-stop"], "EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=Stop,SessionStart,SessionEnd' run-claude.sh ... --include-hook-events", "Stop/SessionStart/SessionEnd hooks exit 2: the agent is forced to continue. NOTE: in this run the model also ran read-only Bash greps/listings looking for the hook, including `grep -n log-hook` over the owner's ~/.claude/settings.json (output: only `exit=2`); allowedTools Bash is not confined to the temp dir."),
    ("exit2-on-userpromptsubmit", ["e2-exit2-prompt"], "EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=UserPromptSubmit' run-claude.sh ... --include-hook-events", "UserPromptSubmit hook exits 2: the prompt is rejected, the model is never called (num_turns 0)."),
    ("timeout", ["e3-timeout"], "EXTRA_ENV='WHY_CAPTURE_SLEEP=6' SETTINGS=<ws>/to2.json run-claude.sh ... --include-hook-events  (to2.json = claude-settings.py ... 2)", "Hooks sleep 6s with `timeout: 2`."),
    ("missing-binary", ["e4-missing"], "SETTINGS=<ws>/miss.json run-claude.sh ... --include-hook-events  (command: /nonexistent/why-hook claude)", "Hook command does not exist."),
    ("settings-shared-main-only", ["w1-shared-main-only"], "session in <ws>/fixture-wt, untracked <ws>/fixture-repo/.claude/settings.json holds the hooks", "Hooks only in the main checkout's untracked .claude/settings.json. No hooks.jsonl = hooks did NOT fire (result.json shows the run itself succeeded; the `.err` seen during recording was a stale line from an earlier attempt with an invalid --settings path)."),
    ("settings-local-main-only", ["w3-local-main-only"], "session in <ws>/fixture-wt, <ws>/fixture-repo/.claude/settings.local.json holds the hooks", "Hooks only in the main checkout's .claude/settings.local.json: they DO fire for the worktree session."),
    ("settings-shared-in-main", ["w0-shared-in-main"], "control: same shared settings, session in the main checkout", "Control for the two runs above."),
]
CODEX = [
    ("basic-main", ["cx1-main-basic"], True,
     "run-codex.sh <ws> cx1-main-basic <ws>/fixture-repo '<prompt 1>'; then RESUME_ID=<session-id> run-codex.sh <ws> cx1-main-basic <ws>/fixture-repo '<prompt 2>'",
     "Turn 1: apply_patch add / update / delete, shell command (the unittest call ran 0 tests / exit 5 because the prompt had a stray trailing ' .'), failing shell command (sh -c 'exit 3' .). Turn 2 (`codex exec resume`, same session id, new turn id): shell command."),
    ("failing-patch-and-shell-edit", ["cx5-failing-patch"], True, "run-codex.sh <ws> cx5-failing-patch <ws>/fixture-repo '<prompt>'",
     "apply_patch whose context does not match (PreToolUse fires, PostToolUse does NOT) and a shell command that creates a file (hook shows only the command string)."),
    ("subagent", ["cx2-subagent"], True, "run-codex.sh <ws> cx2-subagent <ws>/fixture-repo '<prompt>'",
     "spawn_agent: a subagent applies a patch. Subagent hooks use the parent's session_id plus agent_id; the subagent has its own transcript_path."),
    ("worktree", ["cx3-worktree"], True, "run-codex.sh <ws> cx3-worktree <ws>/fixture-wt '<prompt>'", "Session started in the linked worktree: apply_patch."),
    ("subdir", ["cx4-subdir"], True, "run-codex.sh <ws> cx4-subdir <ws>/fixture-repo/src '<prompt>'", "Session started in a subdirectory: shell command."),
]
CODEX_BEHAVIOR = [
    ("stdout-injection", ["cx-e1-stdout"], "EXTRA_ENV='WHY_CAPTURE_STDOUT=MARKER_{event}' run-codex.sh ...", "Every hook prints MARKER_<event> on stdout."),
    ("exit1-on-pretooluse", ["cx-e2-exit1-pre"], "EXTRA_ENV='WHY_CAPTURE_EXIT=1 WHY_CAPTURE_ONLY=PreToolUse' run-codex.sh ...", "PreToolUse hook exits 1."),
    ("exit2-on-pretooluse-no-stderr", ["cx-e2-exit2-pre"], "EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=PreToolUse' run-codex.sh ...", "PreToolUse exits 2 with empty stderr: NOT blocked."),
    ("exit2-on-pretooluse-with-stderr", ["cx-e2-exit2-pre-stderr"], "EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=PreToolUse WHY_CAPTURE_STDERR=BLOCKED_BY_HOOK_MARKER' run-codex.sh ...", "PreToolUse exits 2 with a reason on stderr: blocked."),
    ("exit2-on-posttooluse", ["cx-e2-exit2-post"], "EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=PostToolUse' run-codex.sh ...", "PostToolUse exits 2."),
    ("exit2-on-stop-no-stderr", ["cx-e2-exit2-stop"], "EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=Stop' RUN_TIMEOUT=100 run-codex.sh ...", "Stop exits 2 with empty stderr: one Stop, no continuation, rc 0 (compare with the stderr variant)."),
    ("exit2-on-stop-with-stderr", ["cx-e2-exit2-stop-stderr"], "EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=Stop WHY_CAPTURE_STDERR=KEEP_GOING_MARKER' RUN_TIMEOUT=100 run-codex.sh ...", "Stop exits 2 with stderr: the turn is continued again and again until the 100s wrapper timeout killed it (rc=124)."),
    ("exit2-on-userpromptsubmit-with-stderr", ["cx-e2-exit2-prompt-stderr"], "EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=UserPromptSubmit WHY_CAPTURE_STDERR=PROMPT_BLOCKED_MARKER' run-codex.sh ...", "UserPromptSubmit exits 2 with stderr: the turn completes with zero model usage, no file created."),
    ("timeout", ["cx-e3-timeout"], "EXTRA_ENV='WHY_CAPTURE_SLEEP=6' HOOK_TIMEOUT=2 run-codex.sh ...", "Hooks sleep 6s with `timeout = 2`."),
    ("missing-binary", ["cx-e4-missing"], "HOOK_CMD=/nonexistent/why-hook run-codex.sh ...  (hooks.jsonl is absent: nothing ran)", "Hook command does not exist."),
]




# ---- staging and export -----------------------------------------------------------------

def readme(path, harness, version, model, command, desc, logs=()):
    ptxt = "".join(f"- Prompt{' (turn %d)' % (i + 1) if len(PROMPTS.get(n, [])) > 1 else ''} for `{n}`: `{pr}`\n"
                   for n in logs for i, pr in enumerate(PROMPTS.get(n, [])))
    path.write_text(
        f"# {path.parent.name}\n\n"
        f"- Harness: {harness} {version}\n- Model: {model}\n"
        f"- Command: `{command}`\n  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir "
        f"holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)\n"
        f"{ptxt}"
        f"- What happened: {desc}\n"
        f"- Files: `hooks.jsonl` = one record per hook invocation, in arrival order "
        f"(`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). "
        f"Other files are described in the harness README.\n"
        f"- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.\n")


def claude_extra(base, logs):
    for n in logs:
        res = []
        op = ws / "logs" / f"{n}.out.jsonl"
        for d in (read_jsonl(op) if op.exists() else []):
            if d.get("subtype") == "hook_response":
                res.append({k: d.get(k) for k in ("hook_event", "hook_name", "exit_code", "outcome", "stdout", "stderr")})
            if d.get("type") == "result":
                base_res = {k: d.get(k) for k in ("result", "session_id", "duration_ms", "num_turns", "is_error")}
                (base / "result.json").write_text(dumps(scrub(base_res)) + "\n")
        if res:
            write_jsonl(base / "hook-results.jsonl", res)


def transcripts(hooks_rows):
    paths = []
    for r in hooks_rows:
        for k in ("transcript_path", "agent_transcript_path"):
            p = r["stdin"].get(k)
            if p and p not in paths and os.path.exists(p):
                paths.append(p)
    return paths


def recorded(logs):
    return any((ws / "logs" / f"{n}{suffix}").exists() for n in logs for suffix in (".jsonl", ".out.jsonl"))


def stage(harness, stage_root, table, behavior, meta):
    """Write scenario folders (without READMEs) under stage_root; return [(dir, readme args)]."""
    jobs = []
    entries = [(a, b, c, d, e, False) for a, b, c, d, e in table] + \
              [(a, b, False, c, d, True) for a, b, c, d in behavior]
    for name, logs, with_tr, cmd, desc, is_behavior in entries:
        if not recorded(logs):
            msg = f"{harness}/{name}: no recorded input ({', '.join(logs)})"
            if STRICT:
                raise Abort(msg)
            print("skip", msg)
            continue
        base = stage_root / ("behavior/" + name if is_behavior else name)
        base.mkdir(parents=True, exist_ok=True)
        rows = [r for n in logs if (ws / "logs" / f"{n}.jsonl").exists()
                for r in read_jsonl(ws / "logs" / f"{n}.jsonl")]
        if rows:
            write_jsonl(base / "hooks.jsonl", rows)
        for r in rows:
            if r["stdin"].get("model"):
                meta["models"].add(r["stdin"]["model"])
        if with_tr:
            paths = transcripts(rows)
            if not paths:
                raise Abort(f"{harness}/{name}: transcript expected but no readable transcript_path")
            for i, p in enumerate(paths):
                src = read_jsonl(p)
                kind = "subagent-transcript" if "/subagents/" in p else ("transcript" if i == 0 else f"transcript-{i}")
                write_jsonl(base / f"{kind}.jsonl", (slim_claude if harness == "claude" else slim_codex)(src))
                for rec in src:
                    if harness == "claude":
                        if rec.get("version"):
                            meta["versions"].add(rec["version"])
                        m = rec.get("message")
                        if rec.get("type") == "assistant" and isinstance(m, dict) and m.get("model"):
                            meta["models"].add(m["model"])
                    elif rec.get("type") == "session_meta" and rec["payload"].get("cli_version"):
                        meta["versions"].add(rec["payload"]["cli_version"])
                if harness == "claude" and "/subagents/" in p:
                    mp = Path(p).with_suffix(".meta.json")
                    if mp.exists():
                        (base / "subagent-transcript.meta.json").write_text(dumps(scrub(json.loads(mp.read_text()))) + "\n")
        if harness == "claude":
            claude_extra(base, logs)
        elif is_behavior:
            op = ws / "logs" / f"{logs[0]}.out.jsonl"
            if not op.exists():
                raise Abort(f"codex/{name}: missing {op.name}")
            write_jsonl(base / "exec-events.jsonl",
                        [d for d in read_jsonl(op) if "bypass-hook-trust" not in json.dumps(d)])
        jobs.append((base, (cmd, desc, logs)))
    return jobs


def stage_harness(harness, label, table, behavior, tmp):
    meta = {"versions": set(), "models": set()}
    root = tmp / harness
    jobs = stage(harness, root, table, behavior, meta)
    if not jobs:
        print(f"{harness}: nothing recorded, skipped")
        return None
    if len(meta["versions"]) != 1:
        raise Abort(f"{harness}: need exactly one harness version in the recorded transcripts, found {sorted(meta['versions'])}")
    version = next(iter(meta["versions"]))
    vf = ws / "versions" / harness
    if vf.exists() and version not in vf.read_text():
        raise Abort(f"{harness}: recorded version {version} not found in {vf} ({vf.read_text().strip()!r})")
    model = ", ".join(sorted(meta["models"])) or "unknown"
    for base, (cmd, desc, logs) in jobs:
        readme(base / "README.md", label, version, model, cmd, desc, logs)
    return version, root, [b for b, _ in jobs]


def main():
    out_root.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp(prefix="agents-fixtures-"))
    try:
        results = []
        for harness, label, table, behavior in (("claude", "Claude Code", CLAUDE, CLAUDE_BEHAVIOR),
                                                ("codex", "Codex", CODEX, CODEX_BEHAVIOR)):
            r = stage_harness(harness, label, table, behavior, tmp)
            if r:
                results.append((harness, *r))
        if not results:
            raise Abort("no scenario had recorded input")
        hits = leak_scan(tmp)
        if hits:
            raise Abort("leak scan hit(s), nothing written:\n  " + "\n  ".join(hits))
        # swap in: replace only the scenario folders that were generated
        for harness, version, root, dirs in results:
            for d in dirs:
                dest = out_root / harness / version / d.relative_to(root)
                if dest.exists():
                    shutil.rmtree(dest)
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.move(str(d), str(dest))
            print(f"{harness} {version}: wrote {len(dirs)} scenario folders")
    except Abort as e:
        print("ABORT:", e, file=sys.stderr)
        sys.exit(1)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


main()
