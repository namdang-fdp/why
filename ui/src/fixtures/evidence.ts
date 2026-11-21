// Fixture answers for the evidence commands (WHY-16 attribution, WHY-53a
// rationale and Codex shell outcomes). Everything here is invented: no real
// paths, no real transcripts. The turn ids point into `./sessions.ts`, so a
// click on a turn in the rail finds that turn in the timeline.
import type {
  FileAttribution,
  RationaleResult,
  ShellLead,
  ShellOutcomesResult,
  TranscriptUnavailable,
  TurnRef,
  WorktreeId,
} from "@/contracts";
import { WT_WHY_AGENT, WT_WHY_MAIN } from "./repos";
import { SESSION_CLAUDE, SESSION_CODEX, SESSION_SUBAGENT } from "./sessions";

/** Every reason the core can give for "nothing could be read", for scenario switches and tests. */
export const unavailableReasons = [
  "noTranscript",
  "missing",
  "outsideRoot",
  "unsupportedPlatform",
  "notFound",
  "unreadable",
  "unsupportedHarness",
  "notApplicable",
] as const satisfies readonly TranscriptUnavailable[];

function at(minutes: number, seconds = 0): string {
  return new Date(Date.UTC(2026, 10, 20, 9, minutes, seconds)).toISOString();
}

// ---- Turn references -------------------------------------------------------------

const CLAUDE_T0 = "c3a1f0e2-0001";
const CODEX_T0 = "01a1f812-8eb3-7732-9caa-21c0b36398fc";
const CODEX_T1 = "01a1f812-9f04-7c11-8d2e-5b0a7e3c1d22";
const SUB_T0 = "d41d8cd9-0001";
const SUB_T2 = "d41d8cd9-0003";

const claude = (partial: Pick<TurnRef, "path" | "outcome" | "at"> & Partial<TurnRef>): TurnRef => ({
  harness: "claudeCode",
  sessionId: SESSION_CLAUDE,
  turnId: CLAUDE_T0,
  turnIndex: 0,
  agentId: null,
  toolUseId: "toolu_fixture_c1-edit",
  ...partial,
});

const codex = (partial: Pick<TurnRef, "path" | "outcome" | "at"> & Partial<TurnRef>): TurnRef => ({
  harness: "codex",
  sessionId: SESSION_CODEX,
  turnId: CODEX_T0,
  turnIndex: 0,
  agentId: null,
  toolUseId: null,
  ...partial,
});

const GIT = "src-tauri/src/git";

function file(
  change: FileAttribution["change"],
  explainedBy: TurnRef[],
  attempts: TurnRef[] = [],
): FileAttribution {
  const explained = explainedBy.length > 0;
  return {
    change,
    status: explained ? "explained" : "unexplained",
    evidence: explained ? "observed" : "unknown",
    explainedBy,
    attempts,
  };
}

type Change = FileAttribution["change"];
export type AttributionEntry = { explainedBy: TurnRef[]; attempts: TurnRef[] };

/**
 * What the core attributes, by worktree then by (new) path. A path that is not
 * listed is `unexplained` with no attempts, as in the real core. Between them
 * these cover: one turn, several turns across both harnesses, a subagent, a
 * rename (the edit named the OLD path), attempts that never applied, and files
 * nothing explains.
 */
