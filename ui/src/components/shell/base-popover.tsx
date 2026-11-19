import { useState, type FormEvent } from "react";
import { ArrowRight, GitBranch, Loader2 } from "lucide-react";
import type { IpcError, Worktree } from "@/contracts";
import { Button } from "@/components/ui/button";
import { PopoverContent, PopoverRoot, PopoverTrigger } from "@/components/ui/overlays";
import { describeBaseError } from "@/lib/error-text";
import { useWorkspace } from "@/stores/workspace-context";
import { BranchDivergence, branchLabel } from "./change-labels";

/** Refs worth offering whatever the repository: the usual trunks. */
const suggestedRefs = ["origin/main", "main"];

function BaseForm({ worktree, close }: { worktree: Worktree; close: () => void }) {
  const setBase = useWorkspace((state) => state.setBase);
  const current = worktree.base?.baseRef ?? "";
  const [value, setValue] = useState(current);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);
  const suggestions = [...new Set([current, ...suggestedRefs].filter(Boolean))];

  async function apply(ref: string) {
    if (busy) return;
    setBusy(true);
    setError(null);
    const result = await setBase(worktree, ref);
    setBusy(false);
    if (result.status === "ok") close();
    else setError(result.error);
  }

  function onSubmit(event: FormEvent) {
    event.preventDefault();
    void apply(value);
  }

  return (
    <form className="base-form" onSubmit={onSubmit}>
      <label htmlFor="base-ref">Compare against</label>
      <input
        id="base-ref"
        className="input mono"
        value={value}
        onChange={(event) => setValue(event.target.value)}
        placeholder="origin/main"
        spellCheck={false}
        autoComplete="off"
        aria-invalid={error ? true : undefined}
        aria-describedby={error ? "base-ref-error" : "base-ref-hint"}
      />
      {error ? (
        <p id="base-ref-error" className="field-error" role="alert">
          {describeBaseError(error)}
        </p>
      ) : (
        <p id="base-ref-hint" className="field-hint">
          Changes are measured from the merge-base with this branch or commit.
        </p>
      )}
      <div className="suggestions" role="group" aria-label="Suggested bases">
        {suggestions.map((ref) => (
          <button
            key={ref}
            type="button"
            className="chip mono"
            aria-pressed={ref === value.trim()}
            onClick={() => void apply(ref)}
          >
            {ref}
          </button>
        ))}
      </div>
      <div className="modal-actions">
        <Button type="button" variant="ghost" size="sm" onClick={() => void apply("")}>
          Use detected base
        </Button>
        <Button type="submit" size="sm" disabled={busy || !value.trim()}>
          {busy && <Loader2 className="spin" aria-hidden />}
          Set base
        </Button>
      </div>
    </form>
  );
}

/**
 * The `branch -> base` readout in the top bar, as a button that opens a small
 * form to change the base. With no base it reads "Set base".
 */
export function BaseSummary({ worktree }: { worktree: Worktree }) {
  const [open, setOpen] = useState(false);
  return (
    <PopoverRoot open={open} onOpenChange={setOpen}>
      {/* `title`, not a Tooltip: closing the popover returns focus to this button,
          and a focus-opened tooltip would then stay on screen. */}
      <PopoverTrigger asChild>
        <button type="button" className="branch-summary" title="Change the base branch">
          <GitBranch size={14} aria-hidden />
          <span className="truncate" title={branchLabel(worktree)}>
            {branchLabel(worktree)}
          </span>
          {worktree.base ? (
            <>
              <ArrowRight size={13} aria-hidden />
              <span className="truncate" title={worktree.base.baseRef}>
                {worktree.base.baseRef}
              </span>
              <BranchDivergence base={worktree.base} />
            </>
          ) : (
            <span className="branch-summary-empty">Set base</span>
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="base-popover">
        <BaseForm worktree={worktree} close={() => setOpen(false)} />
      </PopoverContent>
    </PopoverRoot>
  );
}
