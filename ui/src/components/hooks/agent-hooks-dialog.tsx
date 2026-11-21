import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { LockKeyhole, RefreshCw, X } from "lucide-react";
import type {
  ChangeOutcome,
  HookHarness,
  HookPreview,
  HookStatusReport,
  PreviewKind,
} from "@/contracts";
import { ErrorMessage, LoadingMessage } from "@/components/shell/async-message";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { idle, resolveState, type AsyncState } from "@/data/async-state";
import { useDialogFocus } from "@/hooks/use-dialog-focus";
import { useWorkspace } from "@/stores/workspace-context";
import { appliedOutcomes, HookReview, type HookRequest, type ReviewState } from "./hook-review";
import { HarnessCard, harnessNames, HookHealth } from "./hook-status";

type ReportRead = AsyncState<HookStatusReport> & { previous?: HookStatusReport };
type Flow =
  | { view: "overview"; applied?: { request: HookRequest; outcome: ChangeOutcome } }
  | { view: "review"; state: ReviewState };

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
  // Each visit owns its requests. Closing the Radix dialog discards this
  // panel immediately, even while its empty shell animates out.
  useLayoutEffect(() => invalidate, [invalidate]);

  const refresh = useCallback(
    async (previous?: HookStatusReport) => {
      if (current.current) return;
      const version = Symbol();
      current.current = version;
      setReport({ status: "loading", previous });
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
    const result = await resolveState(() =>
      kind === "install"
        ? source.install(harness, fingerprint)
        : source.uninstall(harness, fingerprint),
    );
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

export function AgentHooksDialog() {
  const open = useWorkspace((state) => state.agentHooksOpen);
  const setOpen = useWorkspace((state) => state.setAgentHooksOpen);
  const focus = useDialogFocus("shell-agent-hooks");
  const closeButton = useRef<HTMLButtonElement>(null);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent
        {...focus}
        className="agent-hooks-dialog"
        showCloseButton={false}
        onInteractOutside={(event) => event.preventDefault()}
        onOpenAutoFocus={(event) => {
          focus.onOpenAutoFocus();
          event.preventDefault();
          closeButton.current?.focus();
        }}
      >
        <header className="hook-dialog-header">
          <DialogTitle>Agent hooks</DialogTitle>
          <DialogDescription>
            Review changes to your user-level config before WHY writes them.
          </DialogDescription>
        </header>
        <DialogClose asChild>
          <Button
            className="hook-dialog-close"
            ref={closeButton}
            type="button"
            aria-label="Close agent hooks"
            variant="ghost"
            size="icon-sm"
          >
            <X aria-hidden />
          </Button>
        </DialogClose>
        {open && <HooksPanel />}
        <footer className="hook-privacy">
          <LockKeyhole size={15} aria-hidden />
          <p>
            Everything stays on this machine. WHY captures activity only from repositories you
            register and never reads agent credential stores. Uninstall restores your original file
            when possible and preserves any later edits.
          </p>
        </footer>
      </DialogContent>
    </Dialog>
  );
}
