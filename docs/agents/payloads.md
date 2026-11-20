# What Claude Code and Codex really emit (WHY-45 spike)

Recorded on 2026-11-20 with **Claude Code 2.1.322** (model `claude-haiku-5-5`, `-p` headless) and **Codex 0.170.0** (`codex exec`, model `gpt-6-luna`, effort low) on Linux. Everything below was observed on those binaries unless marked *(docs)* (taken from the vendor docs, not reproduced) or *(not observed)*.

Fixtures: `src-tauri/tests/fixtures/agents/{claude/2.1.322,codex/0.170.0}/<scenario>/`. Each scenario has a `README.md` with the exact command and prompt, `hooks.jsonl` (every hook payload, in arrival order) and, where relevant, the harness's own transcript. Re-record with `scripts/agents-capture/` (see its README). The repo is public, so fixtures are scrubbed (`/HOME`, `/TMP`, `/REPO`, ids, owner-specific instruction text dropped); the transcripts are therefore *slimmed copies*, see "What was scrubbed" at the end.

Doc sources: <https://code.claude.com/docs/en/hooks>; the Codex hook docs moved from `developers.openai.com/codex/hooks` to `learn.chatgpt.com/docs/hooks` (308 redirect).

## 1. Hooks: how they are wired

| | Claude Code 2.1.322 | Codex 0.170.0 |
|---|---|---|
| Config sources | `~/.claude/settings.json`, `.claude/settings.json`, `.claude/settings.local.json`, managed, plugins, `--settings <file>` | `~/.codex/hooks.json`, `[hooks]` in `config.toml`, trusted-project `.codex/hooks.json`, `/etc/codex/hooks.json`, plugins |
| Per-run injection (no config edits) | `claude --settings <file>`; `--setting-sources project,local` skips the user's own hooks/plugins | `-c 'hooks.<Event>=[{hooks=[{type="command",command="..."}]}]'` works (user-layer, no trust prompt needed with `--dangerously-bypass-hook-trust`) |
| Project-level file | works headless (`-p` treats folder as trusted) | **ignored until the project is trusted**; `-c projects."<p>".trust_level="trusted"` did **not** enable it; `codex exec --trust` persists to `~/.codex/config.toml`, so I did not test it |
| Handler | `{"type":"command","command":...,"timeout":s}` (also http, mcp_tool, prompt, agent) | `{"type":"command","command":...,"timeout":s}` (also mcp_tool) |
| Default timeout *(docs)* | 600 s (30 s on UserPromptSubmit; SessionEnd shares 1.5 s) | 600 s (SessionEnd/Interrupt 1 s, max 3) |
| Hook process cwd | the session cwd (a subdirectory stays the subdirectory) | the session cwd |
| Hook env | inherits the parent env, plus `CLAUDE_PROJECT_DIR` (= session start cwd, **not** the git root), `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDECODE=1`, `CLAUDE_ENV_FILE` (SessionStart only) | inherits the parent env; **no Codex session/thread variable at all** (only `CODEX_MANAGED_*` set by the npm wrapper). Identity comes only from stdin |
| Parallel hooks | all hooks of one event run in parallel *(docs)* | not tested |

Hooks are spawned as one process per event, so a hook binary pays process start-up every time (my python hook: ~45-55 ms per Claude event, from `stop_hook_summary.durationMs`).

## 2. Claude Code payloads

Common to every event: `session_id`, `transcript_path`, `cwd`, `hook_event_name`. Added on most: `prompt_id` (the "turn" id, a UUID that groups UserPromptSubmit → tools → Stop of one user prompt; also on SessionEnd), `permission_mode`, `effort` `{level}`. **There is no `model` field in any Claude hook payload**; the model is only in the transcript.

Events seen (all in `-p` mode): SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, PostToolBatch, SubagentStart, SubagentStop, Stop, SessionEnd.

