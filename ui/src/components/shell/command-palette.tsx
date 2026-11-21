import { useContext, useEffect, useLayoutEffect, useRef } from "react";
import {
  ChevronDown,
  ChevronUp,
  Columns2,
  FileDiff,
  FolderGit2,
  GitBranch,
  History,
  Palette,
  PanelRight,
  Plug,
  Plus,
  Settings2,
  Type,
} from "lucide-react";
import type { DiffMode, FileChange, Repository, Worktree } from "@/contracts";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";
import { useDialogFocus } from "@/hooks/use-dialog-focus";
import {
  changesKey,
  diffModes,
  selectActiveFile,
  selectChanges,
  selectWorktree,
} from "@/stores/workspace";
import { useStore } from "zustand";
import { editorSettingsStore, themeLabels, type EditorTheme } from "@/stores/editor-settings";
import { WorkspaceContext, useSessions, useWorkspace } from "@/stores/workspace-context";
import { focusInspector, moveTurn } from "@/components/sessions/session-controls";
import { filesInTurn, matchesFile } from "@/components/sessions/timeline-model";
import { ErrorMessage, LoadingMessage } from "./async-message";
import { branchLabel, displayPath, worktreeName } from "./change-labels";

type SearchableFile = { file: FileChange; modes: DiffMode[] };

function filterCommands(_id: string, search: string, keywords: string[] = []) {
  const query = search.trim().toLowerCase();
  if (!query) return 1;

  const fields = keywords.map((keyword) => keyword.toLowerCase());
  let best = 0;
  for (const [index, field] of fields.entries()) {
    const weight = index === 0 ? 1 : 0.85;
    if (field === query) best = Math.max(best, weight);
    else if (field.startsWith(query)) best = Math.max(best, 0.9 * weight);
    else if (field.includes(query)) best = Math.max(best, 0.75 * weight);
  }
  if (best > 0) return best;

  const terms = query.split(/\s+/);
  return terms.every((term) => fields.some((field) => field.includes(term))) ? 0.5 : 0;
}

function validModes(worktree: Worktree) {
  // A worktree that failed to load has no files to search (its error is shown in the rail).
  if (worktree.loadError) return [];
  return diffModes.filter(({ mode }) => mode !== "vsBase" || worktree.base);
}

