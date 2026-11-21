import { Settings } from "lucide-react";
import { Tip } from "@/components/ui/overlays";
import { settingsKeys } from "@/hooks/use-workspace-shortcuts";
import { useWorkspace } from "@/stores/workspace-context";

export const settingsButtonId = "rail-settings";

export function RailFooter() {
  const openSettings = useWorkspace((state) => state.openSettings);

  return (
    <div className="rail-footer">
      <Tip label="Settings" shortcut={settingsKeys} side="top">
        <button
          id={settingsButtonId}
          type="button"
          className="rail-row"
          aria-keyshortcuts="Meta+, Control+,"
          onClick={() => openSettings()}
        >
          <Settings size={15} aria-hidden />
          Settings
        </button>
      </Tip>
    </div>
  );
}
