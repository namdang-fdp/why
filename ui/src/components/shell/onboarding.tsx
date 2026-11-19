import { GitPullRequest, Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useWorkspace } from "@/stores/workspace-context";

/** First run: no repositories yet. One idea, one action. */
export function Onboarding() {
  const open = useWorkspace((state) => state.setAddRepositoryOpen);
  return (
    <main className="onboarding" aria-labelledby="onboarding-heading">
      <div className="onboarding-card">
        <span className="onboarding-mark" aria-hidden>
          <GitPullRequest size={18} />
        </span>
        <h2 id="onboarding-heading">Review what your agents wrote</h2>
        <p className="onboarding-lead">
          WHY lists every repository and git worktree your coding agents work in, and shows each
          change as a diff as it happens, so you can read it before it ships.
        </p>
        <div className="onboarding-actions">
          <Button onClick={() => open(true)}>
            <Plus aria-hidden />
            Add repository
          </Button>
          <span className="onboarding-hint">
            or press <kbd>a</kbd>
          </span>
        </div>
        <p className="onboarding-note">
          WHY only reads. It never edits, stages or commits in your repositories.
        </p>
      </div>
    </main>
  );
}
