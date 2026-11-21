import type {
  ApplyResult,
  BinaryInfo,
  BinaryStatus,
  ChangeOutcome,
  CollectorState,
  HookHarness,
  HookPreview,
  HookState,
  HookStatusReport,
  PreviewKind,
  PreviewResult,
  RefusalReason,
} from "@/contracts";

export type HookFixtureScenario = {
  states?: Partial<Record<HookHarness, HookState["kind"]>>;
  collector?: CollectorState["kind"];
  binary?: BinaryStatus;
  refusal?: { reason: RefusalReason; at?: "preview" | "apply" };
  stale?: boolean;
  uninstall?: "restoreOriginal" | "removeEntries" | "deleteFile";
  instance?: string;
};

const configPaths = {
  claudeCode: "/home/you/.claude/settings.json",
  codex: "/home/you/.codex/hooks.json",
};
const codexEvents = [
  "SessionStart",
  "SessionEnd",
  "UserPromptSubmit",
  "PreToolUse",
  "PostToolUse",
  "Stop",
  "SubagentStart",
  "SubagentStop",
];
const events = {
  claudeCode: [
    ...codexEvents.slice(0, 5),
    "PostToolUseFailure",
    "PostToolBatch",
    ...codexEvents.slice(5),
  ],
  codex: codexEvents,
};
const codexReview: HookState = {
  kind: "needsReview",
  hint: "Codex runs a new or changed hook only after you approve it. Open Codex and approve WHY's hooks in `/hooks`. WHY cannot tell whether you already did, and it never bypasses the approval.",
};
const states: Record<HookState["kind"], HookState> = {
  notInstalled: { kind: "notInstalled" },
  installed: { kind: "installed" },
  outdated: { kind: "outdated", reasons: ["a timeout differs"] },
  needsReview: codexReview,
  disabledByUserSetting: {
    kind: "disabledByUserSetting",
    message: "Set `disableAllHooks` to false in your Claude Code settings to receive events.",
  },
  disabledByPolicy: { kind: "disabledByPolicy", source: "/home/you/managed/settings.json" },
  configInvalid: { kind: "configInvalid", message: "The hooks field must be a JSON object." },
};
const collectors: Record<CollectorState["kind"], CollectorState> = {
  starting: { kind: "starting" },
  listening: { kind: "listening" },
  disabled: { kind: "disabled", reason: "The fixture socket could not be bound." },
  notRunning: { kind: "notRunning" },
};
const refusalMessages: Record<RefusalReason, string> = {
  configInvalid: "The hooks field must be a JSON object.",
  disabledByPolicy:
    "Managed settings at /home/you/managed/settings.json disallow user-level hooks.",
  unsafeTarget: "The config file is a symbolic link. WHY only edits regular files.",
  readOnly: "The config file is read-only.",
  locked: "Another process is holding the config directory lock.",
  hookBinaryMissing: "This build has no bundled why-hook binary to install.",
  noHome: "WHY could not locate the user configuration directory.",
  tooLarge: "The config exceeds the size limit for the hook installer.",
};

type FixtureConfig = {
  env?: { PROJECT_MODE: string };
  disableAllHooks?: true;
  fixtureReview?: true;
  hooks?: Record<string, { hooks: { type: "command"; command: string; timeout: number }[] }[]>;
};

type FixtureFile = {
  state: HookState;
  generation: number;
  config: FixtureConfig | null;
  restore: { original: FixtureConfig | null; written: FixtureConfig } | null;
};

function configText(config: FixtureConfig | null) {
  return config === null ? null : JSON.stringify(config, null, 2) + "\n";
}

function withoutHooks(config: FixtureConfig) {
  const next = { ...config };
  delete next.hooks;
  return next;
}