| Event | Extra fields observed |
|---|---|
| SessionStart | `source` = `startup` \| `resume` (also `--continue`); on resume `seconds_since_last_response`, `context_tokens`, `prompt_cache_likely_expired`, `estimated_cache_write_usd`. No `prompt_id`, no `model`. |
| UserPromptSubmit | `prompt` (full text) |
| PreToolUse | `tool_name`, `tool_input`, `tool_use_id` |
| PostToolUse | the same + `tool_response`, `duration_ms` |
| PostToolUseFailure | `tool_name`, `tool_input`, `tool_use_id`, `error` (string, first line `Exit code N`), `is_interrupt`, `duration_ms` |
| PostToolBatch | `tool_calls[]` = `{tool_name, tool_input, tool_use_id, tool_response}` where `tool_response` is the **string the model saw** |
| SubagentStart | `agent_id`, `agent_type` |
| SubagentStop | `agent_id`, `agent_type`, `agent_transcript_path`, `last_assistant_message`, `background_tasks`, `stop_hook_active` |
| Stop | `last_assistant_message`, `stop_hook_active`, `background_tasks[]`, `session_crons[]` |
| SessionEnd | `reason` (`other` in `-p`) |

Subagent activity: while a subagent runs, its tool events carry the **same `session_id`/`transcript_path` as the parent plus `agent_id` and `agent_type`** (including PreToolUse, PostToolUse and PostToolBatch). The parent's own events have no `agent_id`.

### Tool payloads (what WHY cares about: file edits)

| Tool | `tool_input` | `tool_response` (PostToolUse) |
|---|---|---|
| Write (new file) | `file_path` (absolute), `content` | `{type:"create", filePath, content, structuredPatch:[], originalFile:null, userModified:false}` |
| Write (overwrite) | same | `{type:"update", filePath, content, structuredPatch:[{oldStart,oldLines,newStart,newLines,lines:[" ctx","-old","+new"]}], originalFile:"<whole old content>", userModified}` |
| Edit | `file_path`, `old_string`, `new_string`, `replace_all` | `{filePath, oldString, newString, originalFile, structuredPatch:[…hunks…], userModified, replaceAll}` |
| Bash | `command`, `description` | `{stdout, stderr, interrupted, isImage, noOutputExpected}` (no exit code on success) |
| Read | `file_path` | `{type:"text", file:{filePath, content, numLines, startLine, totalLines}}` |
| Agent | `prompt`, `description`, `subagent_type` | in `-p` mode: `{isAsync:true, status:"async_launched", agentId, …}` (see below) |
| MultiEdit | **does not exist** in 2.1.322 (model searched via ToolSearch, no match) | – |

Facts that matter:

1. **Write/Edit PostToolUse already carries `originalFile` and a `structuredPatch` (unified-diff hunks).** An edit can be reconstructed from the hook alone, without reading the transcript or git.
2. **File changes made through Bash are invisible**: `rm old.py && printf … > new.py` produces a PostToolUse with only `stdout/stderr`; no `bashEditDiff` field appeared (the docs mention one for ≥2.1.269; not seen here). Git is the only source for those.
3. **Failed Bash** = `PostToolUseFailure` (not PostToolUse) with `error: "Exit code 1\n<output>"`. **A failing test wrapped as `cmd; echo exit=$?` is a *successful* PostToolUse.** Do not infer failure from output text.
4. **An Edit the harness rejects before running** (`String to replace not found`, edit without prior Read) fires **no PreToolUse, PostToolUse or PostToolUseFailure at all**. The only trace is the `PostToolBatch` entry whose `tool_response` starts with `<tool_use_error>`, plus the transcript.
5. Parallel tool calls interleave and finish out of order (`Pre ToolSearch, Pre Read, Post Read, Post ToolSearch`). Pair Pre/Post by `tool_use_id`, never by position.
6. In `-p` mode the Agent tool launched the subagent **asynchronously** even though its `tool_input` has no `run_in_background` (only `description`, `prompt`, `subagent_type`), so the harness chose it; whether interactive runs differ is *not observed*: PostToolUse(Agent) returned `async_launched`, the parent's `Stop` fired *before* the subagent's own tool calls, then `SubagentStop`, then a synthetic `UserPromptSubmit` whose `prompt` is a `<task-notification>…` block, then a second `Stop`. So a "turn" can contain a UserPromptSubmit that the human did not type. Interactive behaviour *(not observed)*.
7. File order in `hooks.jsonl` is arrival order; across 179 consecutive same-session record pairs in all fixtures the monotonic timestamps never went backwards, but that is a property of my runs, not a guarantee (each hook is a separate process).

