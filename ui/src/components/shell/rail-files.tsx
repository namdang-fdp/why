import { useEffect, useRef } from "react";
import { Binary, FileWarning } from "lucide-react";
import type { FileChange } from "@/contracts";
import { useSlidingHighlight } from "@/hooks/use-sliding-highlight";
import { selectActiveWorktreeTitle, selectChanges } from "@/stores/workspace";
import { useWorkspace } from "@/stores/workspace-context";
import { ErrorMessage, LoadingMessage } from "./async-message";
import { ChangeStatusBadge, LineCounts, displayPath } from "./change-labels";
import { RailSectionHeader } from "./rail-section";
import { fileMatches } from "./rail-model";
import { RowMarker } from "./rail-repositories";

function PathPart({ path }: { path: string }) {
  const nameIndex = path.lastIndexOf("/") + 1;
  return (
    <span className="path-part">
      <span className="path-directory">{path.slice(0, nameIndex)}</span>
      <span className="path-basename">{path.slice(nameIndex)}</span>
    </span>
  );
}

function FileRow({ file, selected }: { file: FileChange; selected: boolean }) {
  const selectFile = useWorkspace((state) => state.selectFile);
  // Set by a live update that touched this file; the number changes each time,
  // so the overlay (keyed by it) remounts and its fade restarts.
  const flash = useWorkspace((state) => state.flashes.get(file.path));
  return (
    <button
      type="button"
      className="rail-row file-row"
      aria-current={selected || undefined}
      title={displayPath(file)}
      onClick={() => void selectFile(file.path)}
    >
      <ChangeStatusBadge status={file.status} />
      <span className="file-path">
        {file.oldPath && (
          <span className="file-old-path">
            <PathPart path={file.oldPath} /> <span aria-hidden>→</span>
          </span>
        )}
        <PathPart path={file.path} />
      </span>
      <span className="file-markers">
        {file.binary && <RowMarker Icon={Binary} label="Binary file" />}
        {file.pathLossy && <RowMarker Icon={FileWarning} label="Lossy path" />}
        <LineCounts file={file} />
      </span>
      {flash !== undefined && <span key={flash} className="row-flash" aria-hidden="true" />}
    </button>
  );
}

export function ChangedFiles({ expanded, onToggle }: { expanded: boolean; onToggle: () => void }) {
  const changes = useWorkspace(selectChanges);
  const selectedPath = useWorkspace((state) => state.filePath);
  const retry = useWorkspace((state) => state.retryChanges);
  const filter = useWorkspace((state) => state.railFilter);
  const worktreeTitle = useWorkspace(selectActiveWorktreeTitle);
  const rows = useRef<HTMLUListElement>(null);
  const bar = useRef<HTMLLIElement>(null);
  useSlidingHighlight(rows, bar);

  // Only when the selection moves or the list first appears (not on every live update).
  const listed = changes.status === "ok";
  useEffect(() => {
    const list = rows.current;
    const selection = list?.querySelector<HTMLButtonElement>("[aria-current=true]");
    selection?.scrollIntoView?.({ block: "nearest" });
    if (list?.contains(document.activeElement)) selection?.focus({ preventScroll: true });
  }, [selectedPath, listed]);

  // The filter is shared with the repository list. If it names files, narrow the
  // list to them; if it names a worktree (so no file matches), keep every file
  // rather than emptying the list the reader is looking at.
  const all = changes.status === "ok" ? changes.data : [];
  const matching = all.filter((file) => fileMatches(file, filter));
  const files = matching.length > 0 ? matching : all;
  const narrowed = filter.trim() !== "" && matching.length > 0 && matching.length < all.length;

  return (
    <section className="files-section" aria-labelledby="files-heading">
      <RailSectionHeader
        id="files-heading"
        title="Changed files"
        expanded={expanded}
        onToggle={onToggle}
        context={worktreeTitle}
        contextTitle={worktreeTitle ? `Changes in ${worktreeTitle}` : undefined}
      >
        {changes.status === "ok" && (
          <span className="rail-total">
            {narrowed ? `${files.length}/${changes.data.length}` : changes.data.length}
          </span>
        )}
      </RailSectionHeader>
      <div className="rail-scroll">
        {changes.status === "idle" && <p className="rail-note">Select a worktree.</p>}
        {changes.status === "loading" && <LoadingMessage>Loading changes…</LoadingMessage>}
        {changes.status === "error" &&
          (changes.error.kind === "untrustedFilters" ? (
            // Not a failure to retry: the main pane explains it and asks.
            <p className="rail-note">Blocked until you review this repository's filter programs.</p>
          ) : (
            <ErrorMessage error={changes.error} retry={() => void retry()} />
          ))}
        {changes.status === "ok" &&
          (changes.data.length === 0 ? (
            <p className="rail-note">No changes in this mode.</p>
          ) : (
            <ul id="changed-file-list" className="file-list" aria-label="Changed files" ref={rows}>
              <li className="slide-highlight" aria-hidden="true" ref={bar} />
              {files.map((file) => (
                <li key={file.path}>
                  <FileRow file={file} selected={file.path === selectedPath} />
                </li>
              ))}
            </ul>
          ))}
      </div>
    </section>
  );
}
