import { FileDiff, Files } from "lucide-react";
import { selectActiveFile, selectChanges } from "@/stores/workspace";
import { useWorkspace } from "@/stores/workspace-context";
import { ErrorMessage, LoadingMessage } from "./async-message";
import { ChangeStatusBadge, LineCounts, displayPath } from "./change-labels";

export function DiffPane() {
  const file = useWorkspace(selectActiveFile);
  const changes = useWorkspace(selectChanges);
  const diff = useWorkspace((state) => state.fileDiff);
  const selectFile = useWorkspace((state) => state.selectFile);

  const preparing = changes.status === "loading";
  const clean = changes.status === "ok" && changes.data.length === 0;

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
      </div>
      {file && (
        <div className="diff-metadata" aria-label="File metadata">
          {file.staged && <span>Staged</span>}
          {file.unstaged && <span>Unstaged</span>}
          {file.binary && <span>Binary</span>}
          {file.pathLossy && <span>Lossy path</span>}
          {diff.status === "ok" && diff.data.tooLarge && <span>Too large to preview</span>}
        </div>
      )}
      <div className="diff-content">
        {file && diff.status === "error" ? (
          <ErrorMessage error={diff.error} retry={() => void selectFile(file.path)} />
        ) : (
          <div className="diff-empty">
            <span className="empty-icon">
              {file ? <FileDiff size={22} aria-hidden /> : <Files size={22} aria-hidden />}
            </span>
            {file ? (
              <>
                <h3>Diff preview</h3>
                {diff.status === "loading" ? (
                  <LoadingMessage>Loading file metadata…</LoadingMessage>
                ) : (
                  <p>The diff editor will appear here.</p>
                )}
              </>
            ) : preparing ? (
              <LoadingMessage>Loading changes…</LoadingMessage>
            ) : (
              <>
                <h3>{clean ? "No changes" : "Select a file"}</h3>
                <p>
                  {clean
                    ? "There are no changed files in this mode."
                    : changes.status === "ok"
                      ? "Choose a changed file from the rail."
                      : "Choose a worktree to see its changed files."}
                </p>
                {!clean && changes.status === "ok" && (
                  <div className="empty-shortcut">
                    <kbd>j</kbd>
                    <kbd>k</kbd>
                    <span>Navigate files</span>
                  </div>
                )}
              </>
            )}
          </div>
        )}
      </div>
    </main>
  );
}
