import { useEffect, useRef, useState } from "react";
import {
  Archive,
  ArrowRight,
  Binary,
  ChevronDown,
  ChevronRight,
  FileWarning,
  FolderGit2,
  GitBranch,
  Lock,
  SquareDashed,
} from "lucide-react";
import type { FileChange, RepoId, Worktree } from "@/contracts";
import { selectChanges, selectWorktrees } from "@/stores/workspace";
import { useWorkspace } from "@/stores/workspace-context";
import { ErrorMessage, LoadingMessage } from "./async-message";
import {
  BranchDivergence,
  ChangeStatusBadge,
  LineCounts,
  branchLabel,
  displayPath,
  worktreeName,
} from "./change-labels";

function RowMarker({ label, Icon }: { label: string; Icon: typeof Lock }) {
  return (
    <span className="worktree-marker" title={label}>
      <Icon size={12} aria-hidden />
      <span className="sr-only">{label}</span>
    </span>
  );
}

function WorktreeRow({ worktree, selected }: { worktree: Worktree; selected: boolean }) {
  const select = useWorkspace((state) => state.selectWorktree);
  return (
    <button
      type="button"
      className="rail-row worktree-row"
      aria-current={selected || undefined}
      title={worktree.path}
      onClick={() => void select(worktree)}
    >
      <GitBranch size={14} className="worktree-icon" aria-hidden />
      <span className="worktree-text">
        <span className="worktree-title">
          <span className="truncate">{worktreeName(worktree)}</span>
          {worktree.isMain && <RowMarker Icon={SquareDashed} label="Main" />}
          {worktree.isLocked && <RowMarker Icon={Lock} label="Locked" />}
          {worktree.isPrunable && <RowMarker Icon={Archive} label="Prunable" />}
        </span>
        <span className="worktree-ref">
          <span className="truncate">{branchLabel(worktree)}</span>
          {worktree.base && (
            <>
              <ArrowRight size={11} aria-hidden />
              <span className="truncate">{worktree.base.baseRef}</span>
            </>
          )}
        </span>
      </span>
      <span className="worktree-stats">
        <span className="count-badge" aria-label={`${worktree.changeCount} changed files`}>
          {worktree.changeCount}
        </span>
        {worktree.base && <BranchDivergence base={worktree.base} />}
      </span>
    </button>
  );
}

const collapsedReposKey = "why:collapsed-repositories:v1";

function readCollapsedRepos(): ReadonlySet<RepoId> {
  try {
    const saved: unknown = JSON.parse(window.localStorage.getItem(collapsedReposKey) ?? "[]");
    if (Array.isArray(saved)) {
      return new Set(saved.filter((id): id is RepoId => typeof id === "string"));
    }
  } catch {
    // Start with every repository open if storage cannot be read.
  }
  return new Set();
}

