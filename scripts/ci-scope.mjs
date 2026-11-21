// Decides whether CI has to run the app's jobs (Rust, lint, tests) for a pull request.
//
// The repo ruleset requires the check "Lint and test (Linux)", so the workflow itself must always
// run (a workflow-level `paths-ignore` would leave that check missing and block the merge). This
// script lets the job's expensive steps skip themselves instead. It fails toward running
// everything: any doubt means `app=true`.
import { execFileSync } from "node:child_process";
import { appendFileSync, realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";

/** False only when every changed path is under `site/` or `docs/`. */
export function appChanged(paths) {
  if (paths.length === 0) return true; // nothing listed: the diff was unreadable, be safe
  return !paths.every((path) => path.startsWith("site/") || path.startsWith("docs/"));
}

function decide() {
  // Pushes to main always run everything.
  if (process.env.GITHUB_EVENT_NAME !== "pull_request") return true;
  try {
    // Only a merge commit (two parents) has the layout this relies on. Anything else, such as a
    // checkout of the PR head, means HEAD^1 is not the base branch: run everything.
    const parents = execFileSync("git", ["rev-list", "--parents", "-n", "1", "HEAD"], {
      encoding: "utf8",
    })
      .trim()
      .split(" ");
    if (parents.length !== 3) return true;
    // On a pull_request run HEAD is the merge commit, so its first parent is the base branch tip
    // and this lists exactly what the PR changes. --no-renames lists both sides of a rename, so
    // moving a file out of `ui/` into `docs/` is still seen as touching `ui/`.
    const out = execFileSync("git", ["diff", "--name-only", "--no-renames", "HEAD^1", "HEAD"], {
      encoding: "utf8",
    });
    return appChanged(out.split("\n").filter(Boolean));
  } catch {
    return true;
  }
}

// realpath: a symlinked checkout path must still count as "run directly", or the script would
// print nothing and the workflow would skip every step.
if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  const line = `app=${decide()}\n`;
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, line);
  process.stdout.write(line);
}
