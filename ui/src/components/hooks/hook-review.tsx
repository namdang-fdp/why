import { useLayoutEffect, useRef } from "react";
import { ArrowLeft, FileDiff } from "lucide-react";
import type {
  ChangeOutcome,
  HookHarness,
  HookPreview,
  IpcError,
  PreviewKind,
  RefusalReason,
} from "@/contracts";
import { Button } from "@/components/ui/button";
import { ErrorMessage, LoadingMessage } from "@/components/shell/async-message";
import { binaryExplanation, harnessNames } from "./hook-status";

export type HookRequest = { kind: PreviewKind; harness: HookHarness };

export type ReviewState = { request: HookRequest } & (
  | { status: "loading" }
  | { status: "review"; preview: HookPreview; stale: boolean }
  | { status: "applying"; preview: HookPreview }
  | { status: "error"; error: IpcError; stage: "preview" | "apply" }
  | { status: "refused"; reason: RefusalReason; message: string }
);

const refusalTitles: Record<RefusalReason, string> = {
  configInvalid: "Invalid config",
  disabledByPolicy: "Managed policy blocks this install",
  unsafeTarget: "Unsafe config path",
  readOnly: "Read-only config",
  locked: "Config is locked",
  hookBinaryMissing: "Hook binary unavailable",
  noHome: "Config location unavailable",
  tooLarge: "Config file is too large",
};

const previewOutcomes: Record<ChangeOutcome, string> = {
  noChange: "The config file will stay as it is.",
  createFile: "Create the config file with WHY’s hook entries.",
  modifyFile: "Add or update WHY’s hook entries in the config file.",
  restoreOriginal: "Restore the original file exactly.",
  deleteFile: "Delete the unchanged config file created by WHY.",
  removeEntries: "Remove only WHY’s hook entries and keep your edits.",
};

export const appliedOutcomes: Record<ChangeOutcome, string> = {
  noChange: "The config file is unchanged.",
  createFile: "Config file created with WHY’s hook entries.",
  modifyFile: "WHY’s hook entries were written.",
  restoreOriginal: "Original file restored exactly.",
  deleteFile: "The config file created by WHY was deleted.",
  removeEntries: "WHY’s hook entries were removed. Your edits were kept.",
};

function ConfigDiff({ diff }: { diff: string }) {
  return (
    <pre className="hook-diff" aria-label="Unified config diff" tabIndex={0}>
      <code>
        {diff.split(/(?<=\n)/).map((line, index) => {
          const kind =
            index < 2 || line.startsWith("@@")
              ? "meta"
              : line.startsWith("+")
                ? "add"
                : line.startsWith("-")
                  ? "remove"
                  : "context";
          return (
            <span key={index} data-kind={kind}>
              {line}
            </span>
          );
        })}
      </code>
    </pre>
  );
}

function PreviewEvidence({ preview }: { preview: HookPreview }) {
  const { binary } = preview;
  return (
    <>
      <div className="hook-preview-file">
        <FileDiff size={17} aria-hidden />
        <div>
          <strong>{previewOutcomes[preview.outcome]}</strong>
          <code>{preview.configPath}</code>
        </div>
      </div>
      {preview.diff ? (
        <ConfigDiff key={preview.fingerprint} diff={preview.diff} />
      ) : (
        <p>No config diff to show.</p>
      )}
      {preview.kind === "install" &&
        (binary.status === "missing" || binary.status === "outdated") && (
          <div className="hook-binary-note">
            <strong>Hook binary</strong>
            <p>{binaryExplanation(binary)}</p>
            {binary.stablePath && <code>{binary.stablePath}</code>}
          </div>
        )}
      {preview.notes.length > 0 && (
        <ul className="hook-notes" aria-label="Preview notes">
          {preview.notes.map((note, index) => (
            <li key={index}>{note}</li>
          ))}
        </ul>
      )}
    </>
  );
}

export function HookReview({
  state,
  onBack,
  onConfirm,
  onRetry,
}: {
  state: ReviewState;
  onBack: () => void;
  onConfirm: (preview: HookPreview) => void;
  onRetry: (request: HookRequest) => void;
}) {
  const titleRef = useRef<HTMLHeadingElement>(null);
  const fingerprint = "preview" in state ? state.preview.fingerprint : null;
  useLayoutEffect(() => {
    // Focus the evidence on first review and again on a stale response. Enter
    // on the heading has no action, even when the user pressed it twice.
    titleRef.current?.focus();
  }, [state.status, fingerprint]);

  const install = state.request.kind === "install";
  const applying = state.status === "applying";
  const verb = install ? "install" : "uninstall";
  return (
    <section className="hook-review" aria-labelledby="hook-review-title">
      <h3 id="hook-review-title" ref={titleRef} tabIndex={-1}>
        {install ? "Install" : "Uninstall"} {harnessNames[state.request.harness]} hooks
      </h3>
      {state.status === "loading" && <LoadingMessage>Reading the config preview…</LoadingMessage>}
      {state.status === "review" && state.stale && (
        <div className="hook-refusal" data-stale role="status">
          <strong>The file changed since your preview.</strong>
          <p>Review the updated diff and confirm again. No change was written.</p>
        </div>
      )}
      {"preview" in state && <PreviewEvidence preview={state.preview} />}
      {state.status === "error" && (
        <>
          <ErrorMessage
            context={state.stage === "apply" ? "Applying hook changes" : "Reading hook preview"}
            error={state.error}
            retry={() => onRetry(state.request)}
          />
          {state.stage === "apply" && <p>Try again to read a fresh preview before confirming.</p>}
        </>
      )}
      {state.status === "refused" && (
        <div className="hook-refusal" role="status">
          <strong>{refusalTitles[state.reason]}</strong>
          <p>{state.message}</p>
          <p>No change was written.</p>
        </div>
      )}
      <div className="hook-review-actions">
        <Button type="button" size="sm" variant="ghost" disabled={applying} onClick={onBack}>
          <ArrowLeft aria-hidden />
          Back to status
        </Button>
        {applying && <p role="status">Applying the reviewed change…</p>}
        {state.status === "review" && (
          <Button
            type="button"
            size="sm"
            onKeyDown={(event) => {
              if (event.repeat && (event.key === "Enter" || event.key === " "))
                event.preventDefault();
            }}
            onClick={(event) => {
              if (event.detail <= 1) onConfirm(state.preview);
            }}
          >
            Confirm {verb}
          </Button>
        )}
      </div>
    </section>
  );
}
