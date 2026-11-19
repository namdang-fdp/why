import { TriangleAlert } from "lucide-react";
import { useWorkspace } from "@/stores/workspace-context";

/**
 * Shown for the whole session when the database could not be opened and the
 * core fell back to memory. Calm on purpose (a notice, not an alarm): the app
 * works, only the repository list is lost on exit. Not dismissable, because the
 * situation does not go away.
 */
export function StorageBanner() {
  const storage = useWorkspace((state) => state.storage);
  if (storage.status !== "ok" || storage.data.persistent) return null;
  const { reason } = storage.data;
  return (
    <div className="storage-banner" role="status">
      <TriangleAlert size={14} aria-hidden />
      <p>
        <strong>Repositories won&apos;t be saved</strong>
        {reason ? `: ${reason}` : ""}. They are forgotten when WHY closes.
      </p>
    </div>
  );
}