export const attributionTable: Record<WorktreeId, Record<string, AttributionEntry>> = {
  [WT_WHY_AGENT]: {
    // Edited by Claude, then again by Codex.
    [`${GIT}/mod.rs`]: {
      explainedBy: [
        codex({
          turnId: CODEX_T1,
          turnIndex: 1,
          outcome: "ok",
          at: at(12, 29),
          path: `${GIT}/mod.rs`,
          toolUseId: "toolu_fixture_x2-retry",
        }),
        claude({ outcome: "ok", at: at(0, 31), path: `${GIT}/mod.rs` }),
      ],
      attempts: [
        codex({
          turnId: CODEX_T1,
          turnIndex: 1,
          outcome: "unknown",
          at: at(12, 14),
          path: `${GIT}/mod.rs`,
          toolUseId: "toolu_fixture_x2-rejected",
        }),
      ],
    },
    [`${GIT}/worktree.rs`]: {
      explainedBy: [
        codex({
          outcome: "ok",
          at: at(10, 18),
          path: `${GIT}/worktree.rs`,
          toolUseId: "toolu_fixture_x1-add",
        }),
      ],
      attempts: [],
    },
    // A rename: the call named the OLD path, the file is listed under the new one.
    [`${GIT}/parse.rs`]: {
      explainedBy: [
        claude({
          outcome: "ok",
          at: at(0, 58),
          path: `${GIT}/parser.rs`,
          toolUseId: "toolu_fixture_c1-write",
        }),
      ],
      attempts: [],
    },
    // Only an unconfirmed edit (a rejected patch OR a lost delivery): not an explanation.
    [`${GIT}/legacy.rs`]: {
      explainedBy: [],
      attempts: [
        codex({
          turnId: CODEX_T1,
          turnIndex: 1,
          outcome: "unknown",
          at: at(12, 14),
          path: `${GIT}/legacy.rs`,
          toolUseId: "toolu_fixture_x2-rejected",
        }),
      ],
    },
    // The harness said the edit failed.
    "src-tauri/Cargo.toml": {
      explainedBy: [],
      attempts: [claude({ outcome: "failed", at: at(0, 40), path: "src-tauri/Cargo.toml" })],
    },
  },
  [WT_WHY_MAIN]: {
    "src-tauri/src/lib.rs": {
      explainedBy: [
        {
          harness: "claudeCode",
          sessionId: SESSION_SUBAGENT,
          turnId: SUB_T0,
          turnIndex: 0,
          agentId: "a7f3c2e1b9d84f06",
          toolUseId: "toolu_fixture_s1-agent-write",
          outcome: "ok",
          at: at(20, 21),
          path: "src-tauri/src/lib.rs",
        },
        {
          harness: "claudeCode",
          sessionId: SESSION_SUBAGENT,
          turnId: SUB_T2,
          turnIndex: 2,
          agentId: null,
          toolUseId: "toolu_fixture_s3-edit",
          outcome: "ok",
          at: at(23, 16),
          path: "src-tauri/src/lib.rs",
        },
      ],
      attempts: [],
    },
  },
};

/** Shell turns per worktree: a lead for "look here", never an attribution. */
export const shellLeadTable: Record<WorktreeId, ShellLead[]> = {
  [WT_WHY_AGENT]: [
    {
      harness: "codex",
      sessionId: SESSION_CODEX,
      turnId: "01a1f812-a7c9-7e60-b3f1-9e4d2c6a8b07",
      turnIndex: 2,
      commandCount: 3,
      firstAt: at(14, 7),
    },
    {
      harness: "claudeCode",
      sessionId: SESSION_CLAUDE,
      turnId: "c3a1f0e2-0002",
      turnIndex: 1,
      commandCount: 1,
      firstAt: at(3, 9),
    },
  ],
  [WT_WHY_MAIN]: [
    {
      harness: "claudeCode",
      sessionId: SESSION_SUBAGENT,
      turnId: SUB_T0,
      turnIndex: 0,
      commandCount: 2,
      firstAt: at(20, 44),
    },
  ],
};

export function attributeFile(worktreeId: WorktreeId, change: Change): FileAttribution {
  const entry = attributionTable[worktreeId]?.[change.path];
  return file(change, entry?.explainedBy ?? [], entry?.attempts ?? []);
}

/**
 * `get_file_activity`: every edit call that NAMED exactly this path, newest first,
 * any outcome. Like the real core it does not follow renames: a call made under
 * a file's old name is only found by asking for the old name.
 */
export function fileActivity(worktreeId: WorktreeId, path: string): TurnRef[] {
  const refs = Object.values(attributionTable[worktreeId] ?? {}).flatMap((entry) => [
    ...entry.explainedBy,
    ...entry.attempts,
  ]);
  return refs.filter((ref) => ref.path === path).sort((a, b) => b.at.localeCompare(a.at));
}

// ---- Rationale ----------------------------------------------------------------------

const claudeSource = "claudeTranscript" as const;
const codexSource = "codexRollout" as const;

function ok(
  source: typeof claudeSource | typeof codexSource,
  blocks: Array<{
    text: string;
    position?: "turn" | "before" | "after";
    encrypted?: boolean;
    timestamp?: string | null;
  }>,
  extra: { truncated?: boolean; versionSupported?: boolean; harnessVersion?: string | null } = {},
): RationaleResult {
  return {
    status: "ok",
    rationale: {
      evidence: "declared",
      source,
      blocks: blocks.map(({ text, position = "turn", encrypted = false, timestamp = null }) => ({
        text,
        position,
        encrypted,
        timestamp,
      })),
      harnessVersion: extra.harnessVersion ?? (source === claudeSource ? "2.1.150" : "0.9.2"),
      versionSupported: extra.versionSupported ?? true,
      truncated: extra.truncated ?? false,
    },
  };
}