### Sessions, resume, locations

- `--resume <id>` and `--continue` keep the **same `session_id` and the same transcript file**; a new `SessionStart` fires with `source:"resume"`. Each `claude -p` invocation ends with its own `SessionEnd` (so SessionEnd ≠ "conversation over").
- Transcript: `~/.claude/projects/<cwd with / → ->/<session_id>.jsonl`, written as the session runs. For a subagent: `…/<session_id>/subagents/agent-<agent_id>.jsonl` (+ `.meta.json`), path given in `SubagentStop.agent_transcript_path`.
- Directory name = cwd mangled, so a session in a subdirectory or a linked worktree is stored under **that** cwd's directory, not the repo root's.

### Transcript format (Claude)

JSONL, one record per line, `type` ∈ `user`, `assistant`, `system` (`stop_hook_summary`), `attachment`, `queue-operation`, `last-prompt`, `tools-used`, `mode`, `cost-state`, plus bulky `api-request*` records (dropped from fixtures). Conversation records chain by `parentUuid`/`uuid` and carry `sessionId`, `cwd`, `gitBranch`, `version`, `timestamp`, `promptId` (= hook `prompt_id`).

- `assistant.message.content[]` blocks: `tool_use{id,name,input}`, `text`, `thinking`. **`thinking` text is empty** (only a `signature`), so the model's reasoning is not recoverable. With this model the visible "why" is the final `text`, the Bash `description` field and the user prompt.
- The tool result is a `user` record with `message.content[0].type == "tool_result"` and a structured `toolUseResult` (same object as the hook's `tool_response`). `tool_use.id` == hook `tool_use_id`.

## 3. Codex payloads

Common: `session_id`, `transcript_path`, `cwd`, `hook_event_name`, `model`. Turn-scoped events add `turn_id`; most add `permission_mode`.

Events seen: SessionStart, UserPromptSubmit, PreModelRequest, PostModelRequest, PreToolUse, PostToolUse, Stop, SessionEnd, SubagentStart, SubagentStop. Registered but never fired in my scenarios: PermissionRequest (nothing needed approval under `workspace-write`), PreCompact/PostCompact, Interrupt.

| Event | Extra fields observed |
|---|---|
| SessionStart | `source` = `startup` \| `resume`; no `turn_id` |
| UserPromptSubmit | `turn_id`, `prompt` |
| Pre/PostModelRequest | `turn_id`; Post adds `status` (`completed`), `error` (null), `token_usage{input,cached_input,output,reasoning,total}`. Fires **before/after every model call** (one pair per assistant step), very chatty |
| PreToolUse | `turn_id`, `tool_name`, `tool_use_id`, `tool_input` |
| PostToolUse | the same + `tool_response` (a **string**) |
| Stop | `turn_id`, `last_assistant_message`, `stop_hook_active` |
| SubagentStart/Stop | `turn_id`, `agent_id`, `agent_type` (`default`); Stop adds `agent_transcript_path`, `last_assistant_message` |
| SessionEnd | `reason` (`other`) |

### Tool names and apply_patch

| What the model did | `tool_name` | `tool_input` | `tool_response` |
|---|---|---|---|
| shell (`exec_command`) | `Bash` | `{command}` | the raw output string only. **No exit code**: `sh -c "exit 3"` → `""`; a failing `unittest` → just its traceback text |
| file edit | `apply_patch` | `{command: "*** Begin Patch\n*** Add File: <abs path>\n+line\n*** End Patch"}` (also `*** Update File:` with `@@` hunks, `*** Delete File:`) | `"Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated the following files:\nA <path>\n"` |
| spawn subagent | `collaborationspawn_agent` (sic, no separator) | `{task_name, fork_turns, message: "gAAAA…"}`; **`message` is encrypted** | – |
| wait | `collaborationwait_agent` | `{timeout_ms}` | – |

Facts that matter:

1. **`apply_patch` content *is* exposed in the hook** (`tool_input.command` holds the whole patch, with absolute paths on Add/Update/Delete). The transcript has it too, but nested inside JavaScript (item 5 below), so the hook is the cleaner source. A patch gives you the *hunk*, not the full old file (no `originalFile` equivalent).
2. **A patch the harness rejects** (context did not match) fires PreToolUse but **no PostToolUse**. Pair Pre/Post by `tool_use_id`. A Pre with no Post means the **outcome is unknown**: it can be a rejected patch (the one case seen) *or* a lost delivery (collector down under fail-open, hook timeout/crash); only another source (rollout output, `git`) can tell.
3. Hooks exist for shell, apply_patch, MCP and function tools; `tool_name` for an edit is `apply_patch` (docs: also matchable as `Edit`/`Write`).
4. Shell failures are **not distinguishable in the hook** (no exit code, no failure event; verified for `exit 3` and a failing unittest). The `exec --json` stream carries `exit_code` (`command_execution` items); the rollout's `custom_tool_call_output` was not checked for it.
5. **The rollout records tool use as a JavaScript wrapper.** In 0.170 the model calls one tool, `exec`, with JS source such as `await tools.apply_patch("*** Begin Patch…")` or `await tools.exec_command({cmd:…})`; the rollout line is `{"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"<JS>"}}` and the output is `custom_tool_call_output`. Parsing edits out of the rollout means parsing that JS string. Hook payloads are normalised and do not have this problem.
6. `Stop.last_assistant_message` and the transcript's `assistant` messages give the narration, e.g. "I'll inspect the repository instructions and target files, then apply each requested file change…": this is the closest thing to a declared rationale (Codex narrates before acting; Claude with this model mostly did not).
7. Subagents: hooks use the **parent's `session_id`**, plus `agent_id` (= the subagent's own thread id) and a different `transcript_path`. The subagent has its own rollout whose `session_meta` has `id` = agent id, `parent_thread_id`, `agent_path:"/root/create_file"`, `thread_source:"subagent"`.

