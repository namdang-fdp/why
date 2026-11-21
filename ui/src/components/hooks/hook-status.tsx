import { useState } from "react";
import { ChevronRight, Copy, Terminal } from "lucide-react";
import type {
  BinaryInfo,
  CollectorState,
  HarnessHookStatus,
  HookHarness,
  HookState,
  HookStatusReport,
  PreviewKind,
} from "@/contracts";
import { Button } from "@/components/ui/button";

export const harnessNames: Record<HookHarness, string> = {
  claudeCode: "Claude Code",
  codex: "Codex",
};

const hookLabels: Record<HookState["kind"], string> = {
  notInstalled: "Not installed",
  installed: "Installed",
  outdated: "Outdated",
  needsReview: "Review in Codex",
  disabledByUserSetting: "Disabled in settings",
  disabledByPolicy: "Blocked by policy",
  configInvalid: "Config needs attention",
};

function hookExplanation(state: HookState) {
  switch (state.kind) {
    case "notInstalled":
      return "Preview the changes that would add WHY’s hooks to your config.";
    case "installed":
      return "WHY’s hook entries match this build. Preview uninstall to remove them.";
    case "outdated":
      return "WHY’s hooks need an update. Preview the changes below.";
    case "needsReview":
      return state.hint;
    case "disabledByUserSetting":
    case "configInvalid":
      return state.message;
    case "disabledByPolicy":
      return "Managed settings block user-level hooks. Contact your administrator to enable them.";
  }
}

export function binaryExplanation(binary: BinaryInfo) {
  switch (binary.status) {
    case "current":
      return "Matches the hook binary bundled with this build.";
    case "missing":
      return binary.bundledPath
        ? "The hook binary is missing from its install path. An install will copy the bundled binary."
        : "This build has no bundled hook binary to install. Use a build that includes why-hook.";
    case "outdated":
      return binary.bundledPath
        ? "Preview an install or update to reinstall the bundled hook binary."
        : "The hook binary needs replacing. Use a build with a bundled copy to reinstall it.";
    case "unverified":
      return "A hook binary is present. This build has no bundled copy to compare it with.";
  }
}

const binaryLabels: Record<BinaryInfo["status"], string> = {
  current: "Current",
  missing: "Missing",
  outdated: "Outdated",
  unverified: "Unverified",
};

/**
 * WHY cannot read Codex's trust state (it never touches `config.toml`), so a
 * Codex card would say "Review in Codex" forever. The one real evidence is an
 * event that arrived: once this instance has received one, say so, with its time.
 */
function receivingSince(status: HarnessHookStatus) {
  if (status.state.kind !== "needsReview" || status.lastEventAt === null) return null;
  const at = new Date(status.lastEventAt);
  return Number.isNaN(at.getTime()) ? status.lastEventAt : at.toLocaleString();
}

export function HarnessCard({
  status,
  binary,
  onPreview,
  refreshing,
}: {
  status: HarnessHookStatus;
  binary: BinaryInfo;
  refreshing: boolean;
  onPreview: (kind: PreviewKind, harness: HookHarness) => void;
}) {
  const { state, harness, events, configPath } = status;
  const name = harnessNames[harness];
  const receiving = receivingSince(status);
  const update =
    state.kind === "outdated" ||
    (state.kind !== "notInstalled" &&
      Boolean(binary.bundledPath) &&
      (binary.status === "outdated" || binary.status === "missing"));
  const canPreviewInstall =
    update ||
    state.kind === "notInstalled" ||
    state.kind === "disabledByPolicy" ||
    state.kind === "configInvalid";
  return (
    <section className="hook-harness" aria-labelledby={`hook-${harness}-title`}>
      <div className="hook-harness-title">
        <Terminal size={16} aria-hidden />
        <h3 id={`hook-${harness}-title`} tabIndex={-1}>
          {name}
        </h3>
        <span className="hook-state" data-kind={receiving === null ? state.kind : "installed"}>
          {receiving === null ? hookLabels[state.kind] : "Receiving events"}
        </span>
      </div>
      <p>
        {receiving === null
          ? hookExplanation(state)
          : `WHY has received events from ${name}. Last event ${receiving}.`}
      </p>
      {state.kind === "outdated" && state.reasons.length > 0 && (
        <ul className="hook-notes">
          {state.reasons.map((reason, index) => (
            <li key={index}>{reason}</li>
          ))}
        </ul>
      )}
      {state.kind === "disabledByPolicy" && (
        <p>
          Policy source <code>{state.source || "unknown"}</code>
        </p>
      )}
      {state.kind === "configInvalid" && <p>Check the config before installing hooks.</p>}
      <div className="hook-harness-config">
        <span>Config file</span>
        <code>{configPath || "unknown"}</code>
      </div>
      <details className="hook-events">
        <summary>
          <ChevronRight size={13} aria-hidden />
          Events WHY registers ({events.length})
        </summary>
        <ul aria-label={`${name} hook events`}>
          {events.map((event) => (
            <li key={event}>
              <code>{event}</code>
            </li>
          ))}
        </ul>
      </details>
      <div className="hook-actions">
        {canPreviewInstall && (
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={refreshing}
            onClick={() => onPreview("install", harness)}
          >
            {update ? "Preview update" : "Preview install"}
          </Button>
        )}
        {state.kind !== "notInstalled" && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={refreshing}
            onClick={() => onPreview("uninstall", harness)}
          >
            Preview uninstall
          </Button>
        )}
      </div>
    </section>
  );
}

