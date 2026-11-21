import { lazy, Suspense, useContext, useEffect, useState, type ReactNode } from "react";
import { Binary, FileDiff, Files, FileWarning } from "lucide-react";
import { DiffSkeleton } from "@/components/diff/diff-skeleton";
import { hasContentDiff } from "@/components/diff/diff-preview";
import type { FileDiff as FileDiffData } from "@/contracts";
import { finishFileSwitch } from "@/lib/perf";
import { selectActiveFile, selectChanges } from "@/stores/workspace";
import { WorkspaceContext, useWorkspace } from "@/stores/workspace-context";
import { ErrorMessage, LoadingMessage } from "./async-message";
import { FilterNotice, FilterTrustPanel } from "./filter-trust";
import { ChangeStatusBadge, LineCounts, displayPath } from "./change-labels";

const loadMonacoDiff = () => import("@/components/diff/monaco-diff");
const MonacoDiff = lazy(loadMonacoDiff);

function DiffEmpty({
  Icon,
  title,
  detail,
  children,
}: {
  Icon: typeof Files;
  title: string;
  detail?: string;
  children?: ReactNode;
}) {
  return (
    <div className="diff-empty">
      <span className="empty-icon">
        <Icon size={22} aria-hidden />
      </span>
      <h3>{title}</h3>
      {detail && <p>{detail}</p>}
      {children}
    </div>
  );
}

/** Dim the old file after this long, and replace it with a skeleton after the longer one. */
const DIM_AFTER_MS = 120;
const SKELETON_AFTER_MS = 200;

/** True once `active` has been true for `ms` without a break. */
export function useDelayed(active: boolean, ms: number) {
  const [wasActive, setWasActive] = useState(active);
  const [elapsed, setElapsed] = useState(false);
  // Reset when it stops (derived state, adjusted during render).
  if (active !== wasActive) {
    setWasActive(active);
    if (!active) setElapsed(false);
  }
  useEffect(() => {
    if (!active) return;
    const timer = setTimeout(() => setElapsed(true), ms);
    return () => clearTimeout(timer);
  }, [active, ms]);
  return active && elapsed;
}

function DiffBody() {
  const file = useWorkspace(selectActiveFile);
  const changes = useWorkspace(selectChanges);
  const diff = useWorkspace((state) => state.fileDiff);
  const selectFile = useWorkspace((state) => state.selectFile);
  const preview = diff.status === "ok" && hasContentDiff(diff.data) ? diff.data : null;
  const [editorStarted, setEditorStarted] = useState(false);
  // Load Monaco on the first text preview and retain its editor through subsequent states.
  if (preview && !editorStarted) setEditorStarted(true);

  // The previous file stays on screen while the next one loads, instead of being
  // replaced at once by a skeleton and then by a half-built diff: a quick switch
  // is then just old -> new. `retained` is the last text diff shown; it is
  // dropped as soon as the state is anything but "ok" or "loading".
  const [retained, setRetained] = useState<FileDiffData | null>(null);
  const loading = diff.status === "loading";
  if (diff.status === "ok" ? retained !== preview : !loading && retained !== null) {
    setRetained(diff.status === "ok" ? preview : null);
  }
  const displayed = loading ? retained : preview;
  // What the editor reports as really visible (null: nothing, or still booting).
  const [shown, setShown] = useState<FileDiffData | null>(null);
  const waiting = loading || (preview !== null && shown !== preview);
  const slowForDim = useDelayed(waiting, DIM_AFTER_MS);
  const slowForSkeleton = useDelayed(waiting, SKELETON_AFTER_MS);
  const showSkeleton = waiting && (shown === null || slowForSkeleton) && file !== undefined;
  const dim = waiting && shown !== null && slowForDim;

  // A file with no text diff (binary, too large) has no editor to reveal it.
  useEffect(() => {
    if (diff.status === "ok" && !preview) finishFileSwitch(diff.data.path);
  }, [diff, preview]);

  const repo = useWorkspace((state) =>
    state.repositories.status === "ok"
      ? state.repositories.data.find((entry) => entry.id === state.repoId)
      : undefined,
  );
  const blocked = changes.status === "error" && changes.error.kind === "untrustedFilters";

  let placeholder: ReactNode;
  if (blocked && repo) {
    // WHY refused to read this repository: explain, and ask.
    placeholder = <FilterTrustPanel repo={repo} />;
  } else if (diff.status === "ok" && !preview) {
    const path = displayPath(diff.data);
    if (diff.data.binary)
      placeholder = <DiffEmpty Icon={Binary} title="Binary file" detail={path} />;
    else if (diff.data.tooLarge)
      placeholder = (
        <DiffEmpty Icon={FileWarning} title="Too large to preview (> 1 MiB)" detail={path} />
      );
    else placeholder = <DiffEmpty Icon={FileDiff} title="No content changes" detail={path} />;
  } else if (file && diff.status === "error") {
    placeholder = <ErrorMessage error={diff.error} retry={() => void selectFile(file.path)} />;
  } else if (!file && changes.status === "loading") {
    placeholder = <LoadingMessage>Loading changes…</LoadingMessage>;
  } else if (!file && changes.status === "ok" && changes.data.length === 0) {
    placeholder = (
      <DiffEmpty
        Icon={Files}
        title="No changes"
        detail="There are no changed files in this mode."
      />
    );
  } else if (!file) {
    placeholder = (
      <DiffEmpty
        Icon={Files}
        title="Select a file"
        detail={
          changes.status === "ok"
            ? "Choose a changed file from the rail."
            : "Choose a worktree to see its changed files."
        }
      >
        {changes.status === "ok" && (
          <div className="empty-shortcut">
            <kbd>j</kbd> <kbd>k</kbd> <span>Navigate files</span>
          </div>
        )}
      </DiffEmpty>
    );
  }

  return (
    <div className="diff-body">
      {editorStarted && (
        <Suspense fallback={preview && <DiffSkeleton />}>
          <MonacoDiff diff={displayed} stale={dim} onReveal={setShown} />
        </Suspense>
      )}
      {placeholder && <div className="diff-content">{placeholder}</div>}
      {showSkeleton && <DiffSkeleton />}
      {file && diff.status === "idle" && <DiffSkeleton />}
    </div>
  );
}

