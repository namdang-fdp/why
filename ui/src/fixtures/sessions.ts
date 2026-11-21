// Sample agent sessions (v0.2 session context) for building the timeline UI
// without the Rust core. Shapes follow what the core builds from real hook
// payloads (WHY-45 fixtures, docs/agents/payloads.md):
//  - a Claude Code session (no model: Claude hooks do not report it),
//  - a Codex session (model known; a rejected apply_patch has outcome "unknown"),
//  - a Claude session with a subagent, a failed shell command and a tool call
//    whose result never arrived,
//  - plus, from `@/dev-fixtures/long-session`, one ~500-turn session.
// Every value is checked against the generated contracts with `satisfies`.
import type { SessionSummary, SessionTimeline, TimelineEvent, Turn } from "@/contracts";
import { WT_WHY_AGENT, WT_WHY_MAIN } from "./repos";

export const SESSION_CLAUDE = "claude-code:69c7ffb9-e8d9-43db-8d22-958e94e1e889";
export const SESSION_CODEX = "codex:01a1f812-8d57-7393-87df-7cfc4fefbfb3";
export const SESSION_SUBAGENT = "claude-code:2b4e1d0a-5c3f-4a8e-9b71-0d6c2e8f4a13";

/** `2026-11-20T09:MM:SS.000Z`, `minutes` after 09:00 UTC. */
function at(minutes: number, seconds = 0): string {
  const date = new Date(Date.UTC(2026, 10, 20, 9, minutes, seconds));
  return date.toISOString();
}

/** Defaults for the fields most entries share. */
function event(
  partial: Pick<TimelineEvent, "id" | "kind" | "at" | "summary"> & Partial<TimelineEvent>,
): TimelineEvent {
  return {
    toolName: null,
    files: [],
    evidence: "observed",
    outcome: null,
    agentId: null,
    // A tool call (it has an outcome) gets a plausible fixture id; the rest null.
    toolUseId: partial.outcome != null ? `toolu_fixture_${partial.id}` : null,
    ...partial,
  };
}

// ---- Claude Code: three turns, ended ------------------------------------------

const claudeTurns = [
  {
    id: "c3a1f0e2-0001",
    index: 0,
    prompt: "Add a subtract function to src/calc.py and a test for it.",
    startedAt: at(0, 5),
    endedAt: at(1, 40),
    events: [
      event({
        id: "c1-prompt",
        kind: "prompt",
        at: at(0, 5),
        summary: "Add a subtract function to src/calc.py and a test for it.",
      }),
      event({
        id: "c1-read",
        kind: "toolCall",
        at: at(0, 12),
        toolName: "Read",
        summary: "src/calc.py",
        files: ["src/calc.py"],
        outcome: "ok",
      }),
      event({
        id: "c1-edit",
        kind: "edit",
        at: at(0, 31),
        toolName: "Edit",
        summary: "src/calc.py (+3 −0)",
        files: ["src/calc.py"],
        outcome: "ok",
      }),
      event({
        id: "c1-write",
        kind: "edit",
        at: at(0, 58),
        toolName: "Write",
        summary: "tests/test_calc.py (new file)",
        files: ["tests/test_calc.py"],
        outcome: "ok",
      }),
      event({
        id: "c1-test",
        kind: "testRun",
        at: at(1, 20),
        toolName: "Bash",
        summary: "python -m unittest discover -s tests",
        outcome: "ok",
        evidence: "observed",
      }),
      event({
        id: "c1-stop",
        kind: "stop",
        at: at(1, 40),
        summary: "Added `subtract(a, b)` and a unit test; all 4 tests pass.",
        evidence: "declared",
      }),
    ],
  },
  {
    id: "c3a1f0e2-0002",
    index: 1,
    prompt: "Rename old.py to new.py.",
    startedAt: at(3, 0),
    endedAt: at(3, 25),
    events: [
      event({ id: "c2-prompt", kind: "prompt", at: at(3, 0), summary: "Rename old.py to new.py." }),
      event({
        id: "c2-shell",
        kind: "shell",
        at: at(3, 9),
        toolName: "Bash",
        // Changes made through the shell are invisible to the hooks: no files.
        summary: "git mv old.py new.py",
        outcome: "ok",
      }),
      event({
        id: "c2-stop",
        kind: "stop",
        at: at(3, 25),
        summary: "Renamed.",
        evidence: "declared",
      }),
    ],
  },
  {
    id: "c3a1f0e2-0003",
    index: 2,
    prompt: "Explain what calc.py does.",
    startedAt: at(5, 0),
    endedAt: at(5, 30),
    events: [
      event({
        id: "c3-prompt",
        kind: "prompt",
        at: at(5, 0),
        summary: "Explain what calc.py does.",
      }),
      event({
        id: "c3-message",
        kind: "assistantMessage",
        at: at(5, 28),
        summary: "calc.py defines add and subtract helpers used by the CLI.",
        evidence: "declared",
      }),
      event({ id: "c3-stop", kind: "stop", at: at(5, 30), summary: "", evidence: "declared" }),
    ],
  },
] satisfies Turn[];