function PaletteContents({
  close,
  afterClose,
}: {
  close: () => void;
  afterClose: (action: () => void) => void;
}) {
  const workspace = useContext(WorkspaceContext);
  const repositories = useWorkspace((state) => state.repositories);
  const worktrees = useWorkspace((state) => state.worktrees);
  const changes = useWorkspace((state) => state.changes);
  const activeWorktree = useWorkspace(selectWorktree);
  const activeFile = useWorkspace(selectActiveFile);
  const hasMatchingTurn = useSessions(
    (state) =>
      !!activeFile &&
      state.worktreeId === activeWorktree?.id &&
      state.timeline.status === "ok" &&
      state.timeline.data.turns.some((turn) => matchesFile(filesInTurn(turn), activeFile)),
  );
  const activeChanges = useWorkspace(selectChanges);
  const inspectorOpen = useWorkspace((state) => state.inspectorOpen);
  const selectRepo = useWorkspace((state) => state.selectRepository);
  const selectWorktreeAction = useWorkspace((state) => state.selectWorktree);
  const jumpToFile = useWorkspace((state) => state.jumpToFile);
  const loadFiles = useWorkspace((state) => state.loadPaletteFiles);
  const setMode = useWorkspace((state) => state.setDiffMode);
  const toggleInspector = useWorkspace((state) => state.toggleInspector);
  const toggleLayout = useWorkspace((state) => state.toggleDiffLayout);
  const moveChange = useWorkspace((state) => state.moveChange);
  const openAdd = useWorkspace((state) => state.setAddRepositoryOpen);
  const openAgentHooks = useWorkspace((state) => state.setAgentHooksOpen);
  const openEditorSettings = useWorkspace((state) => state.setEditorSettingsOpen);
  const editor = useStore(editorSettingsStore, (state) => state.settings);
  const updateEditor = useStore(editorSettingsStore, (state) => state.update);

  // Load every list nobody has asked for yet. Re-run when `changes` changes too: a
  // live update drops the lists it did not refresh, and they must come back while
  // the palette is open. Only MISSING keys are requested, so a failed list (which
  // stays cached as an error) is not retried in a loop.
  useEffect(() => {
    for (const list of worktrees.values()) {
      if (list.status !== "ok") continue;
      for (const worktree of list.data) {
        for (const { mode } of validModes(worktree)) {
          if (!changes.has(changesKey(worktree.id, mode))) void loadFiles(worktree, mode);
        }
      }
    }
  }, [worktrees, changes, loadFiles]);

  const repos: Repository[] = repositories.status === "ok" ? repositories.data : [];
  const visibleWorktrees = repos.flatMap((repo) => {
    const list = worktrees.get(repo.id);
    return list?.status === "ok" ? list.data.map((worktree) => ({ repo, worktree })) : [];
  });
  const errors = repos.flatMap((repo) => {
    const list = worktrees.get(repo.id);
    return list?.status === "error"
      ? [{ key: repo.id, context: repo.name, error: list.error }]
      : [];
  });
  if (repositories.status === "error") {
    errors.push({ key: "repositories", context: "Repositories", error: repositories.error });
  }

  const filesByWorktree = visibleWorktrees.map(({ repo, worktree }) => {
    const entries = new Map<string, SearchableFile>();
    let loading = false;
    for (const { mode, label } of validModes(worktree)) {
      const key = changesKey(worktree.id, mode);
      const state = changes.get(key);
      if (!state || state.status === "loading") loading = true;
      if (state?.status === "error") {
        errors.push({ key, context: `${worktreeName(worktree)} · ${label}`, error: state.error });
      }
      if (state?.status !== "ok") continue;
      for (const file of state.data) {
        const existing = entries.get(file.path);
        if (existing) existing.modes.push(mode);
        else entries.set(file.path, { file, modes: [mode] });
      }
    }
    return { repo, worktree, entries, loading };
  });
  const loading =
    repositories.status === "loading" ||
    repos.some((repo) => worktrees.get(repo.id)?.status === "loading") ||
    filesByWorktree.some((worktree) => worktree.loading);

  function run(action: () => void) {
    return () => {
      close();
      action();
    };
  }

  return (
    <>
      <CommandInput
        aria-label="Search repositories, worktrees, files, and actions"
        placeholder="Search repositories, worktrees, files, and actions…"
      />
      <CommandList className="palette-list">
        {loading && <LoadingMessage>Loading search results…</LoadingMessage>}
        {errors.map(({ key, ...result }) => (
          <ErrorMessage key={key} {...result} />
        ))}
        {!loading && <CommandEmpty>No matching commands or files.</CommandEmpty>}
        <CommandGroup heading="Actions">
          <CommandItem
            value="action-agent-hooks"
            keywords={["Set up agent hooks", "install uninstall", "collector", "Claude Code Codex"]}
            onSelect={() =>
              afterClose(() => {
                document.getElementById("shell-command")?.focus();
                openAgentHooks(true);
              })
            }
          >
            <Plug aria-hidden />
            Set up agent hooks
          </CommandItem>
          <CommandItem
            value="action-show-sessions"
            keywords={["Show sessions", "Why inspector", "agent timeline"]}
            disabled={!activeWorktree}
            onSelect={() =>
              afterClose(() => {
                void workspace.getState().sessions.getState().selectSession(null);
                focusInspector(workspace);
              })
            }
          >
            <History aria-hidden />
            Show sessions
          </CommandItem>
          <CommandItem
            value="action-next-matching-turn"
            keywords={["Next turn touching this file", "Why inspector", "session timeline"]}
            disabled={!hasMatchingTurn}
            onSelect={() =>
              afterClose(() => focusInspector(workspace, () => moveTurn(workspace, 1, true)))
            }
          >
            <ChevronDown aria-hidden />
            Next turn touching this file
          </CommandItem>
          {diffModes.map(({ mode, label, key }) => (
            <CommandItem
              key={mode}
              value={`action-${mode}`}
              keywords={[label, "diff mode", key]}
              disabled={!activeWorktree || (mode === "vsBase" && !activeWorktree.base)}
              onSelect={run(() => void setMode(mode))}
            >
              <FileDiff aria-hidden />
              {label}
              <CommandShortcut>
                <kbd>{key}</kbd>
              </CommandShortcut>
            </CommandItem>
          ))}
          <CommandItem
            value="action-add-repository"
            keywords={["Add repository", "open folder", "register", "a"]}
            onSelect={run(() => openAdd(true))}
          >
            <Plus aria-hidden />
            Add repository…
            <CommandShortcut>
              <kbd>a</kbd>
            </CommandShortcut>
          </CommandItem>
          <CommandItem
            value="action-editor-settings"
            keywords={["Editor settings", "font", "theme", "whitespace", "line height"]}
            onSelect={run(() => openEditorSettings(true))}
          >
            <Settings2 aria-hidden />
            Editor settings…
          </CommandItem>
          <CommandItem
            value="action-font-larger"
            keywords={["Editor: increase font size", "bigger", "zoom in", "font"]}
            onSelect={run(() => updateEditor({ fontSize: editor.fontSize + 1 }))}
          >
            <Type aria-hidden />
            Editor: increase font size
          </CommandItem>
          <CommandItem
            value="action-font-smaller"
            keywords={["Editor: decrease font size", "smaller", "zoom out", "font"]}
            onSelect={run(() => updateEditor({ fontSize: editor.fontSize - 1 }))}
          >
            <Type aria-hidden />
            Editor: decrease font size
          </CommandItem>
          <CommandItem
            value="action-ligatures"
            keywords={["Editor: toggle ligatures", "font ligatures"]}
            onSelect={run(() => updateEditor({ ligatures: !editor.ligatures }))}
          >
            <Type aria-hidden />
            Editor: {editor.ligatures ? "turn off" : "turn on"} ligatures
          </CommandItem>
          {(Object.keys(themeLabels) as EditorTheme[]).map((theme) => (
            <CommandItem
              key={theme}
              value={`action-theme-${theme}`}
              keywords={[`Editor theme: ${themeLabels[theme]}`, "theme", "colors"]}
              disabled={editor.theme === theme}
              onSelect={run(() => updateEditor({ theme }))}
            >
              <Palette aria-hidden />
              Editor theme: {themeLabels[theme]}
            </CommandItem>
          ))}
          <CommandItem
            value="action-diff-layout"
            keywords={["Toggle unified/split", "diff layout", "u"]}
            onSelect={run(toggleLayout)}
          >
            <Columns2 aria-hidden />
            Toggle unified/split
            <CommandShortcut>
              <kbd>u</kbd>
            </CommandShortcut>
          </CommandItem>
          <CommandItem
            value="action-next-change"
            keywords={["Next change", "hunk", "n"]}
            disabled={activeChanges.status !== "ok" || activeChanges.data.length === 0}
            onSelect={run(() => void moveChange("next"))}
          >
            <ChevronDown aria-hidden />
            Next change
            <CommandShortcut>
              <kbd>n</kbd>
            </CommandShortcut>
          </CommandItem>
          <CommandItem
            value="action-previous-change"
            keywords={["Previous change", "hunk", "p"]}
            disabled={!activeFile}
            onSelect={run(() => void moveChange("previous"))}
          >
            <ChevronUp aria-hidden />
            Previous change
            <CommandShortcut>
              <kbd>p</kbd>
            </CommandShortcut>
          </CommandItem>
          <CommandItem
            value="action-inspector"
            keywords={["toggle inspector", "why", inspectorOpen ? "hide" : "show"]}
            onSelect={run(toggleInspector)}
          >
            <PanelRight aria-hidden />
            {inspectorOpen ? "Hide Why inspector" : "Show Why inspector"}
            <CommandShortcut>
              <kbd>i</kbd>
            </CommandShortcut>
          </CommandItem>
        </CommandGroup>
        {repos.length > 0 && (
          <CommandGroup heading="Repositories">
            {repos.map((repo) => (
              <CommandItem
                key={repo.id}
                value={`repository-${repo.id}`}
                keywords={[repo.name, repo.rootPath]}
                onSelect={run(() => void selectRepo(repo.id))}
              >
                <FolderGit2 aria-hidden /> {repo.name}
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {visibleWorktrees.length > 0 && (
          <CommandGroup heading="Worktrees">
            {visibleWorktrees.map(({ repo, worktree }) => (
              <CommandItem
                key={worktree.id}
                value={`worktree-${worktree.id}`}
                keywords={[worktreeName(worktree), worktree.path, repo.name, branchLabel(worktree)]}
                onSelect={run(() => void selectWorktreeAction(worktree))}
              >
                <GitBranch aria-hidden />
                <span className="truncate">{worktreeName(worktree)}</span>
                <span className="palette-context truncate">{repo.name}</span>
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {filesByWorktree.some(({ entries }) => entries.size > 0) && (
          <CommandGroup heading="Files">
            {filesByWorktree.flatMap(({ repo, worktree, entries }) =>
              Array.from(entries.values(), ({ file, modes }) => (
                <CommandItem
                  key={JSON.stringify([worktree.id, file.path])}
                  value={JSON.stringify(["file", worktree.id, file.path])}
                  keywords={[
                    file.path.slice(file.path.lastIndexOf("/") + 1),
                    file.path,
                    file.oldPath ?? "",
                    worktreeName(worktree),
                    repo.name,
                  ]}
                  onSelect={run(() => void jumpToFile(worktree, file.path, modes))}
                >
                  <FileDiff aria-hidden />
                  <span className="palette-path truncate" title={displayPath(file)}>
                    {displayPath(file)}
                  </span>
                  <span className="palette-context truncate">{worktreeName(worktree)}</span>
                </CommandItem>
              )),
            )}
          </CommandGroup>
        )}
      </CommandList>
      <div className="palette-footer">
        <kbd>↑</kbd>
        <kbd>↓</kbd> Navigate <kbd>↵</kbd> Select <kbd>Esc</kbd> Close
      </div>
    </>
  );
}

export function CommandPalette() {
  const open = useWorkspace((state) => state.paletteOpen);
  const setOpen = useWorkspace((state) => state.setPaletteOpen);
  const focus = useDialogFocus("shell-command");
  const pending = useRef<(() => void) | null>(null);
  useLayoutEffect(() => {
    if (open) pending.current = null;
  }, [open]);
  return (
    <CommandDialog
      title="Command palette"
      description="Find a repository, worktree, or file, or run a workspace action."
      className="palette-dialog shadow-none"
      filter={filterCommands}
      showCloseButton={false}
      open={open}
      onOpenChange={setOpen}
      onOpenAutoFocus={focus.onOpenAutoFocus}
      onCloseAutoFocus={(event) => {
        const action = pending.current;
        pending.current = null;
        if (!action) {
          focus.onCloseAutoFocus(event);
          return;
        }
        event.preventDefault();
        action();
      }}
    >
      {open && (
        <PaletteContents
          close={() => setOpen(false)}
          afterClose={(action) => {
            pending.current = action;
            setOpen(false);
          }}
        />
      )}
    </CommandDialog>
  );
}