const unavailable = (reason: TranscriptUnavailable): RationaleResult => ({
  status: "unavailable",
  reason,
});

/** A block cut at the core's text cap ends with this character. */
const LONG_TEXT = `${"The helper reads the config once and caches it. ".repeat(40)}…`;

/** `get_turn_rationale` answers by turn id. Unlisted turns are `notFound`. */
export const turnRationales: Record<string, RationaleResult> = {
  [CLAUDE_T0]: ok(claudeSource, [
    {
      text: "I'll read calc.py first, add subtract next to add(), then write a unit test that covers negative results.",
      position: "turn",
      timestamp: at(0, 10),
    },
    {
      text: "Added `subtract(a, b)` and a unit test; all 4 tests pass.",
      position: "turn",
      timestamp: at(1, 40),
    },
  ]),
  "c3a1f0e2-0002": unavailable("missing"),
  "c3a1f0e2-0003": unavailable("noTranscript"),
  [CODEX_T0]: unavailable("unsupportedPlatform"),
  [CODEX_T1]: ok(codexSource, [
    { text: LONG_TEXT, position: "turn" },
    { text: "(encrypted)", position: "turn", encrypted: true },
  ]),
  "01a1f812-a7c9-7e60-b3f1-9e4d2c6a8b07": ok(
    codexSource,
    [{ text: "Running the unit tests to check the current state.", position: "turn" }],
    { versionSupported: false, harnessVersion: "9.0.0" },
  ),
  [SUB_T0]: ok(claudeSource, [], { truncated: true }),
  "d41d8cd9-0002": unavailable("outsideRoot"),
  [SUB_T2]: unavailable("unreadable"),
};

/** `get_tool_rationale` answers by `TimelineEvent.id`. Unlisted calls are `notFound`. */
export const toolRationales: Record<string, RationaleResult> = {
  "c1-edit": ok(claudeSource, [
    {
      text: "Reading calc.py showed add() but no subtract, so I'm adding it right below.",
      position: "before",
    },
    { text: "The edit applied cleanly; now the test file.", position: "after" },
  ]),
  "c1-write": ok(claudeSource, [
    { text: "Creating the test file next to the others.", position: "before" },
  ]),
  "x1-add": unavailable("unsupportedHarness"),
  "x2-rejected": ok(codexSource, [{ text: "(encrypted)", position: "before", encrypted: true }], {
    truncated: true,
  }),
  "s1-agent-write": unavailable("notApplicable"),
};

export function turnRationale(sessionId: string, turnId: string): RationaleResult {
  void sessionId;
  return turnRationales[turnId] ?? unavailable("notFound");
}

export function toolRationale(sessionId: string, eventId: string): RationaleResult {
  void sessionId;
  return toolRationales[eventId] ?? unavailable("notFound");
}

// ---- Recovered Codex shell outcomes ---------------------------------------------------

export type OutcomesScenario = "default" | "unavailable" | "unsupportedVersion" | "truncated";

/** `get_shell_outcomes`. The sample Codex session has two `unknown` shell calls in its last turn. */
export function shellOutcomes(
  sessionId: string,
  scenario: OutcomesScenario = "default",
): ShellOutcomesResult {
  if (sessionId !== SESSION_CODEX) return { status: "unavailable", reason: "notApplicable" };
  if (scenario === "unavailable") return { status: "unavailable", reason: "missing" };
  const base = {
    outcomes: [] as Extract<ShellOutcomesResult, { status: "ok" }>["result"]["outcomes"],
    unresolved: 0,
    harnessVersion: "0.9.2" as string | null,
    versionSupported: true,
    truncated: false,
  };
  if (scenario === "unsupportedVersion") {
    return {
      status: "ok",
      result: { ...base, unresolved: 3, harnessVersion: "9.0.0", versionSupported: false },
    };
  }
  const turnId = "01a1f812-a7c9-7e60-b3f1-9e4d2c6a8b07";
  return {
    status: "ok",
    result: {
      ...base,
      outcomes: [
        {
          toolUseId: "toolu_fixture_x3-test",
          eventId: "x3-test",
          turnId,
          agentId: null,
          exitCode: 1,
          outcome: "failed",
          evidence: "observed",
          source: "codexRollout",
        },
        {
          toolUseId: "toolu_fixture_x3-lint",
          eventId: "x3-lint",
          turnId,
          agentId: null,
          exitCode: 0,
          outcome: "ok",
          evidence: "observed",
          source: "codexRollout",
        },
      ],
      // x3-mystery stays unknown: the rollout has no matching record.
      unresolved: 1,
      truncated: scenario === "truncated",
    },
  };
}