export function DiffPane() {
  // Monaco is a ~2.8 MB chunk: fetch and parse it while the app is idle, so the
  // first file the reader opens does not wait for it (measured at ~1.7 s cold).
  useEffect(() => {
    const timer = setTimeout(() => void loadMonacoDiff(), 400);
    return () => clearTimeout(timer);
  }, []);

  const file = useWorkspace(selectActiveFile);
  const diff = useWorkspace((state) => state.fileDiff);
  const layout = useWorkspace((state) => state.diffLayout);

  // Line origins are read only for the file open here, never for every changed file.
  // A path that is not valid UTF-8 shares its display string with other files: never ask.
  const workspace = useContext(WorkspaceContext);
  const worktreeId = useWorkspace((state) => state.worktreeId);
  const originsPath = file && !file.pathLossy ? file.path : null;
  useEffect(() => {
    const origins = workspace.getState().lineOrigins.getState();
    void origins.setTarget(originsPath === null ? null : worktreeId, originsPath);
  }, [workspace, worktreeId, originsPath]);
  useEffect(
    () => () => void workspace.getState().lineOrigins.getState().setTarget(null, null),
    [workspace],
  );
  const setLayout = useWorkspace((state) => state.setDiffLayout);
  const pathLossy = file?.pathLossy || (diff.status === "ok" && diff.data.pathLossy);
  const repo = useWorkspace((state) =>
    state.repositories.status === "ok"
      ? state.repositories.data.find((entry) => entry.id === state.repoId)
      : undefined,
  );

  return (
    <main className="diff-pane" aria-labelledby="diff-heading">
      <div className="pane-heading">
        <FileDiff size={15} aria-hidden />
        <h2 id="diff-heading">Diff</h2>
        {file && (
          <>
            <div className="diff-file-name">
              <ChangeStatusBadge status={file.status} />
              <span className="truncate" title={displayPath(file)}>
                {displayPath(file)}
              </span>
            </div>
            <LineCounts file={file} />
            <div className="diff-chips" aria-label="File metadata">
              {file.staged && <span>Staged</span>}
              {file.unstaged && <span>Unstaged</span>}
            </div>
          </>
        )}
        <div className="diff-layout" role="group" aria-label="Diff layout">
          <button
            type="button"
            aria-pressed={layout === "split"}
            onClick={() => setLayout("split")}
          >
            Split
          </button>
          <button
            type="button"
            aria-pressed={layout === "unified"}
            onClick={() => setLayout("unified")}
          >
            Unified
          </button>
        </div>
      </div>
      {repo && <FilterNotice repo={repo} />}
      {pathLossy && (
        <p className="diff-path-notice">
          <FileWarning size={14} aria-hidden />
          The file path could not be decoded exactly.
        </p>
      )}
      <DiffBody />
    </main>
  );
}