### Sessions, resume, locations

- `codex exec`'s `thread.started.thread_id` == hook `session_id` == rollout `session_meta.id`.
- `codex exec resume <id>` keeps the **same `session_id` and rollout file**; SessionStart `source:"resume"`; each turn gets a new `turn_id`; SessionEnd fires per process (same as Claude).
- Transcript: `~/.codex/sessions/YYYY/MM/DD/rollout-<local-timestamp>-<session_id>.jsonl`, given in every payload as `transcript_path`. **JSONL is the source of truth**: `~/.codex/thread_history_1.sqlite` (tables `thread_turns`, `thread_items`, `thread_history_projection_state`) is a derived projection (`next_rollout_byte_offset` into the rollout). Only my own thread's rows were queried.
- Rollout record types: `session_meta` (cwd, cli_version, `git:{commit_hash,branch}`, model_provider, `base_instructions`), `turn_context`, `world_state`, `response_item` (`message`, `custom_tool_call`, `custom_tool_call_output`, `additional_tools`), `event_msg` (`task_started`, `item_completed`, `token_count`, `task_complete`), `token_usage_record`. Records carry `ordinal` and `timestamp`.
- `session_meta` and the first developer/user messages contain **account ids and the owner's own AGENTS.md text** (also tool catalogues of every configured MCP server). Treat the raw rollout as sensitive; the fixtures redact those parts.

## 4. Behaviour experiments (drive WHY-13 fail-open)

All results from `behavior/` fixtures. "Proceeds" = the agent carried on and the task finished.

### Does hook stdout reach the model?

| | Claude Code | Codex |
|---|---|---|
| SessionStart, UserPromptSubmit | **yes** (model listed exactly `MARKER_SessionStart, MARKER_UserPromptSubmit`) | **yes** (same two) |
| PreToolUse, PostToolUse, PostToolBatch, Stop | no (plain stdout ignored) | no |
| Shown to the user | `hook_response` stream event for every event (with `--include-hook-events`) | nothing in `exec --json` |