const claudeSession = {
  session: {
    id: SESSION_CLAUDE,
    harness: "claudeCode",
    model: null,
    worktreeId: WT_WHY_AGENT,
    cwd: "/home/dev/code/why-worktrees/agent-git",
    task: "Add a subtract function to src/calc.py and a test for it.",
    startedAt: at(0, 0),
    lastEventAt: at(5, 31),
    status: "ended",
    turnCount: claudeTurns.length,
  },
  turns: claudeTurns,
} satisfies SessionTimeline;

// ---- Codex: three turns, still live, a rejected patch -------------------------

const codexTurns = [
  {
    id: "01a1f812-8eb3-7732-9caa-21c0b36398fc",
    index: 0,
    prompt: "Create src/notes.py containing NOTE = 1, then delete src/old.py.",
    startedAt: at(10, 0),
    endedAt: at(10, 50),
    events: [
      event({
        id: "x1-prompt",
        kind: "prompt",
        at: at(10, 0),
        summary: "Create src/notes.py containing NOTE = 1, then delete src/old.py.",
      }),
      event({
        id: "x1-add",
        kind: "edit",
        at: at(10, 18),
        toolName: "apply_patch",
        summary: "Add File: src/notes.py",
        files: ["src/notes.py"],
        outcome: "ok",
      }),
      event({
        id: "x1-delete",
        kind: "edit",
        at: at(10, 33),
        toolName: "apply_patch",
        summary: "Delete File: src/old.py",
        files: ["src/old.py"],
        outcome: "ok",
      }),
      event({
        id: "x1-stop",
        kind: "stop",
        at: at(10, 50),
        summary: "I'll inspect the target files, then apply each requested change… Done.",
        evidence: "declared",
      }),
    ],
  },
  {
    id: "01a1f812-9f04-7c11-8d2e-5b0a7e3c1d22",
    index: 1,
    prompt: "Update the docstring in src/calc.py.",
    startedAt: at(12, 0),
    endedAt: at(12, 41),
    events: [
      event({
        id: "x2-prompt",
        kind: "prompt",
        at: at(12, 0),
        summary: "Update the docstring in src/calc.py.",
      }),
      event({
        id: "x2-rejected",
        kind: "edit",
        at: at(12, 14),
        toolName: "apply_patch",
        summary: "Update File: src/calc.py",
        files: ["src/calc.py"],
        // PreToolUse without PostToolUse: rejected patch OR lost delivery.
        outcome: "unknown",
      }),
      event({
        id: "x2-retry",
        kind: "edit",
        at: at(12, 29),
        toolName: "apply_patch",
        summary: "Update File: src/calc.py",
        files: ["src/calc.py"],
        outcome: "ok",
      }),
      event({
        id: "x2-stop",
        kind: "stop",
        at: at(12, 41),
        summary: "Docstring updated.",
        evidence: "declared",
      }),
    ],
  },
  {
    id: "01a1f812-a7c9-7e60-b3f1-9e4d2c6a8b07",
    index: 2,
    prompt: "Run the tests.",
    startedAt: at(14, 0),
    endedAt: null,
    events: [
      event({ id: "x3-prompt", kind: "prompt", at: at(14, 0), summary: "Run the tests." }),
      event({
        id: "x3-test",
        kind: "testRun",
        at: at(14, 7),
        toolName: "Bash",
        summary: "python -m unittest",
        // Codex hooks carry no exit code: the core cannot tell pass from fail.
        outcome: "unknown",
      }),
      // More shell calls the hooks left `unknown`. The rollout settles two of them
      // (see `evidence.ts`); the third has no matching record and stays unknown.
      event({
        id: "x3-lint",
        kind: "shell",
        at: at(14, 20),
        toolName: "Bash",
        summary: "cargo fmt --check",
        outcome: "unknown",
      }),
      event({
        id: "x3-mystery",
        kind: "shell",
        at: at(14, 31),
        toolName: "Bash",
        summary: "./scripts/check.sh --all",
        outcome: "unknown",
      }),
    ],
  },
] satisfies Turn[];

const codexSession = {
  session: {
    id: SESSION_CODEX,
    harness: "codex",
    model: "gpt-6-luna",
    worktreeId: WT_WHY_AGENT,
    cwd: "/home/dev/code/why-worktrees/agent-git/src",
    task: "Create src/notes.py containing NOTE = 1, then delete src/old.py.",
    startedAt: at(9, 58),
    lastEventAt: at(14, 7),
    status: "live",
    turnCount: codexTurns.length,
  },
  turns: codexTurns,
} satisfies SessionTimeline;

