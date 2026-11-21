import { CircleHelp, PanelRight, Plug, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tip } from "@/components/ui/overlays";
import { paletteKeys } from "@/hooks/use-workspace-shortcuts";
import { diffModes, selectDiffMode, selectWorktree } from "@/stores/workspace";
import { useWorkspace } from "@/stores/workspace-context";
import { BasePopover } from "./base-popover";
import { BrandMark } from "./brand-mark";
import { EditorSettingsPopover } from "./editor-settings-popover";

export function TopBar({ onHelp }: { onHelp: () => void }) {
  const worktree = useWorkspace(selectWorktree);
  const mode = useWorkspace(selectDiffMode);
  const setMode = useWorkspace((state) => state.setDiffMode);
  const inspectorOpen = useWorkspace((state) => state.inspectorOpen);
  const toggleInspector = useWorkspace((state) => state.toggleInspector);
  const setAgentHooksOpen = useWorkspace((state) => state.setAgentHooksOpen);
  const setPaletteOpen = useWorkspace((state) => state.setPaletteOpen);

  return (
    <header className="top-bar">
      <div className="brand">
        <span className="brand-icon">
          <BrandMark />
        </span>
        <h1 className="sr-only">WHY</h1>
      </div>
      <Button
        id="shell-command"
        aria-label="Search commands"
        aria-haspopup="dialog"
        aria-keyshortcuts="Meta+K Control+K"
        variant="ghost"
        size="sm"
        className="command-trigger"
        onClick={() => setPaletteOpen(true)}
      >
        <Search className="size-3.5" aria-hidden />
        <span className="search-label">Search</span>
        <kbd aria-hidden>{paletteKeys}</kbd>
      </Button>
      <div className="top-controls">
        <div className="mode-control" role="group" aria-label="Diff mode">
          {diffModes.map(({ mode: value, label, key }) => (
            <button
              key={value}
              type="button"
              aria-pressed={value === mode}
              aria-keyshortcuts={key}
              disabled={!worktree || (value === "vsBase" && !worktree.base)}
              title={
                value === "vsBase" && worktree && !worktree.base
                  ? "This worktree has no base ref"
                  : `${label} (${key})`
              }
              onClick={() => void setMode(value)}
            >
              {label}
            </button>
          ))}
        </div>
        {worktree && <BasePopover worktree={worktree} />}
        <Tip label="Agent hooks">
          <Button
            id="shell-agent-hooks"
            aria-label="Agent hooks"
            aria-haspopup="dialog"
            variant="ghost"
            size="sm"
            className="hook-trigger"
            onClick={(event) => {
              event.currentTarget.focus({ preventScroll: true });
              setAgentHooksOpen(true);
            }}
          >
            <Plug aria-hidden />
            <span>Agent hooks</span>
          </Button>
        </Tip>
        <EditorSettingsPopover />
        <Tip label="Keyboard shortcuts" shortcut="?">
          <Button
            id="shell-help"
            aria-label="Keyboard shortcuts"
            aria-keyshortcuts="?"
            aria-haspopup="dialog"
            variant="ghost"
            size="icon-sm"
            onClick={onHelp}
          >
            <CircleHelp aria-hidden />
          </Button>
        </Tip>
        <Tip label={inspectorOpen ? "Hide Why inspector" : "Show Why inspector"} shortcut="I">
          <Button
            id="shell-inspector"
            aria-label="Why inspector"
            aria-expanded={inspectorOpen}
            aria-controls={inspectorOpen ? "why-inspector" : undefined}
            aria-keyshortcuts="I"
            variant={inspectorOpen ? "secondary" : "ghost"}
            size="icon-sm"
            onClick={toggleInspector}
          >
            <PanelRight aria-hidden />
          </Button>
        </Tip>
      </div>
    </header>
  );
}