So a hook that prints anything on SessionStart/UserPromptSubmit **pollutes the agent's context**. `why-hook` must print nothing.

### Non-zero exits

| Hook exit | Claude Code | Codex |
|---|---|---|
| 1 on Pre/PostToolUse | tool **proceeds**; `hook_response` outcome `error`; no visible effect on the task | PreToolUse only tested: tool proceeds; nothing surfaced in `exec --json` |
| 2, empty stderr, PreToolUse | **blocks the tool**; the model is told "hook errored" and, in my run, refused to work around it and gave up | **does NOT block** (patch applied) |
| 2 + stderr text, PreToolUse | blocks *(docs)* | **blocks** (file not created) |
| 2 on PostToolUse | tool already ran; model is told the hook reported a blocking error, continues | empty stderr only tested: no effect (patch applied, run finished) |
| 2 on UserPromptSubmit | **prompt is rejected** (`num_turns: 0`, "operation blocked by hook"), nothing sent to the model | with stderr text: **prompt rejected**, turn completes with zero usage; without stderr *(not observed)* |
| 2 on Stop | agent is **forced to continue**: 20 Stop hooks in one run (interleaved with new tool calls), run took 29 s and ended with an empty result *(docs: a cap of 8 consecutive blocks, `CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`)* | with stderr text: **endless loop**; killed by my 100 s wrapper (47 Stops, 47 "done" messages). With **empty stderr: no loop** (one Stop, rc 0) |

Note on the Claude exit-2-on-Stop run: the model went looking for the hook and ran read-only Bash greps/listings, including over the owner's `~/.claude/settings.json` (only `exit=2` came back); `--allowedTools Bash` is not confined to the temp dir.

Take-away: **a crashing hook that accidentally exits 2 can block tools (Claude) or trap the agent in a Stop loop (both)**. Exit 0 always, and never exit 2 from a panic path. A bare Rust panic exits 101, which is non-blocking in both: but verify with the real binary.

### Timeout and missing binary

| | Claude Code | Codex |
|---|---|---|
| Hook sleeps 6 s with `timeout: 2` | hook is **cancelled** (`outcome: cancelled`), agent proceeds; total run 12.6 s for 6 events × 2 s: **every slow hook adds its full timeout to the latency of that event** | same: run took 28 s vs an 8 s baseline; agent proceeds |
| Command not found | exit **127** + `/bin/sh: …: No such file or directory`, outcome `error`, **non-blocking**, task completes | **silent**: no event, no warning in `exec --json`, task completes |

### Where hooks are looked up (git worktrees)

| Setup | Claude Code | Codex |
|---|---|---|
| Hooks in the main checkout's shared `.claude/settings.json`, session in the linked worktree | **do NOT fire** (in my test the file was untracked, so absent from the worktree; a tracked file would simply be checked out there; the run itself succeeded, the log is simply absent) | *(not observed, see below)* |
| Hooks in the main checkout's `.claude/settings.local.json`, session in the linked worktree | **fire** (docs: local settings are read from the main checkout root) | *(not observed)* |
| User-level (`~/.claude/settings.json`, `~/.codex/hooks.json`) | fire everywhere *(docs)* | fire everywhere, also in untrusted projects *(docs)* |
| Project-level file | – | untrusted project → ignored; trusting needs a write to `~/.codex/config.toml`, which this spike was not allowed to do. Docs: "in a linked Git worktree, a project layer can read hooks from the corresponding directory in the main checkout" |

→ **User-level installation is the only mechanism that is uniform for both harnesses and for every worktree.** That matches the plan for `why-hook`.

## 5. Ordering and ids

- Claude: `session_id` is stable across resume/continue; `prompt_id` identifies the user turn; `tool_use_id` pairs Pre/Post and equals the transcript's `tool_use.id`; `agent_id` marks subagent events.
- Codex: `session_id` stable across resume; `turn_id` identifies the turn; `tool_use_id` pairs Pre/Post; `agent_id` marks subagent events; hook `session_id` is the *root* session for subagents.
- Within a turn, events arrive in causal order for sequential tool calls; for parallel calls Pre/Post interleave.
- `(session_id, prompt_id | turn_id, tool_use_id)` is a **correlation key** that ties a PreToolUse to its PostToolUse (and PostToolBatch entry); the two share it, so it must **not** be used to deduplicate deliveries. A dedupe key must also include `hook_event_name` (and, for events without a tool id, `prompt_id`/`turn_id`). No true duplicate delivery of the same event was observed in these runs, so what one would look like is *not observed*; a synthetic `UserPromptSubmit` (Claude, subagent notification) is a distinct event with its own `prompt_id`, not a duplicate.

