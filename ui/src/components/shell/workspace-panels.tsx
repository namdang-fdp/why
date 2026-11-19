import { useState } from "react";
import type { Layout, LayoutChangedMeta } from "react-resizable-panels";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { useWorkspace } from "@/stores/workspace-context";
import { DiffPane } from "./diff-pane";
import { Inspector } from "./inspector";
import { LeftRail } from "./left-rail";

function usePanelLayout(key: string, fallback: Layout) {
  const [layout, setLayout] = useState(() => {
    try {
      const raw = window.localStorage.getItem(key);
      if (raw === null) return fallback;
      const saved: unknown = JSON.parse(raw);
      if (!saved || typeof saved !== "object") return fallback;

      const restored: Layout = {};
      for (const id of Object.keys(fallback)) {
        const size: unknown = Reflect.get(saved, id);
        if (typeof size !== "number" || !Number.isFinite(size) || size <= 0 || size >= 100) {
          return fallback;
        }
        restored[id] = size;
      }
      const sum = Object.values(restored).reduce((total, size) => total + size, 0);
      return Math.abs(sum - 100) < 0.1 ? restored : fallback;
    } catch {
      return fallback;
    }
  });

  return {
    defaultLayout: layout,
    onLayoutChanged(next: Layout, meta: LayoutChangedMeta) {
      if (!meta.isUserInteraction) return;
      const saved = meta.requestedLayout ?? next;
      setLayout(saved);
      try {
        window.localStorage.setItem(key, JSON.stringify(saved));
      } catch {
        // The panels remain resizable when webview storage is unavailable.
      }
    },
  };
}

export function WorkspacePanels() {
  const inspectorOpen = useWorkspace((state) => state.inspectorOpen);
  const railLayout = usePanelLayout("why:rail-layout:v1", { rail: 27, workspace: 73 });
  const inspectorLayout = usePanelLayout("why:inspector-layout:v1", { diff: 70, inspector: 30 });

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
