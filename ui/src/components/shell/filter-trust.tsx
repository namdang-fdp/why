import { useEffect, useState } from "react";
import { Shield, ShieldAlert, ShieldCheck } from "lucide-react";
import type {
  FilterDriverInfo,
  FilterScope,
  RepoId,
  Repository,
  RepositorySecurity,
} from "@/contracts";
import { Button } from "@/components/ui/button";
import { ConfirmContent, ConfirmRoot, Tip } from "@/components/ui/overlays";
import { commandParts } from "@/lib/command-text";
import { useWorkspace } from "@/stores/workspace-context";
import { ErrorMessage, LoadingMessage } from "./async-message";

/*
 * Git filter programs (WHY-39). A repository's own config can name programs
 * that git runs while it reads files, so WHY refuses to read such a repository
 * until the user has looked at them and said yes. The core does the refusing;
 * this file only explains it and records the user's answer. The wording is
 * deliberately calm: nothing bad has happened, nothing was run.
 */

const scopeText: Record<FilterScope, string> = {
  system: "system git config",
  global: "your git config",
  local: "this repository's config",
  worktree: "this worktree's config",
  command: "the environment WHY started in",
  unknown: "an unrecognised git config",
};

/** "from .git/config" (the file) and which layer of git config that is. */
function whereFrom(driver: FilterDriverInfo) {
  const layer = scopeText[driver.scope];
  return driver.origin ? `from ${driver.origin} (${layer})` : `from ${layer}`;
}

function programs(driver: FilterDriverInfo) {
  return [
    ["clean", driver.clean],
    ["process", driver.process],
    ["smudge", driver.smudge],
  ] as const;
}

/**
 * A command shown exactly: whitespace kept, control and invisible characters
 * spelled out (see `lib/command-text.ts`). Never truncated.
 */
function Command({ text }: { text: string }) {
  return (
    <code className="filter-command">
      {commandParts(text).map((part, index) =>
        part.escaped ? (
          <span key={index} className="filter-escape">
            {part.text}
          </span>
        ) : (
          part.text
        ),
      )}
    </code>
  );
}

function DriverCard({ driver }: { driver: FilterDriverInfo }) {
  return (
    <li className="filter-driver">
      <div className="filter-driver-head">
        <strong>{driver.name}</strong>
        <span className="filter-driver-origin">{whereFrom(driver)}</span>
        {driver.trusted && <span className="filter-driver-chip">Trusted earlier</span>}
      </div>
      <dl className="filter-programs">
        {programs(driver).map(([kind, command]) =>
          command ? (
            <div key={kind}>
              <dt>{kind}</dt>
              <dd>
                <Command text={command} />
              </dd>
            </div>
          ) : null,
        )}
      </dl>
      {!driver.referenced && (
        <p className="filter-driver-note">
          No <code>.gitattributes</code> at the top of the repository selects it. A file in a
          subfolder still might.
        </p>
      )}
    </li>
  );
}

/**
 * Shown in place of the diff when the selected repository's filter programs
 * are untrusted. Reads the core's own security report, so what is listed is
 * what trusting would cover.
 */