const collectorLabels: Record<CollectorState["kind"], string> = {
  listening: "Listening",
  starting: "Starting",
  disabled: "Disabled",
  notRunning: "Not running",
};

function collectorExplanation(status: CollectorState) {
  switch (status.kind) {
    case "listening":
      return "Ready to receive hook events.";
    case "starting":
      return "The collector is starting. Refresh status to check when it’s ready.";
    case "disabled":
      return status.reason;
    case "notRunning":
      return "No collector is running in this WHY process.";
  }
}

const collectorFields = [
  { key: "eventsReceived", label: "Events received" },
  { key: "spoolFiles", label: "Spool files waiting" },
  { key: "spoolIngested", label: "Events read from spool" },
] as const;

function fieldLabel(key: string) {
  if (key === "lastEventAt") return "Last event";
  return key.replace(/([A-Z])/g, " $1").replace(/^./, (letter) => letter.toUpperCase());
}

function SocketPath({ path }: { path: string }) {
  const [copied, setCopied] = useState<"copied" | "failed" | null>(null);
  async function copy() {
    try {
      await navigator.clipboard.writeText(path);
      setCopied("copied");
    } catch {
      setCopied("failed");
    }
  }

  return (
    <div className="hook-socket">
      <dt>Collector socket</dt>
      <dd>
        <code>{path || "unknown"}</code>
        {path && (
          <Button
            type="button"
            aria-label="Copy socket path"
            variant="ghost"
            size="icon-sm"
            onClick={() => void copy()}
          >
            <Copy aria-hidden />
          </Button>
        )}
        {copied && (
          <span role="status">
            {copied === "copied"
              ? "Socket path copied."
              : "Copy failed. Select the path to copy it."}
          </span>
        )}
      </dd>
    </div>
  );
}

export function HookHealth({ report }: { report: HookStatusReport }) {
  const { collector, binary } = report;
  const unavailable = new Set(collector.unavailable);
  return (
    <section className="hook-health" aria-labelledby="hook-health-title">
      <div className="hook-health-title">
        <h3 id="hook-health-title">Collector and binary</h3>
        <span className="hook-instance">
          Instance <code>{report.instance}</code>
        </span>
      </div>
      <div className="hook-health-states">
        <div>
          <div className="hook-health-label">
            <strong>Collector</strong>
            <span className="hook-state" data-kind={collector.status.kind}>
              {collectorLabels[collector.status.kind]}
            </span>
          </div>
          <p>{collectorExplanation(collector.status)}</p>
        </div>
        <div>
          <div className="hook-health-label">
            <strong>Hook binary</strong>
            <span className="hook-state" data-kind={binary.status}>
              {binaryLabels[binary.status]}
            </span>
          </div>
          <p>{binaryExplanation(binary)}</p>
        </div>
      </div>
      <dl className="hook-metrics">
        {collectorFields.map(({ key, label }) => (
          <div key={key}>
            <dt>{label}</dt>
            <dd>{unavailable.has(key) ? "unknown" : (collector[key] ?? "unknown")}</dd>
          </div>
        ))}
        {[...unavailable]
          .filter(
            (key) => key !== "socketPath" && !collectorFields.some((field) => field.key === key),
          )
          .map((key) => (
            <div key={key}>
              <dt>{fieldLabel(key)}</dt>
              <dd>unknown</dd>
            </div>
          ))}
      </dl>
      <dl>
        <SocketPath
          key={collector.socketPath}
          path={unavailable.has("socketPath") ? "" : collector.socketPath}
        />
      </dl>
    </section>
  );
}