## 6. Not observed / open questions

**Caveats on the experiments themselves**
- The runs were made on the owner's machine with the owner's own user-level config still present. Claude: `--setting-sources project,local` skipped `~/.claude/settings.json` hooks and plugins (every recorded `stop_hook_summary` had `hookCount: 1`, our hook only). Codex: `CODEX_HOME` could not be isolated without copying credentials, so any user-level hooks in `~/.codex/` would also have fired (`~/.codex/hooks.json` had none at the time; `~/.codex/config.toml` has a `[hooks.state]` table, probably where `/hooks` trust decisions are persisted; its values were not read, and it is not a hook definition; the run scripts now print the names of user hook tables before each run). Such extra hooks could in principle have affected the stdout-injection and timing results; the fixtures contain only our capture hook's log, so the payloads themselves stay valid.
- The capture scripts now run agents under bubblewrap with a narrowed `--allowedTools`, but the recorded fixtures were made **before** that (plain `env -i`, blanket `Bash`), which is how the `exit2-on-stop` model came to grep the owner's `~/.claude/settings.json`. The new containment was proven with a command-level check only (`scripts/agents-capture/sandbox-check.sh`), not with a fresh agent run.

**Not observed**

- Interactive-only events (Claude: Notification, PermissionRequest, PermissionDenied, PreCompact/PostCompact, TeammateIdle…; Codex: PermissionRequest, PreCompact/PostCompact, Interrupt). Claude's `-p` runs also never emitted `InstructionsLoaded`/`CwdChanged` though they were registered (`Setup`, `FileChanged`, `WorktreeCreate/Remove` were deliberately not registered: the last two replace git's own behaviour).
- Compaction and `/clear`: `source: compact|clear` on SessionStart; never produced.
- Codex project-level `.codex/hooks.json` and `[hooks]` in project `config.toml`, including worktrees (needs project trust persisted to `~/.codex/config.toml`).
- Codex trust-store for `/hooks` approvals: docs do not say; `~/.codex/config.toml` has a `[hooks.state]` table (key name only, values not read) that is probably it. I used `--dangerously-bypass-hook-trust`. A real install must go through the trust flow: **a changed hook binary hash needs re-trust** *(docs)*.
- Codex non-0/2 exit codes beyond 1; Codex UserPromptSubmit exit 2 *without* stderr; Codex PostToolUse exit 1 / exit 2 *with* stderr. (Pattern seen so far: Codex honours exit 2 only when stderr carries a reason, for PreToolUse and Stop.)
- Claude PreToolUse/PostToolUse for MCP tools, Notebook edits, and `MultiEdit` (absent).
- Whether `exec --json` hides hook errors that the interactive TUI shows (Codex), and what the interactive Claude UI shows for failing hooks.
- Concurrency: two sessions writing events at the same instant (I ran sessions sequentially).
- `scratchpad_dir` appeared in Claude payloads only in my first smoke run (inherited desktop-app environment); it is absent in the clean-env fixtures.
- The Codex model and plan in `~/.codex/config.toml` are the owner's; with other models `apply_patch` may be called natively instead of via the JS `exec` wrapper (the hook payload would be unchanged, the rollout shape would not).
- Codex's `workspace-write` policy lists `<repo>/.git`, `.agents`, `.codex`, `.aws` as read-only protected paths (`missing_path_behavior: skip`); an `ls -a` from inside a Codex shell command showed empty `.agents`/`.aws`/`.codex` placeholders in the repo root, which were not there on the host afterwards. Relevant to WHY only as "a watcher can see transient dot-directories appear".

## 7. Recommendations (not decisions)

