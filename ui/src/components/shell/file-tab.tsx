import { selectActiveFile } from "@/stores/workspace";
import { useSessions, useWorkspace } from "@/stores/workspace-context";
import { inspectorViewStore } from "@/stores/inspector-view";
import { evidenceKind, fileCaveat, fileChipLabel } from "./attribution-model";
import { FileActivity } from "./file-activity";
import { useCurrentAttribution } from "./use-attribution";

/**
 * The inspector's File tab: why does THIS file look like this, and how far can
 * that answer be trusted? One evidence chip, the captured edits behind it, and
 * (only when there are none) a lead about shell commands. Never a session log.
 */
export function FileTab() {
  const file = useWorkspace(selectActiveFile);
  const attribution = useCurrentAttribution();
  const sessions = useSessions((state) => state.list);

  if (!file) {
    return (
      <div className="inspector-body">
        <p className="rail-note">
          Select a changed file to see its captured edits, or open the{" "}
          <button
            type="button"
            className="session-text-button"
            onClick={() => inspectorViewStore.getState().setTab("worktree")}
          >
            Worktree tab
          </button>{" "}
          for an overview.
        </p>
      </div>
    );
  }

  // The chip only claims something once the answer for THIS worktree and comparison has
  // arrived and names this file. Anything else is neutral: never "no evidence" by default.
  // A path that was not valid UTF-8 is never joined by path (the core leaves it unexplained).
  const entry =
    attribution.status === "ok" && !file.pathLossy
      ? attribution.data.files.find((item) => item.change.path === file.path)
      : undefined;
  const kind = entry && evidenceKind(entry);
  const leads = attribution.status === "ok" ? attribution.data.shellLeads.length : 0;
  const sessionCount = sessions.status === "ok" ? sessions.data.length : null;

  return (
    <div className="inspector-body">
      <div className="file-header">
        <h3 title={file.path}>{file.path.slice(file.path.lastIndexOf("/") + 1)}</h3>
        {entry && kind ? (
          <span className="evidence-chip file-chip" data-kind={kind}>
            {fileChipLabel(entry)}
          </span>
        ) : (
          <span className="evidence-chip file-chip" data-kind="pending" role="status">
            {attribution.status === "error"
              ? "Evidence unavailable"
              : file.pathLossy
                ? "Path not joined to evidence"
                : "Checking captured evidence…"}
          </span>
        )}
        {kind === "explained" && <p className="evidence-note">{fileCaveat}</p>}
      </div>
      <FileActivity
        file={file}
        empty={
          <>
            <p className="evidence-note">No captured agent edit names this file.</p>
            {leads > 0 && (
              <p className="evidence-note">
                {leads} {leads === 1 ? "turn" : "turns"} ran shell commands here; WHY cannot
                attribute file changes made by shell commands yet.
              </p>
            )}
          </>
        }
      />
      {sessionCount !== null && (
        <p className="file-sessions">
          Session activity: {sessionCount} {sessionCount === 1 ? "session" : "sessions"}.{" "}
          <button
            type="button"
            className="session-text-button"
            onClick={() => inspectorViewStore.getState().setTab("worktree")}
          >
            View sessions
          </button>
        </p>
      )}
    </div>
  );
}
