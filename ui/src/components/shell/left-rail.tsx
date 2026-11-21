import { useEffect, useState } from "react";
import { useGroupRef, usePanelRef } from "react-resizable-panels";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { saveLayout, usePanelLayout } from "@/hooks/use-panel-layout";
import { ChangedFiles } from "./rail-files";
import { RailFooter } from "./rail-footer";
import { RailFilter } from "./rail-filter";
import { Repositories } from "./rail-repositories";

const layoutKey = "why:rail-sections:v1";
const collapsedKey = "why:rail-collapsed:v1";
const defaultLayout = { repos: 46, files: 54 };
/** A collapsed section is its header: keep in step with `--pane-header` in the CSS. */
const HEADER_PX = 40;
const MIN_PX = 120;

type Collapsed = { repos: boolean; files: boolean };

function readCollapsed(): Collapsed {
  try {
    const saved: unknown = JSON.parse(window.localStorage.getItem(collapsedKey) ?? "{}");
    if (typeof saved === "object" && saved !== null) {
      const repos = Reflect.get(saved, "repos") === true;
      const files = Reflect.get(saved, "files") === true;
      // Never both: the rail would be two headers and nothing else.
      if (!(repos && files)) return { repos, files };
    }
  } catch {
    // Fall through to "both open".
  }
  return { repos: false, files: false };
}

/**
 * The rail: a filter, then two sections (Repositories, Changed files) in a
 * vertical resizable split. Drag the hairline between them (or focus it and use
 * the arrow keys); double-click resets it. Each section's title folds it to its
 * header and the other takes the space. Sizes and folds are remembered.
 */
export function LeftRail() {
  const sizes = usePanelLayout(layoutKey, defaultLayout);
  const group = useGroupRef();
  const repos = usePanelRef();
  const files = usePanelRef();
  const [collapsed, setCollapsed] = useState(readCollapsed);

  // Re-apply a remembered fold once the panels exist.
  useEffect(() => {
    if (collapsed.repos) repos.current?.collapse();
    if (collapsed.files) files.current?.collapse();
    // Only on mount: later folds come from the user and are applied directly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function remember(next: Collapsed) {
    setCollapsed(next);
    try {
      window.localStorage.setItem(collapsedKey, JSON.stringify(next));
    } catch {
      // Keep the fold for this session.
    }
  }

  // A section can also fold or open by dragging the hairline past its minimum.
  function sync() {
    const next = {
      repos: repos.current?.isCollapsed() ?? false,
      files: files.current?.isCollapsed() ?? false,
    };
    if (next.repos !== collapsed.repos || next.files !== collapsed.files) remember(next);
  }

  function toggle(which: keyof Collapsed) {
    const [mine, other] = which === "repos" ? [repos, files] : [files, repos];
    if (collapsed[which]) {
      mine.current?.expand();
      remember({ ...collapsed, [which]: false });
    } else {
      // Folding the last open section would leave two bare headers: open the other first.
      if (collapsed[which === "repos" ? "files" : "repos"]) other.current?.expand();
      mine.current?.collapse();
      remember({ repos: which === "repos", files: which === "files" });
    }
  }

  function reset() {
    group.current?.setLayout(defaultLayout);
    saveLayout(layoutKey, defaultLayout);
    remember({ repos: false, files: false });
  }

  return (
    <nav aria-label="Review navigation" className="left-rail">
      <RailFilter />
      <div className="rail-sections">
        <ResizablePanelGroup id="rail-sections" orientation="vertical" groupRef={group} {...sizes}>
          <ResizablePanel
            id="repos"
            panelRef={repos}
            collapsible
            collapsedSize={HEADER_PX}
            minSize={MIN_PX}
            onResize={sync}
          >
            <Repositories expanded={!collapsed.repos} onToggle={() => toggle("repos")} />
          </ResizablePanel>
          <ResizableHandle
            aria-label="Resize repositories and changed files"
            title="Drag to resize, double-click to reset"
            onDoubleClick={reset}
          />
          <ResizablePanel
            id="files"
            panelRef={files}
            collapsible
            collapsedSize={HEADER_PX}
            minSize={MIN_PX}
            onResize={sync}
          >
            <ChangedFiles expanded={!collapsed.files} onToggle={() => toggle("files")} />
          </ResizablePanel>
        </ResizablePanelGroup>
      </div>
      <RailFooter />
    </nav>
  );
}