function fixtureDiff(path: string, before: string | null, after: string | null) {
  if (before === after) return "";
  const a = before?.trimEnd().split("\n") ?? [];
  const b = after?.trimEnd().split("\n") ?? [];
  let prefix = 0;
  while (prefix < a.length && a[prefix] === b[prefix]) prefix++;
  let suffix = 0;
  while (
    suffix < Math.min(a.length, b.length) - prefix &&
    a[a.length - suffix - 1] === b[b.length - suffix - 1]
  )
    suffix++;
  const from = Math.max(0, prefix - 3);
  const context = suffix ? a.slice(a.length - suffix, a.length - suffix + 3) : [];
  const leading = a.slice(from, prefix).map((line) => ` ${line}`);
  const removed = a.slice(prefix, a.length - suffix).map((line) => `-${line}`);
  const added = b.slice(prefix, b.length - suffix).map((line) => `+${line}`);
  const oldCount = leading.length + removed.length + context.length;
  const newCount = leading.length + added.length + context.length;
  const file = path.split("/").at(-1);
  return [
    `--- ${before === null ? "/dev/null" : `a/${file}`}`,
    `+++ ${after === null ? "/dev/null" : `b/${file}`}`,
    `@@ -${oldCount ? from + 1 : from},${oldCount} +${newCount ? from + 1 : from},${newCount} @@`,
    ...leading,
    ...removed,
    ...added,
    ...context.map((line) => ` ${line}`),
    "",
  ].join("\n");
}

function hasEntries(state: HookState) {
  return ["installed", "outdated", "needsReview", "disabledByUserSetting"].includes(state.kind);
}

/** The dev simulator edits only these in-memory files. Setting a scenario starts a fresh simulation. */
export class HookFixtures {
  private readonly files: Record<HookHarness, FixtureFile>;
  private stale: boolean;
  private readonly instance: string;
  private readonly dataDir: string;
  private readonly binary: BinaryInfo;

  constructor(private readonly scenario: HookFixtureScenario = {}) {
    this.instance = scenario.instance ?? "dev:1550";
    const dev = this.instance.startsWith("dev:");
    this.dataDir = `/home/you/.local/share/why${dev ? "-dev" : ""}`;
    const bin = dev ? `bin/dev-${this.instance.slice(4)}` : "bin";
    const status = scenario.binary ?? "current";
    this.binary = {
      status,
      stablePath: `${this.dataDir}/${bin}/why-hook`,
      bundledPath:
        status === "missing" || status === "unverified" ? null : "/home/you/why/why-hook",
    };
    const file = (harness: HookHarness): FixtureFile => {
      const state = states[scenario.states?.[harness] ?? "notInstalled"];
      const original: FixtureConfig | null =
        scenario.uninstall !== "deleteFile" && (harness === "claudeCode" || scenario.uninstall)
          ? {
              ...(harness === "claudeCode" && { env: { PROJECT_MODE: "local" } }),
              ...(state.kind === "disabledByUserSetting" && { disableAllHooks: true }),
            }
          : null;
      if (!hasEntries(state)) return { state, generation: 1, config: original, restore: null };
      const written = { ...original, hooks: this.entries(harness, state.kind === "outdated") };
      const config: FixtureConfig =
        scenario.uninstall === "removeEntries" ? { ...written, fixtureReview: true } : written;
      return { state, generation: 1, config, restore: { original, written } };
    };
    this.files = { claudeCode: file("claudeCode"), codex: file("codex") };
    this.stale = scenario.stale ?? false;
  }

  private harness(harness: HookHarness) {
    return {
      harness,
      configPath: configPaths[harness],
      state: this.files[harness].state,
      events: events[harness],
    };
  }

  private socketPath() {
    const socket = this.instance.startsWith("dev:")
      ? `collector-dev-${this.instance.slice(4)}.sock`
      : "collector.sock";
    return `${this.dataDir}/run/${socket}`;
  }

  status(): HookStatusReport {
    const status = collectors[this.scenario.collector ?? "listening"];
    const running = status.kind !== "notRunning";
    return structuredClone({
      claudeCode: this.harness("claudeCode"),
      codex: this.harness("codex"),
      instance: this.instance,
      binary: this.binary,
      collector: {
        status,
        socketPath: this.socketPath(),
        eventsReceived: running ? (status.kind === "listening" ? 18 : 0) : null,
        spoolIngested: running ? 0 : null,
        spoolFiles: running ? (status.kind === "disabled" ? 3 : 0) : null,
        unavailable: ["lastEventAt"],
      },
    });
  }