export function FilterTrustPanel({ repo }: { repo: Repository }) {
  const security = useWorkspace((state) => state.security.get(repo.id));
  const loadSecurity = useWorkspace((state) => state.loadSecurity);
  const trustFilters = useWorkspace((state) => state.trustFilters);
  // The report as it was when the user opened the confirmation. THIS is what is
  // shown in the dialog and what is trusted: a refresh that arrives while the
  // dialog is open must not slip a different command under a name the user
  // already read (see the effect below).
  const [review, setReview] = useState<RepositorySecurity | null>(null);
  const [kept, setKept] = useState(false);
  const [busy, setBusy] = useState(false);

  // The core just refused to read this repository, so ask for the truth now:
  // a cached report may still say "clear" (a driver that arrived unseen).
  useEffect(() => {
    void loadSecurity(repo.id, true);
  }, [loadSecurity, repo.id]);

  // Derived, not an effect: a real, different report (a null `live` is just
  // "still loading") makes the review invalid. The dialog closes, the notice
  // below appears, and the next click starts a fresh review.
  const live = security?.status === "ok" ? security.data.fingerprint : null;
  const reviewValid = review !== null && (live === null || live === review.fingerprint);
  const changed = review !== null && !reviewValid;

  if (!security || security.status === "idle" || security.status === "loading") {
    return <LoadingMessage>Checking git filter programs…</LoadingMessage>;
  }
  if (security.status === "error") {
    return <ErrorMessage error={security.error} retry={() => void loadSecurity(repo.id, true)} />;
  }

  const report = security.data;
  const needing = report.drivers.filter((driver) => driver.needsTrust);
  if (needing.length === 0) {
    // Trusted (or the config changed) between the refusal and this report: the
    // store is already re-reading, so there is nothing to ask.
    return <LoadingMessage>Reading changes…</LoadingMessage>;
  }

  if (kept) {
    return (
      <div className="filter-panel" data-compact>
        <ShieldAlert size={20} aria-hidden />
        <h3>Kept blocked</h3>
        <p>
          WHY is not reading <strong>{repo.name}</strong>, and nothing from it was run.
        </p>
        <Button variant="ghost" size="sm" onClick={() => setKept(false)}>
          Review again
        </Button>
      </div>
    );
  }

  const reviewed = review?.drivers.filter((driver) => driver.needsTrust) ?? [];

  async function trust(snapshot: RepositorySecurity) {
    setBusy(true);
    // The snapshot's fingerprint, never the latest one: the core compares it
    // with what it finds and refuses if the configuration is not the one shown.
    await trustFilters(repo, snapshot.fingerprint);
    setBusy(false);
  }

  return (
    <div className="filter-panel" role="region" aria-labelledby="filter-panel-title">
      <span className="filter-panel-icon" aria-hidden>
        <ShieldAlert size={22} />
      </span>
      <h3 id="filter-panel-title">Blocked until you review it</h3>
      <p className="filter-panel-lead">
        <strong>{repo.name}</strong> defines{" "}
        {needing.length === 1 ? "a git filter program" : "git filter programs"}. Git runs{" "}
        {needing.length === 1 ? "it" : "these"} while it reads files, so WHY would run code that
        came from this repository. Nothing has been read or run yet. Only trust{" "}
        {needing.length === 1 ? "it" : "them"} if you know where{" "}
        {needing.length === 1 ? "it" : "they"} came from.
      </p>

      {changed && (
        <p className="filter-panel-changed" role="status">
          The filter programs changed while you were reviewing them. Look at them again before
          trusting.
        </p>
      )}

      <ul className="filter-drivers" aria-label="Filter programs this repository defines">
        {needing.map((driver) => (
          <DriverCard key={driver.fingerprint} driver={driver} />
        ))}
      </ul>

      <p className="filter-panel-caution">
        Trusting covers exactly the commands shown. If a command points at a file inside this
        repository, that file can change later and WHY will still run it.
      </p>

      <div className="filter-panel-actions">
        <Button disabled={busy} onClick={() => setReview(report)}>
          Trust and continue
        </Button>
        <Button variant="ghost" onClick={() => setKept(true)}>
          Keep blocked
        </Button>
      </div>

      <ConfirmRoot
        open={reviewValid}
        onOpenChange={(open) => {
          if (!open) setReview(null);
        }}
      >
        <ConfirmContent
          title={`Trust ${repo.name}'s filter programs?`}
          description={`WHY will run ${
            reviewed.length === 1 ? "this program" : "these programs"
          } from the repository's git config whenever it reads the repository. Only continue if you know where ${
            reviewed.length === 1 ? "it comes" : "they come"
          } from. You can stop trusting it later from the repository's menu.`}
          confirmLabel="Trust and continue"
          onConfirm={() => reviewValid && void trust(review)}
        >
          <ul
            className="filter-drivers filter-drivers-confirm"
            aria-label="Programs you are trusting"
          >
            {reviewed.map((driver) => (
              <DriverCard key={driver.fingerprint} driver={driver} />
            ))}
          </ul>
        </ConfirmContent>
      </ConfirmRoot>
    </div>
  );
}

/**
 * A single quiet line for a repository that has filter programs but is not
 * blocked: the user's own (git-lfs and friends) or ones they trusted.
 */
export function FilterNotice({ repo }: { repo: Repository }) {
  const security = useWorkspace((state) => state.security.get(repo.id));
  const revoke = useWorkspace((state) => state.revokeFilters);
  if (security?.status !== "ok") return null;
  const { status, drivers } = security.data;
  if (status !== "info" && status !== "trusted") return null;
  const names = drivers.map((driver) => driver.name).join(", ");

  return (
    <p className="diff-path-notice filter-notice" role="status">
      {status === "trusted" ? (
        <ShieldCheck size={14} aria-hidden />
      ) : (
        <Shield size={14} aria-hidden />
      )}
      <span>
        {status === "trusted"
          ? `You trusted this repository's git filter programs (${names}). WHY runs them while reading it.`
          : `This repository uses git filter programs from your own git config (${names}). WHY runs them the way git does.`}
      </span>
      {status === "trusted" && (
        <Button variant="ghost" size="xs" onClick={() => void revoke(repo)}>
          Stop trusting
        </Button>
      )}
    </p>
  );
}

/** The shield on a repository row; nothing for a repository without filter programs. */
export function FilterMarker({ repoId }: { repoId: RepoId }) {
  const security = useWorkspace((state) => state.security.get(repoId));
  if (security?.status !== "ok") return null;
  const { status, drivers } = security.data;
  if (status === "clear") return null;
  const names = drivers.map((driver) => driver.name).join(", ");
  const text = {
    blocked: `Blocked: defines git filter programs you have not trusted (${names})`,
    trusted: `Git filter programs trusted (${names})`,
    info: `Uses your own git filter programs (${names})`,
  }[status];
  const Icon = status === "blocked" ? ShieldAlert : status === "trusted" ? ShieldCheck : Shield;
  return (
    <Tip label={text} side="right">
      <span className="repo-shield" data-state={status} role="img" aria-label={text}>
        <Icon size={12} aria-hidden />
      </span>
    </Tip>
  );
}