// ---- Claude with a subagent, a failure and an unknown outcome -----------------

const AGENT = "a7f3c2e1b9d84f06";

const subagentTurns = [
  {
    id: "d41d8cd9-0001",
    index: 0,
    prompt: "Use a subagent to create src/helper.py, then run the failing test.",
    startedAt: at(20, 0),
    endedAt: at(21, 10),
    events: [
      event({
        id: "s1-prompt",
        kind: "prompt",
        at: at(20, 0),
        summary: "Use a subagent to create src/helper.py, then run the failing test.",
      }),
      event({
        id: "s1-agent",
        kind: "subagent",
        at: at(20, 6),
        toolName: "Agent",
        summary: "general-purpose: create src/helper.py",
        outcome: "ok",
      }),
      event({
        id: "s1-agent-start",
        kind: "subagent",
        at: at(20, 7),
        summary: "Subagent started (general-purpose)",
        agentId: AGENT,
      }),
      event({
        id: "s1-agent-write",
        kind: "edit",
        at: at(20, 21),
        toolName: "Write",
        summary: "src/helper.py (new file)",
        files: ["src/helper.py"],
        outcome: "ok",
        agentId: AGENT,
      }),
      event({
        id: "s1-agent-stop",
        kind: "subagent",
        at: at(20, 30),
        summary: "Created src/helper.py with a `helper()` function.",
        evidence: "declared",
        agentId: AGENT,
      }),
      event({
        id: "s1-failed",
        kind: "testRun",
        at: at(20, 44),
        toolName: "Bash",
        summary: "python -m unittest tests.test_helper",
        // PostToolUseFailure: "Exit code 1".
        outcome: "failed",
      }),
      event({
        id: "s1-unknown",
        kind: "shell",
        at: at(20, 58),
        toolName: "Bash",
        summary: "python -m unittest tests.test_helper -v",
        files: ["/tmp/why-agent/test-helper.log"],
        // Pre seen, no result delivered.
        outcome: "unknown",
      }),
      event({
        id: "s1-stop",
        kind: "stop",
        at: at(21, 10),
        summary: "The helper exists but its test fails: `helper()` returns None.",
        evidence: "declared",
      }),
    ],
  },
  {
    id: "d41d8cd9-0002",
    index: 1,
    // Not typed by the human: Claude Code injects this when a background
    // subagent finishes.
    prompt: "<task-notification>Subagent a7f3c2e1b9d84f06 completed</task-notification>",
    startedAt: at(21, 15),
    endedAt: at(21, 20),
    events: [
      event({
        id: "s2-prompt",
        kind: "prompt",
        at: at(21, 15),
        summary: "<task-notification>Subagent a7f3c2e1b9d84f06 completed</task-notification>",
      }),
      event({
        id: "s2-stop",
        kind: "stop",
        at: at(21, 20),
        summary: "Noted.",
        evidence: "declared",
      }),
    ],
  },
  {
    id: "d41d8cd9-0003",
    index: 2,
    prompt: "Fix helper() so the test passes.",
    startedAt: at(23, 0),
    endedAt: at(23, 45),
    events: [
      event({
        id: "s3-prompt",
        kind: "prompt",
        at: at(23, 0),
        summary: "Fix helper() so the test passes.",
      }),
      event({
        id: "s3-edit",
        kind: "edit",
        at: at(23, 16),
        toolName: "Edit",
        summary: "src/helper.py (+1 −1)",
        files: ["src/helper.py"],
        outcome: "ok",
      }),
      event({
        id: "s3-test",
        kind: "testRun",
        at: at(23, 33),
        toolName: "Bash",
        summary: "python -m unittest tests.test_helper",
        outcome: "ok",
      }),
      event({
        id: "s3-stop",
        kind: "stop",
        at: at(23, 45),
        summary: "Fixed; the test passes.",
        evidence: "declared",
      }),
    ],
  },
] satisfies Turn[];

const subagentSession = {
  session: {
    id: SESSION_SUBAGENT,
    harness: "claudeCode",
    model: null,
    worktreeId: WT_WHY_MAIN,
    cwd: "/home/dev/code/why",
    task: "Use a subagent to create src/helper.py, then run the failing test.",
    startedAt: at(19, 55),
    lastEventAt: at(23, 46),
    status: "ended",
    turnCount: subagentTurns.length,
  },
  turns: subagentTurns,
} satisfies SessionTimeline;

/** The hand-written sample sessions (the long one is generated on demand). */
export const sessionTimelines: SessionTimeline[] = [claudeSession, codexSession, subagentSession];

/** Just the summaries, as `list_sessions` returns them. */
export const sessionSummaries: SessionSummary[] = sessionTimelines.map((t) => t.session);
