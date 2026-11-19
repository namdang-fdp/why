import { lazy, Suspense, useState, type ReactNode } from "react";
import { Binary, FileDiff, Files, FileWarning } from "lucide-react";
import { DiffSkeleton } from "@/components/diff/diff-skeleton";
import { hasContentDiff } from "@/components/diff/diff-preview";
import { selectActiveFile, selectChanges } from "@/stores/workspace";
import { useWorkspace } from "@/stores/workspace-context";
import { ErrorMessage, LoadingMessage } from "./async-message";
import { ChangeStatusBadge, LineCounts, displayPath } from "./change-labels";

const MonacoDiff = lazy(() => import("@/components/diff/monaco-diff"));

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

function DiffBody() {
  const file = useWorkspace(selectActiveFile);
  const changes = useWorkspace(selectChanges);
  const diff = useWorkspace((state) => state.fileDiff);
  const selectFile = useWorkspace((state) => state.selectFile);
  const preview = diff.status === "ok" && hasContentDiff(diff.data) ? diff.data : null;
  const [editorStarted, setEditorStarted] = useState(false);
  // Load Monaco on the first text preview and retain its editor through subsequent states.
  if (preview && !editorStarted) setEditorStarted(true);

  let placeholder: ReactNode;
  if (diff.status === "ok" && !preview) {
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
          <MonacoDiff diff={preview} />
        </Suspense>
      )}
      {placeholder && <div className="diff-content">{placeholder}</div>}
      {file && (diff.status === "loading" || diff.status === "idle") && <DiffSkeleton />}
    </div>
  );
}

export function DiffPane() {
  const file = useWorkspace(selectActiveFile);
  const diff = useWorkspace((state) => state.fileDiff);
  const layout = useWorkspace((state) => state.diffLayout);
  const setLayout = useWorkspace((state) => state.setDiffLayout);
  const pathLossy = file?.pathLossy || (diff.status === "ok" && diff.data.pathLossy);

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
      {file && (
        <div className="diff-metadata" aria-label="File metadata">
          {file.staged && <span>Staged</span>}
          {file.unstaged && <span>Unstaged</span>}
        </div>
      )}
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
