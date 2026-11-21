import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { LockKeyhole, RefreshCw } from "lucide-react";
import type {
  ApplyResult,
  ChangeOutcome,
  HookHarness,
  HookPreview,
  HookStatusReport,
  PreviewKind,
} from "@/contracts";
import { ErrorMessage, LoadingMessage } from "@/components/shell/async-message";
import { Button } from "@/components/ui/button";
import { idle, resolveState, type AsyncState, type Settled } from "@/data/async-state";
import type { DataSource } from "@/data/data-source";
import { useWorkspace } from "@/stores/workspace-context";
import { appliedOutcomes, HookReview, type HookRequest, type ReviewState } from "./hook-review";
import { HarnessCard, harnessNames, HookHealth } from "./hook-status";

type ReportRead = AsyncState<HookStatusReport> & { previous?: HookStatusReport };
type Flow =
  | { view: "overview"; applied?: { request: HookRequest; outcome: ChangeOutcome } }
  | { view: "review"; state: ReviewState };

// A confirmed write can outlive this panel. A returning visit waits for it
// before reading status, so it can't offer actions based on pre-write state.
const pendingApplies = new WeakMap<
  Pick<DataSource, "install" | "uninstall">,
  Promise<Settled<ApplyResult>>
>();

function HooksPanel() {
  const source = useWorkspace((state) => state.hookData);
  const [report, setReport] = useState<ReportRead>(idle);
  const [flow, setFlow] = useState<Flow>({ view: "overview" });
  const current = useRef<symbol | null>(null);
  const consumedPreview = useRef<HookPreview | null>(null);
  const restore = useRef<HookHarness | null>(null);
  const snapshot = report.status === "ok" ? report.data : report.previous;

  const invalidate = useCallback(() => {
    current.current = null;
  }, []);
  // Each visit owns its UI updates, even when a request outlives the page.
  useLayoutEffect(() => invalidate, [invalidate]);

  const refresh = useCallback(
    async (previous?: HookStatusReport) => {
      if (current.current) return;
      const version = Symbol();
      current.current = version;
      setReport({ status: "loading", previous });
      const pending = pendingApplies.get(source);
      if (pending) await pending;
      if (current.current !== version) return;
      const result = await resolveState(() => source.hookStatus());
      if (current.current !== version) return;
      current.current = null;
      setReport(result.status === "ok" ? result : { ...result, previous });
    },
    [source],
  );

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useLayoutEffect(() => {
    if (flow.view !== "overview" || !restore.current || !snapshot) return;
    document.getElementById(`hook-${restore.current}-title`)?.focus();
    restore.current = null;
  }, [flow, snapshot]);

  async function preview(request: HookRequest) {
    if (current.current) return;
    const version = Symbol();
    current.current = version;
    consumedPreview.current = null;
    const update = (state: ReviewState) => setFlow({ view: "review", state });
    update({ request, status: "loading" });
    const { kind, harness } = request;
    const result = await resolveState(() =>
      kind === "install" ? source.previewInstall(harness) : source.previewUninstall(harness),
    );
    if (current.current !== version) return;
    current.current = null;
    if (result.status === "error") {
      update({ request, status: "error", stage: "preview", error: result.error });
    } else if (result.data.result === "refused") {
      update({ request, status: "refused", ...result.data });
    } else {
      update({ request, status: "review", preview: result.data.preview, stale: false });
    }
  }

  async function confirm(approved: HookPreview) {
    if (
      current.current ||
      pendingApplies.has(source) ||
      consumedPreview.current === approved ||
      flow.view !== "review" ||
      flow.state.status !== "review" ||
      flow.state.preview !== approved
    )
      return;
    // Consume this specific preview before React rerenders. A second activation
    // cannot submit the same snapshot while the first request is pending.
    consumedPreview.current = approved;
    const version = Symbol();
    current.current = version;
    const { kind, harness, fingerprint } = approved;
    const request = flow.state.request;
    const update = (state: ReviewState) => setFlow({ view: "review", state });
    update({ request, status: "applying", preview: approved });
    const pending = resolveState(() =>
      kind === "install"
        ? source.install(harness, fingerprint)
        : source.uninstall(harness, fingerprint),
    );
    pendingApplies.set(source, pending);
    const result = await pending;
    pendingApplies.delete(source);
    if (current.current !== version) return;
    current.current = null;
    if (result.status === "error") {
      update({ request, status: "error", stage: "apply", error: result.error });
      return;
    }
    const applied = result.data;
    switch (applied.result) {
      case "refused":
        update({ request, status: "refused", ...applied });
        break;
      case "stale":
        update({ request, status: "review", preview: applied.preview, stale: true });
        break;
      case "applied":
        restore.current = harness;
        setFlow({ view: "overview", applied: { request, outcome: applied.outcome } });
        void refresh(snapshot && { ...snapshot, [harness]: applied.status });
        break;
    }
  }

  function back(request: HookRequest) {
    invalidate();
    restore.current = request.harness;
    setFlow({ view: "overview" });
  }

  function showPreview(kind: PreviewKind, harness: HookHarness) {
    void preview({ kind, harness });
  }

  return (
    <>
      {flow.view === "overview" && (
        <Button
          className="hook-refresh"
          aria-label="Refresh status"
          aria-busy={report.status === "loading"}
          aria-disabled={report.status === "loading"}
          type="button"
          size="sm"
          variant="ghost"
          onClick={() => void refresh(snapshot)}
        >
          <RefreshCw size={14} aria-hidden />
          <span>{report.status === "loading" ? "Refreshing…" : "Refresh status"}</span>
        </Button>
      )}
      <div className="hook-body">
        {flow.view === "review" ? (
          <HookReview
            state={flow.state}
            onBack={() => back(flow.state.request)}
            onConfirm={(approved) => void confirm(approved)}
            onRetry={(request) => void preview(request)}
          />
        ) : (
          <>
            {flow.applied && (
              <p className="hook-applied" role="status">
                <strong>{harnessNames[flow.applied.request.harness]}</strong>
                {appliedOutcomes[flow.applied.outcome]}
              </p>
            )}
            {report.status === "error" && (
              <ErrorMessage
                context="Hook status"
                error={report.error}
                retry={() => void refresh(snapshot)}
              />
            )}
            {!snapshot && (report.status === "loading" || report.status === "idle") && (
              <LoadingMessage>Checking agent hooks…</LoadingMessage>
            )}
            {snapshot && (
              <>
                <div className="hook-harnesses">
                  <HarnessCard
                    status={snapshot.claudeCode}
                    binary={snapshot.binary}
                    refreshing={report.status === "loading"}
                    onPreview={showPreview}
                  />
                  <HarnessCard
                    status={snapshot.codex}
                    binary={snapshot.binary}
                    refreshing={report.status === "loading"}
                    onPreview={showPreview}
                  />
                </div>
                <HookHealth report={snapshot} />
              </>
            )}
          </>
        )}
      </div>
    </>
  );
}

export function AgentHooksSection() {
  return (
    <div className="agent-hooks-section">
      <p className="hook-description">
        Review changes to your user-level config before WHY writes them.
      </p>
      <HooksPanel />
      <footer className="hook-privacy">
        <LockKeyhole size={15} aria-hidden />
        <p>
          Everything stays on this machine. WHY captures activity only from repositories you
          register and never reads agent credential stores. Uninstall restores your original file
          when possible and preserves any later edits.
        </p>
      </footer>
    </div>
  );
}