function Repositories() {
  const repositories = useWorkspace((state) => state.repositories);
  const activeRepo = useWorkspace((state) => state.repoId);
  const activeWorktree = useWorkspace((state) => state.worktreeId);
  const activeWorktrees = useWorkspace(selectWorktrees);
  const worktreeLists = useWorkspace((state) => state.worktrees);
  const retryWorktrees = useWorkspace((state) => state.retryWorktrees);
  const loadRepositories = useWorkspace((state) => state.loadRepositories);
  const [collapsedRepos, setCollapsedRepos] = useState(readCollapsedRepos);
  const rows = useRef<HTMLDivElement>(null);

  useEffect(() => {
    rows.current?.querySelector("[aria-current=true]")?.scrollIntoView?.({ block: "nearest" });
  }, [activeRepo, activeWorktree, activeWorktrees]);

  function toggleRepo(id: RepoId) {
    const next = new Set(collapsedRepos);
    if (!next.delete(id)) next.add(id);
    setCollapsedRepos(next);
    try {
      window.localStorage.setItem(collapsedReposKey, JSON.stringify([...next]));
    } catch {
      // Keep the disclosure state for this session if storage is unavailable.
    }
  }

  return (
    <section className="repository-section" aria-labelledby="repositories-heading">
      <div className="rail-heading">
        <h2 id="repositories-heading">Repositories</h2>
        {repositories.status === "ok" && (
          <span className="rail-total">{repositories.data.length}</span>
        )}
      </div>
      <div className="rail-scroll" ref={rows}>
        {(repositories.status === "loading" || repositories.status === "idle") && (
          <LoadingMessage>Loading repositories…</LoadingMessage>
        )}
        {repositories.status === "error" && (
          <ErrorMessage error={repositories.error} retry={() => void loadRepositories()} />
        )}
        {repositories.status === "ok" &&
          (repositories.data.length === 0 ? (
            <p className="rail-note">No repositories registered.</p>
          ) : (
            <ul className="repo-list">
              {repositories.data.map((repo) => {
                const active = repo.id === activeRepo;
                const expanded = !collapsedRepos.has(repo.id);
                const worktrees = worktreeLists.get(repo.id);
                return (
                  <li key={repo.id}>
                    <button
                      type="button"
                      className="rail-row repository-row"
                      aria-expanded={expanded}
                      aria-current={active && !expanded ? true : undefined}
                      data-active={active}
                      title={repo.rootPath}
                      onClick={() => toggleRepo(repo.id)}
                    >
                      {expanded ? (
                        <ChevronDown size={13} aria-hidden />
                      ) : (
                        <ChevronRight size={13} aria-hidden />
                      )}
                      <FolderGit2 size={15} aria-hidden />
                      <span className="truncate">{repo.name}</span>
                    </button>
                    {expanded && (
                      <>
                        {(!worktrees || worktrees.status === "loading") && (
                          <LoadingMessage>Loading worktrees…</LoadingMessage>
                        )}
                        {worktrees?.status === "error" && (
                          <ErrorMessage
                            error={worktrees.error}
                            retry={() => void retryWorktrees(repo.id)}
                          />
                        )}
                        {worktrees?.status === "ok" &&
                          (worktrees.data.length === 0 ? (
                            <p className="rail-note">No worktrees found.</p>
                          ) : (
                            <ul className="worktree-list" aria-label={`Worktrees in ${repo.name}`}>
                              {worktrees.data.map((worktree) => (
                                <li key={worktree.id}>
                                  <WorktreeRow
                                    worktree={worktree}
                                    selected={activeWorktree === worktree.id}
                                  />
                                </li>
                              ))}
                            </ul>
                          ))}
                      </>
                    )}
                  </li>
                );
              })}
            </ul>
          ))}
      </div>
    </section>
  );
}

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
    </button>
  );
}

function ChangedFiles() {
  const changes = useWorkspace(selectChanges);
  const selectedPath = useWorkspace((state) => state.filePath);
  const retry = useWorkspace((state) => state.retryChanges);
  const rows = useRef<HTMLUListElement>(null);

  useEffect(() => {
    const list = rows.current;
    const selection = list?.querySelector<HTMLButtonElement>("[aria-current=true]");
    selection?.scrollIntoView?.({ block: "nearest" });
    if (list?.contains(document.activeElement)) selection?.focus({ preventScroll: true });
  }, [selectedPath, changes]);

  return (
    <section className="files-section" aria-labelledby="files-heading">
      <div className="rail-heading">
        <h2 id="files-heading">Changed files</h2>
        {changes.status === "ok" && <span className="rail-total">{changes.data.length}</span>}
      </div>
      <div className="rail-scroll">
        {changes.status === "idle" && <p className="rail-note">Select a worktree.</p>}
        {changes.status === "loading" && <LoadingMessage>Loading changes…</LoadingMessage>}
        {changes.status === "error" && (
          <ErrorMessage error={changes.error} retry={() => void retry()} />
        )}
        {changes.status === "ok" &&
          (changes.data.length === 0 ? (
            <p className="rail-note">No changes in this mode.</p>
          ) : (
            <ul className="file-list" aria-label="Changed files" ref={rows}>
              {changes.data.map((file) => (
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

export function LeftRail() {
  return (
    <nav aria-label="Review navigation" className="left-rail">
      <Repositories />
      <ChangedFiles />
    </nav>
  );
}
