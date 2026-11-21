import { useContext, useLayoutEffect, useRef } from "react";
import { usePanelLayout } from "@/hooks/use-panel-layout";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { useWorkspace, WorkspaceContext } from "@/stores/workspace-context";
import { DiffPane } from "./diff-pane";
import { Inspector } from "./inspector";
import { LeftRail } from "./left-rail";
import { Onboarding } from "./onboarding";
import { SettingsPage } from "./settings-page";
import { settingsButtonId } from "./rail-footer";

export function WorkspacePanels() {
  const store = useContext(WorkspaceContext);
  const settings = useWorkspace((state) => state.settingsView);
  const previousFocus = useRef<HTMLElement | null | undefined>(undefined);
  useLayoutEffect(
    () =>
      store.subscribe((state, previous) => {
        if (previous.settingsView.status === "closed" && state.settingsView.status === "open") {
          const focused = document.activeElement;
          previousFocus.current =
            focused instanceof HTMLElement && focused !== document.body ? focused : null;
        }
      }),
    [store],
  );

  useLayoutEffect(() => {
    const previous = previousFocus.current;
    if (settings.status !== "closed" || previous === undefined) return;
    previousFocus.current = undefined;
    const fallback =
      document.getElementById(settingsButtonId) ?? document.getElementById("shell-command");
    (previous?.isConnected ? previous : fallback)?.focus({ preventScroll: true });
  }, [settings]);

  const inspectorOpen = useWorkspace((state) => state.inspectorOpen);
  const noRepositories = useWorkspace(
    (state) => state.repositories.status === "ok" && state.repositories.data.length === 0,
  );
  const railLayout = usePanelLayout("why:rail-layout:v1", { rail: 27, workspace: 73 });
  const inspectorLayout = usePanelLayout("why:inspector-layout:v1", { diff: 70, inspector: 30 });

  if (settings.status === "open") return <SettingsPage section={settings.section} />;

  // First run (or the last repository was removed): the whole workspace is one invitation.
  if (noRepositories) return <Onboarding />;

  return (
    <div className="workspace-scroll" data-inspector-open={inspectorOpen}>
      <ResizablePanelGroup
        id="review-layout"
        className="review-layout"
        orientation="horizontal"
        {...railLayout}
      >
        <ResizablePanel id="rail" minSize={230} maxSize={510}>
          <LeftRail />
        </ResizablePanel>
        <ResizableHandle aria-label="Resize navigation rail" />
        <ResizablePanel id="workspace" minSize={inspectorOpen ? 520 : 300}>
          <ResizablePanelGroup
            id="review-detail-layout"
            orientation="horizontal"
            {...(inspectorOpen ? inspectorLayout : {})}
          >
            <ResizablePanel id="diff" minSize={300}>
              <DiffPane />
            </ResizablePanel>
            {inspectorOpen && <ResizableHandle aria-label="Resize Why inspector" />}
            {inspectorOpen && (
              <ResizablePanel id="inspector" minSize={218} maxSize={430}>
                <Inspector />
              </ResizablePanel>
            )}
          </ResizablePanelGroup>
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
}
