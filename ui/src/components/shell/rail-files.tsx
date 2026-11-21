import { useEffect, useMemo, useRef } from "react";
import { Binary, FileWarning } from "lucide-react";
import type { FileChange } from "@/contracts";
import { useSlidingHighlight } from "@/hooks/use-sliding-highlight";
import { selectActiveWorktreeTitle, selectChanges } from "@/stores/workspace";
import { useEvidence, useSessions, useWorkspace } from "@/stores/workspace-context";
import { filesInTurn, matchesFile } from "@/components/sessions/timeline-model";
import { ErrorMessage, LoadingMessage } from "./async-message";
import { ChangeStatusBadge, LineCounts, displayPath } from "./change-labels";
import { RailSectionHeader } from "./rail-section";
import { fileMatches } from "./rail-model";
import { RowMarker } from "./rail-repositories";
import { AttributionMarker, ShellLeadsLine } from "./rail-attribution";
import {
  currentAttribution,
  evidenceKind,
  isNotExplained,
  notExplainedCount,
} from "./attribution-model";

function PathPart({ path }: { path: string }) {
  const nameIndex = path.lastIndexOf("/") + 1;
  return (
    <span className="path-part">
      <span className="path-directory">{path.slice(0, nameIndex)}</span>
      <span className="path-basename">{path.slice(nameIndex)}</span>
    </span>
  );
}

function FileRow({
  file,
  selected,
  touched,
}: {
  file: FileChange;
  selected: boolean;
  touched: boolean;
}) {
  const selectFile = useWorkspace((state) => state.selectFile);
  // Set by a live update that touched this file; the number changes each time,
  // so the overlay (keyed by it) remounts and its fade restarts.
  const flash = useWorkspace((state) => state.flashes.get(file.path));
  return (
    <button
      type="button"
      className="rail-row file-row"
      aria-current={selected || undefined}
      data-turn-match={touched || undefined}
      title={displayPath(file)}
      onClick={() => void selectFile(file.path)}
    >
      <ChangeStatusBadge status={file.status} />
      {touched && <span className="sr-only">Touched in selected turn</span>}
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
  const worktreeId = useWorkspace((state) => state.worktreeId);
  const turn = useSessions((state) =>
    state.worktreeId === worktreeId && state.timeline.status === "ok" && state.turnId !== null
      ? state.timeline.data.turns.find(({ id }) => id === state.turnId)
      : undefined,
  );
  const touched = useMemo(() => turn && filesInTurn(turn), [turn]);
  const retry = useWorkspace((state) => state.retryChanges);
  const filter = useWorkspace((state) => state.railFilter);
  const worktreeTitle = useWorkspace(selectActiveWorktreeTitle);
  const evidence = useEvidence((state) => state.attribution);
  const evidenceScope = useEvidence((state) => state.scope);
  const onlyUnexplained = useEvidence((state) => state.onlyUnexplained);
  const setOnlyUnexplained = useEvidence((state) => state.setOnlyUnexplained);
  // Only an answer for THIS worktree and comparison may label its rows
  // (the same guard the inspector's File tab and coverage bar use).
  const guarded = useMemo(
    () => currentAttribution(evidence, worktreeId, evidenceScope?.mode),
    [evidence, worktreeId, evidenceScope?.mode],
  );
  const attribution = useMemo(() => {
    const current = guarded.status === "ok" ? guarded.data : undefined;
    // A file whose name was not valid UTF-8 shares its display string with other
    // files, so it is never joined to evidence by path (the core leaves it unexplained).
    return (
      current && {
        byPath: new Map(
          current.files.filter((f) => !f.change.pathLossy).map((f) => [f.change.path, f]),
        ),
        current,
      }
    );
  }, [guarded]);
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
  const byText = matching.length > 0 ? matching : all;
  const unexplainedCount = attribution ? notExplainedCount(attribution.current.files) : 0;
  // "Show unexplained files" narrows to the files no captured edit explains.
  const files =
    onlyUnexplained && attribution
      ? byText.filter((file) =>
          isNotExplained(attribution.byPath.get(file.path) ?? { status: "unexplained" }),
        )
      : byText;
  const narrowed =
    (filter.trim() !== "" && matching.length > 0 && matching.length < all.length) ||
    (onlyUnexplained && attribution !== undefined);

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
        {changes.status === "ok" && attribution && (unexplainedCount > 0 || onlyUnexplained) && (
          <button
            type="button"
            className="evidence-filter"
            aria-pressed={onlyUnexplained}
            title="Show only files no captured agent edit explains"
            onClick={() => setOnlyUnexplained(!onlyUnexplained)}
          >
            Unexplained {unexplainedCount}
          </button>
        )}
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
            <>
              <ul
                id="changed-file-list"
                className="file-list"
                aria-label="Changed files"
                ref={rows}
              >
                <li className="slide-highlight" aria-hidden="true" ref={bar} />
                {files.map((file) => {
                  const item = file.pathLossy ? undefined : attribution?.byPath.get(file.path);
                  return (
                    <li
                      key={file.path}
                      className="file-item"
                      data-evidence={item && evidenceKind(item)}
                    >
                      <FileRow
                        file={file}
                        selected={file.path === selectedPath}
                        touched={!!touched && matchesFile(touched, file)}
                      />
                      {item && <AttributionMarker file={item} />}
                    </li>
                  );
                })}
              </ul>
              {files.length === 0 && onlyUnexplained && (
                <p className="rail-note">
                  {unexplainedCount === 0
                    ? "Every changed file has a captured agent edit."
                    : "No unexplained file matches the filter."}
                </p>
              )}
              {attribution && <ShellLeadsLine leads={attribution.current.shellLeads} />}
              {evidence.status === "error" && (
                <p className="rail-note">Attribution is unavailable right now.</p>
              )}
            </>
          ))}
      </div>
    </section>
  );
}
