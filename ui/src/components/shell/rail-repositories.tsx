import { useEffect, useRef, useState } from "react";
import {
  Archive,
  ArrowRight,
  ChevronRight,
  FolderGit2,
  Lock,
  MoreHorizontal,
  Plus,
  ShieldOff,
  SquareDashed,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import type { RepoId, Repository, Worktree } from "@/contracts";
import { Button } from "@/components/ui/button";
import {
  ConfirmContent,
  ConfirmRoot,
  MenuContent,
  MenuItem,
  MenuRoot,
  MenuTrigger,
  Tip,
} from "@/components/ui/overlays";
import { useNowSeconds } from "@/hooks/use-now";
import { useSlidingHighlight } from "@/hooks/use-sliding-highlight";
import { describeError } from "@/lib/error-text";
import { useWorkspace } from "@/stores/workspace-context";
import { ErrorMessage, LoadingMessage } from "./async-message";
import { BranchDivergence } from "./change-labels";
import { Collapsible } from "./collapsible";
import { FilterMarker } from "./filter-trust";
import {
  activityOf,
  formatRelative,
  groupWorktrees,
  repositoryMatches,
  sortWorktrees,
  worktreeLabels,
  worktreeMatches,
  type WorktreeLabel,
} from "./rail-model";
import { RailSectionHeader } from "./rail-section";

export function RowMarker({ label, Icon }: { label: string; Icon: typeof Lock }) {
  return (
    <span className="worktree-marker" title={label}>
      <Icon size={12} aria-hidden />
      <span className="sr-only">{label}</span>
    </span>
  );
}

/**
 * "This worktree could not be read": a muted warning with the reason in a
 * tooltip. It is not focusable (it sits inside the row's button); the reason is
 * also in the accessible name so it is not hover-only.
 */
function LoadErrorMarker({ error }: { error: NonNullable<Worktree["loadError"]> }) {
  const text = describeError(error);
  return (
    <Tip label={text} side="right">
      <span
        className="worktree-marker worktree-warning"
        role="img"
        aria-label={`Could not be read. ${text}`}
      >
        <TriangleAlert size={12} aria-hidden />
      </span>
    </Tip>
  );
}

function WorktreeRow({
  worktree,
  label,
  selected,
  now,
}: {
  worktree: Worktree;
  label: WorktreeLabel;
  selected: boolean;
  now: number;
}) {
  const select = useWorkspace((state) => state.selectWorktree);
  const seen = useWorkspace((state) => state.activity.get(worktree.id));
  // Changes each time the watcher sees an edit here; the ring (keyed by it) replays.
  const pulse = useWorkspace((state) => state.pulses.get(worktree.id));
  const state = worktree.loadError ? "error" : worktree.changeCount > 0 ? "changes" : "clean";
  const when = formatRelative(
    now,
    activityOf(worktree, new Map(seen === undefined ? [] : [[worktree.id, seen]])),
  );
  return (
    <button
      type="button"
      className="rail-row worktree-row"
      aria-current={selected || undefined}
      title={when ? `${worktree.path}\nActive ${when} ago` : worktree.path}
      onClick={() => void select(worktree)}
    >
      {/* Decorative: the same facts are in the count's label and the warning marker. */}
      <span className="wt-dot" data-state={state} aria-hidden="true">
        {pulse !== undefined && <span key={pulse} className="wt-pulse" />}
      </span>
      <span className="worktree-text">
        <span className="worktree-title">
          <span className="truncate">{label.primary}</span>{" "}
          {worktree.isMain && <RowMarker Icon={SquareDashed} label="Main" />}
          {worktree.isLocked && <RowMarker Icon={Lock} label="Locked" />}
          {worktree.isPrunable && <RowMarker Icon={Archive} label="Prunable" />}
          {worktree.loadError && <LoadErrorMarker error={worktree.loadError} />}
        </span>
        {(label.secondary || worktree.base) && (
          <span className="worktree-ref">
            {label.secondary && <span className="truncate">{label.secondary}</span>}
            {worktree.base && (
              <>
                <ArrowRight size={11} aria-hidden />
                <span className="truncate">{worktree.base.baseRef}</span>
              </>
            )}
          </span>
        )}
      </span>
      <span className="worktree-stats">
        <span className="stats-top">
          {when && <span className="worktree-time">{when}</span>}{" "}
          <span
            className="count-badge"
            data-zero={worktree.changeCount === 0 || undefined}
            aria-label={`${worktree.changeCount} changed files`}
          >
            {worktree.changeCount}
          </span>
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

function WorktreeList({
  repo,
  shown,
  clean,
  labels,
  activeWorktree,
  now,
}: {
  repo: Repository;
  shown: Worktree[];
  clean: Worktree[];
  labels: ReadonlyMap<string, WorktreeLabel>;
  activeWorktree: string | null;
  now: number;
}) {
  const list = useRef<HTMLUListElement>(null);
  const bar = useRef<HTMLLIElement>(null);
  const [cleanOpen, setCleanOpen] = useState(false);
  useSlidingHighlight(list, bar);
  const row = (worktree: Worktree) => {
    const label = labels.get(worktree.id);
    return label ? (
      <li key={worktree.id}>
        <WorktreeRow
          worktree={worktree}
          label={label}
          selected={activeWorktree === worktree.id}
          now={now}
        />
      </li>
    ) : null;
  };
  return (
    <ul className="worktree-list" aria-label={`Worktrees in ${repo.name}`} ref={list}>
      <li className="slide-highlight" aria-hidden="true" ref={bar} />
      {shown.map(row)}
      {clean.length > 0 && (
        <li className="clean-group">
          <button
            type="button"
            className="clean-toggle"
            aria-expanded={cleanOpen}
            onClick={() => setCleanOpen((open) => !open)}
          >
            <ChevronRight size={12} className="disclosure" aria-hidden />
            {clean.length} clean
          </button>
          <Collapsible open={cleanOpen}>
            <ul className="clean-list" aria-label={`Clean worktrees in ${repo.name}`}>
              {clean.map(row)}
            </ul>
          </Collapsible>
        </li>
      )}
    </ul>
  );
}

export function Repositories({ expanded, onToggle }: { expanded: boolean; onToggle: () => void }) {
  const repositories = useWorkspace((state) => state.repositories);
  const activeRepo = useWorkspace((state) => state.repoId);
  const activeWorktree = useWorkspace((state) => state.worktreeId);
  const worktreeLists = useWorkspace((state) => state.worktrees);
  const activity = useWorkspace((state) => state.activity);
  const filter = useWorkspace((state) => state.railFilter);
  const retryWorktrees = useWorkspace((state) => state.retryWorktrees);
  const loadRepositories = useWorkspace((state) => state.loadRepositories);
  const openAdd = useWorkspace((state) => state.setAddRepositoryOpen);
  const removeRepository = useWorkspace((state) => state.removeRepository);
  const revokeFilters = useWorkspace((state) => state.revokeFilters);
  const security = useWorkspace((state) => state.security);
  const [collapsedRepos, setCollapsedRepos] = useState(readCollapsedRepos);
  const [menuFor, setMenuFor] = useState<RepoId | null>(null);
  const [confirming, setConfirming] = useState(false);
  // Kept after the confirm closes so its text does not blank during the exit animation.
  const [target, setTarget] = useState<Repository | null>(null);
  const rows = useRef<HTMLDivElement>(null);
  const now = useNowSeconds();
  const filtering = filter.trim() !== "";

  // Scroll the selection into view when the SELECTION moves (a click, `[` / `]`),
  // not on every live update: a refresh must never yank the list away from where
  // the reader is.
  useEffect(() => {
    rows.current?.querySelector("[aria-current=true]")?.scrollIntoView?.({ block: "nearest" });
  }, [activeRepo, activeWorktree]);

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

  function askToRemove(repo: Repository) {
    setTarget(repo);
    setConfirming(true);
  }

  // Per repository: its worktrees, named, filtered, ordered and grouped.
  const entries =
    repositories.status === "ok"
      ? repositories.data.flatMap((repo) => {
          const list = worktreeLists.get(repo.id);
          const all = list?.status === "ok" ? list.data : [];
          const labels = worktreeLabels(all, repo);
          const repoHit = repositoryMatches(repo, filter);
          const matched = filtering
            ? all.filter((wt) => repoHit || worktreeMatches(wt, labels.get(wt.id), filter))
            : all;
          if (filtering && !repoHit && matched.length === 0) return [];
          const sorted = sortWorktrees(matched, labels, activity);
          return [
            { repo, list, all, labels, ...groupWorktrees(sorted, activeWorktree, filtering) },
          ];
        })
      : [];

  return (
    <section className="repository-section" aria-labelledby="repositories-heading">
      <RailSectionHeader
        id="repositories-heading"
        title="Repositories"
        expanded={expanded}
        onToggle={onToggle}
      >
        {repositories.status === "ok" && (
          <span className="rail-total">{repositories.data.length}</span>
        )}
        <Tip label="Add repository" shortcut="A">
          <Button
            id="add-repository-trigger"
            variant="ghost"
            size="icon-xs"
            aria-label="Add repository"
            aria-keyshortcuts="A"
            onClick={() => openAdd(true)}
          >
            <Plus aria-hidden />
          </Button>
        </Tip>
      </RailSectionHeader>
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
          ) : entries.length === 0 ? (
            <p className="rail-note">Nothing matches “{filter.trim()}”.</p>
          ) : (
            <ul className="repo-list">
              {entries.map(({ repo, list, all, labels, shown, clean }) => {
                const active = repo.id === activeRepo;
                // A filter opens every repository that has a match.
                const open = filtering || !collapsedRepos.has(repo.id);
                const guard = security.get(repo.id);
                // Offered while ANY approval remains: a repository that was trusted and then
                // gained one more (untrusted) driver is "blocked" as a whole, but the
                // old approval still lets WHY run the driver that was trusted.
                const filtersTrusted =
                  guard?.status === "ok" && guard.data.drivers.some((driver) => driver.trusted);
                return (
                  <li key={repo.id}>
                    <div
                      className="repo-head"
                      data-menu-open={menuFor === repo.id || undefined}
                      onContextMenu={(event) => {
                        event.preventDefault();
                        setMenuFor(repo.id);
                      }}
                    >
                      <button
                        type="button"
                        className="rail-row repository-row"
                        aria-expanded={open}
                        aria-current={active && !open ? true : undefined}
                        data-active={active}
                        title={repo.rootPath}
                        onClick={() => toggleRepo(repo.id)}
                      >
                        <ChevronRight size={13} className="disclosure" aria-hidden />
                        <FolderGit2 size={15} aria-hidden />
                        <span className="truncate">{repo.name}</span>
                        <FilterMarker repoId={repo.id} />
                        {list?.status === "ok" && (
                          <span className="repo-count" aria-hidden="true">
                            {all.length}
                          </span>
                        )}
                      </button>
                      <MenuRoot
                        open={menuFor === repo.id}
                        onOpenChange={(next) => setMenuFor(next ? repo.id : null)}
                      >
                        <MenuTrigger asChild>
                          <button
                            type="button"
                            className="row-action"
                            aria-label={`Actions for ${repo.name}`}
                            data-repo-actions={repo.id}
                          >
                            <MoreHorizontal size={14} aria-hidden />
                          </button>
                        </MenuTrigger>
                        <MenuContent>
                          {filtersTrusted && (
                            <MenuItem onSelect={() => void revokeFilters(repo)}>
                              <ShieldOff size={14} aria-hidden />
                              Stop trusting filter programs
                            </MenuItem>
                          )}
                          <MenuItem danger onSelect={() => askToRemove(repo)}>
                            <Trash2 size={14} aria-hidden />
                            Remove from WHY…
                          </MenuItem>
                        </MenuContent>
                      </MenuRoot>
                    </div>
                    <Collapsible open={open}>
                      {(!list || list.status === "loading") && (
                        <LoadingMessage>Loading worktrees…</LoadingMessage>
                      )}
                      {list?.status === "error" && (
                        <ErrorMessage
                          error={list.error}
                          retry={() => void retryWorktrees(repo.id)}
                        />
                      )}
                      {list?.status === "ok" &&
                        (all.length === 0 ? (
                          <p className="rail-note">No worktrees found.</p>
                        ) : (
                          <WorktreeList
                            repo={repo}
                            shown={shown}
                            clean={clean}
                            labels={labels}
                            activeWorktree={activeWorktree}
                            now={now}
                          />
                        ))}
                    </Collapsible>
                  </li>
                );
              })}
            </ul>
          ))}
      </div>
      <ConfirmRoot open={confirming} onOpenChange={setConfirming}>
        <ConfirmContent
          title={`Remove ${target?.name ?? "repository"} from WHY?`}
          description="This only removes it from WHY's list. Your files, branches and worktrees on disk are not touched, and you can add it again at any time."
          confirmLabel="Remove"
          onConfirm={() => target && void removeRepository(target)}
          onCloseFocus={() => {
            // Back to the repository's Actions button; if it is gone (removed), to Add.
            const actions = target
              ? document.querySelector<HTMLElement>(
                  `[data-repo-actions="${CSS.escape(target.id)}"]`,
                )
              : null;
            (actions ?? document.getElementById("add-repository-trigger"))?.focus();
          }}
        />
      </ConfirmRoot>
    </section>
  );
}
