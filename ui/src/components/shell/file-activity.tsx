import { useContext } from "react";
import { selectActiveFile } from "@/stores/workspace";
import { WorkspaceContext, useEvidence, useWorkspace } from "@/stores/workspace-context";
import { RefLine, useTitles } from "./rail-attribution";
import { ErrorMessage } from "./async-message";

/**
 * The selected file's captured edit calls, newest first, any outcome. It is
 * the inspector's answer to "which turns touched this file?"; each line opens
 * that turn in the timeline.
 */
export function FileActivity() {
  const store = useContext(WorkspaceContext);
  const file = useWorkspace(selectActiveFile);
  const path = useEvidence((state) => state.activityPath);
  const activity = useEvidence((state) => state.activity);
  const titles = useTitles();
  // Never show the previous file's history under a newly selected file.
  if (!file || path !== file.path) return null;
  return (
    <section className="file-activity" aria-label={`Edit history of ${file.path}`}>
      <h3>
        Edits to <code title={file.path}>{file.path.slice(file.path.lastIndexOf("/") + 1)}</code>
      </h3>
      {(activity.status === "loading" || activity.status === "idle") && (
        <p className="rail-note" role="status">
          Loading edit history…
        </p>
      )}
      {activity.status === "error" && (
        <ErrorMessage
          context="Edit history"
          error={activity.error}
          retry={() => void store.getState().evidence.getState().loadActivity()}
        />
      )}
      {activity.status === "ok" &&
        (activity.data.length === 0 ? (
          <p className="evidence-note">No captured agent edit call names this file.</p>
        ) : (
          <ul>
            {activity.data.map((reference, index) => (
              <RefLine
                key={`${reference.sessionId}-${reference.turnId}-${reference.toolUseId ?? index}-${index}`}
                reference={reference}
                file={file}
                titles={titles}
                close={() => undefined}
              />
            ))}
          </ul>
        ))}
    </section>
  );
}