For **WHY-46 (wiring / `why-hook`)**
- Install at user level for both harnesses; keep the binary fail-open: parse errors, missing app, socket errors → exit 0 silently, **no stdout** (stdout of SessionStart/UserPromptSubmit goes into the model context), no stderr text with exit 2.
- Budget the hook: it runs in-line with every tool call. Aim for a few ms of work and a short socket timeout (≤ 200 ms), and set an explicit short `timeout` in the config we write (default 600 s would stall a session if the app hangs). Never `async`-spawn without a bound.
- Register only the events WHY needs. Codex `Pre/PostModelRequest` fire per model call and are not needed.
- Wrap each payload in an envelope the hook adds itself: `{harness, harness_version?, received_at, cwd, payload}`; payloads carry no harness name and no version.

For **WHY-12 (store)** / schema
- Keys: session = `(harness, session_id)`; turn = `(harness, session_id, prompt_id | turn_id)`; tool call = `tool_use_id`; subagent = `agent_id` (same session id as parent). Resume keeps the session id: sessions must be upsert-able, SessionStart is not "new session".
- Store the **cwd of each event** (it may differ from the repo root: subdirectory, linked worktree) and resolve to a worktree via git, not by string-prefix on `CLAUDE_PROJECT_DIR` (which is the session start cwd).
- Keep `transcript_path` per session (and `agent_transcript_path` per subagent) to read lazily; do not copy transcripts into the store (they contain the user's instructions).
- Treat `prompt`/`last_assistant_message` as user content; they can be large and sensitive.

For **WHY-13 (fail-open design)**: the tables in section 4. Verify `why-hook` with exit 1, a panic, a 6 s stall and a missing socket against both harnesses (the spike scripts reproduce them).

For **WHY-14 (Claude adapter)**
- Edits: trust `PostToolUse` for Write/Edit (`structuredPatch` + `originalFile`) as the *attribution* record (which turn touched which file with which hunk); for Bash-driven changes only git can tell, so the diff view stays git-first and the hook says "a Bash command ran between t1 and t2".
- Failures: `PostToolUseFailure` for Bash; harness-rejected edits only via `PostToolBatch` / transcript.
- Rationale: user prompt, Bash `description`, final text; `thinking` is empty.

For **WHY-15 (Codex adapter)**
- Edits: parse `tool_input.command` of `apply_patch` hooks (Add/Update/Delete File sections); do not parse the rollout's JS `exec` wrapper. A PreToolUse without a matching PostToolUse = **outcome unknown** (rejected patch or lost delivery); confirm from the rollout output or git before calling it rejected.
- Shell exit codes only exist in the rollout / `exec --json`, not in hooks; if WHY needs "tests failed", read the rollout `custom_tool_call_output` lazily.
- Transcript source: the **rollout JSONL at `transcript_path`**, not the sqlite projection. Redact/ignore `session_meta.base_instructions`, `world_state`, developer messages and the AGENTS.md user message: they hold owner-specific text.
- `session_meta.git.{commit_hash,branch}` gives the HEAD at session start for free.

## 8. Scenarios and what was scrubbed

Scenario list, commands and prompts: each fixture folder's `README.md`. Cost: all Claude runs on the `haiku` alias, all Codex runs on `gpt-6-luna` with effort `low`; about 60 short sessions.

Scrubbing (`scripts/agents-capture/save-fixtures.py`): home → `/HOME`, scratch dir → `/TMP`, WHY checkout → `/REPO`; e-mail addresses, account/organisation/creator ids and the timezone are replaced; hook env values are recorded only for an allow-list of harmless variables (names of the rest are kept, values `<redacted>`). Dropped from Claude transcripts: `api-request*`, `session-owner`, `atis-latch`, and the bodies of all `attachment` records except `environment`/`date`/`model`. Redacted in Codex rollouts: `base_instructions.text`, developer messages, the AGENTS.md message, `additional_tools`, `world_state`, `token_count.rate_limits` (plan type and usage of the owner's account). Record order, ids, timestamps and everything else are as recorded (JSON re-serialised compactly). Fixture repos were cumulative: a later scenario can see files an earlier one created.