  private refused(harness: HookHarness, kind: PreviewKind, at: "preview" | "apply") {
    const configured = this.scenario.refusal;
    const state = this.files[harness].state.kind;
    const reason =
      (configured && (configured.at ?? "preview") === at ? configured.reason : undefined) ??
      (state === "disabledByPolicy" && kind === "install" ? state : undefined) ??
      (state === "configInvalid" ? state : undefined) ??
      (at === "apply" && kind === "install" && this.binary.status === "missing"
        ? "hookBinaryMissing"
        : undefined);
    return reason ? { result: "refused" as const, reason, message: refusalMessages[reason] } : null;
  }

  private entries(harness: HookHarness, outdated: boolean): NonNullable<FixtureConfig["hooks"]> {
    return Object.fromEntries(
      events[harness].map((event) => [
        event,
        [
          {
            hooks: [
              {
                type: "command",
                command: `${this.binary.stablePath} --harness ${harness === "claudeCode" ? "claude-code" : "codex"} --instance ${this.instance} --data-dir ${this.dataDir} --socket-path ${this.socketPath()}`,
                timeout: outdated || event === "SessionEnd" ? 2 : 5,
              },
            ],
          },
        ],
      ]),
    );
  }

  private plan(
    harness: HookHarness,
    kind: PreviewKind,
  ): { preview: HookPreview; next: FixtureConfig | null } {
    const { config, generation, restore } = this.files[harness];
    const before = configText(config);
    const configPath = configPaths[harness];
    let next = config;
    let outcome: ChangeOutcome = "noChange";
    const notes: string[] = [];
    if (kind === "install") {
      next = { ...config, hooks: this.entries(harness, false) };
      outcome =
        before === null ? "createFile" : before === configText(next) ? "noChange" : "modifyFile";
      notes.push(
        `Adds one hook per event (${events[harness].length}) that forwards the event to WHY; your own hooks are left alone.`,
      );
      if (codexReview.kind === "needsReview" && harness === "codex") notes.push(codexReview.hint);
      if (outcome === "modifyFile") {
        notes.push(
          `WHY keeps a private copy of the original file at ${this.dataDir}/hook-installs/original.orig (mode 0600) to restore it exactly; it is deleted on uninstall.`,
        );
      }
    } else if (config?.hooks) {
      if (restore && config === restore.written) {
        next = restore.original;
        outcome = next === null ? "deleteFile" : "restoreOriginal";
      } else {
        next = withoutHooks(config);
        outcome = "removeEntries";
      }
    }
    const after = configText(next);
    return {
      next,
      preview: {
        harness,
        kind,
        configPath,
        fingerprint: config === null ? "absent" : `fixture:${harness}:${generation}`,
        outcome,
        before,
        after,
        diff: fixtureDiff(configPath, before, after),
        binary: { ...this.binary },
        notes,
      },
    };
  }

  preview(harness: HookHarness, kind: PreviewKind): PreviewResult {
    return (
      this.refused(harness, kind, "preview") ?? {
        result: "ready",
        preview: this.plan(harness, kind).preview,
      }
    );
  }

  apply(harness: HookHarness, kind: PreviewKind, fingerprint: string): ApplyResult {
    const refused = this.refused(harness, kind, "apply");
    if (refused) return refused;
    const file = this.files[harness];
    if (this.stale) {
      this.stale = false;
      file.config = { ...file.config, fixtureReview: true };
      file.generation++;
    }
    const { next, preview } = this.plan(harness, kind);
    if (fingerprint !== preview.fingerprint) return { result: "stale", preview };
    if (kind === "install") {
      if (next && preview.outcome !== "noChange") {
        const restore = file.restore;
        file.restore = {
          original:
            restore && file.config === restore.written
              ? restore.original
              : file.config && withoutHooks(file.config),
          written: next,
        };
      }
      if (file.state.kind !== "disabledByUserSetting") {
        file.state = harness === "codex" ? codexReview : states.installed;
      }
      if (this.binary.status === "outdated" && this.binary.bundledPath)
        this.binary.status = "current";
    } else {
      file.state = states.notInstalled;
      file.restore = null;
    }
    if (preview.outcome !== "noChange") {
      file.config = next;
      file.generation++;
    }
    return structuredClone({
      result: "applied",
      outcome: preview.outcome,
      status: this.harness(harness),
    });
  }
}
