import { useContext, type ReactNode } from "react";
import type { FileChange } from "@/contracts";
import { WorkspaceContext, useEvidence } from "@/stores/workspace-context";
import { RefLine, useTitles } from "./rail-attribution";
import { ErrorMessage } from "./async-message";

/**
 * The file's captured edit calls, newest first, any outcome: the inspector's
 * answer to "which turns touched this file?". Each line opens that turn in
 * the session explorer. `empty` is what to say when the core found none.
 */
export function FileActivity({ file, empty }: { file: FileChange; empty: ReactNode }) {
  const store = useContext(WorkspaceContext);
  const path = useEvidence((state) => state.activityPath);
  const activity = useEvidence((state) => state.activity);
  const titles = useTitles();
  // Never show the previous file's history under a newly selected file.
  if (path !== file.path) return null;
  return (
    <section className="file-activity" aria-label={`Captured edits to ${file.path}`}>
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
          empty
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
