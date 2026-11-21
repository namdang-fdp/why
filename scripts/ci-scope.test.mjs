// Run with: node --test scripts/ci-scope.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { appChanged } from "./ci-scope.mjs";

test("a PR that only touches site/ and docs/ does not need the app jobs", () => {
  assert.equal(appChanged(["site/src/pages/index.astro", "docs/superpowers/specs/x.md"]), false);
});

test("a PR that touches the app as well as site/ needs the app jobs", () => {
  assert.equal(appChanged(["site/a.astro", "ui/src/App.tsx"]), true);
});

test("changing the workflow or root tooling always needs the app jobs", () => {
  assert.equal(appChanged([".github/workflows/ci.yml"]), true);
  assert.equal(appChanged(["eslint.config.js"]), true);
});

test("an empty list means the diff could not be read: run everything", () => {
  assert.equal(appChanged([]), true);
});

test("the prefix is 'site/', not 'site': sitemap.txt is not part of site/", () => {
  assert.equal(appChanged(["sitemap.txt"]), true);
  assert.equal(appChanged(["docs-old/x.md"]), true);
});

// --- CLI behaviour: these run the real script in throwaway directories ---------------------------
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const script = join(dirname(fileURLToPath(import.meta.url)), "ci-scope.mjs");
// A clean environment: nothing inherited from the developer's git config or a running hook.
const cleanEnv = (extra = {}) => ({ PATH: process.env.PATH, ...extra });

function git(cwd, ...args) {
  return execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    env: cleanEnv({
      HOME: cwd,
      GIT_CONFIG_GLOBAL: "/dev/null",
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_AUTHOR_NAME: "t",
      GIT_AUTHOR_EMAIL: "t@example.com",
      GIT_COMMITTER_NAME: "t",
      GIT_COMMITTER_EMAIL: "t@example.com",
    }),
  });
}
function write(repo, path, text) {
  mkdirSync(dirname(join(repo, path)), { recursive: true });
  writeFileSync(join(repo, path), text);
}
const runCli = (cwd) =>
  execFileSync("node", [script], {
    cwd,
    encoding: "utf8",
    env: cleanEnv({ GITHUB_EVENT_NAME: "pull_request" }),
  }).trim();

test("the CLI still decides when started through a symlinked directory", () => {
  // The workflow's `if` must never see an empty output: a missing answer would skip all of CI.
  const root = mkdtempSync(join(tmpdir(), "ci-scope-link-"));
  symlinkSync(dirname(script), join(root, "scripts"));
  const out = execFileSync("node", [join(root, "scripts", "ci-scope.mjs")], {
    encoding: "utf8",
    env: cleanEnv({ GITHUB_EVENT_NAME: "push" }),
  });
  assert.equal(out.trim(), "app=true");
});

test("a PR merge commit that only changes site/ needs no app jobs", () => {
  const repo = mkdtempSync(join(tmpdir(), "ci-scope-merge-"));
  git(repo, "init", "-q", "-b", "main");
  write(repo, "ui/a.txt", "1");
  git(repo, "add", "-A");
  git(repo, "commit", "-q", "-m", "base");
  git(repo, "checkout", "-q", "-b", "pr");
  write(repo, "site/x.txt", "1");
  git(repo, "add", "-A");
  git(repo, "commit", "-q", "-m", "site only");
  git(repo, "checkout", "-q", "main");
  write(repo, "docs/y.txt", "1");
  git(repo, "add", "-A");
  git(repo, "commit", "-q", "-m", "main moved");
  git(repo, "merge", "-q", "--no-ff", "-m", "merge", "pr");
  assert.equal(runCli(repo), "app=false");
});

test("HEAD that is not a merge commit means the layout is unknown: run everything", () => {
  // If the workflow ever checks out the PR head instead of the merge commit, HEAD^1 is the PR's
  // own previous commit and a last commit touching only site/ would hide earlier app changes.
  const repo = mkdtempSync(join(tmpdir(), "ci-scope-linear-"));
  git(repo, "init", "-q", "-b", "main");
  write(repo, "ui/a.txt", "1");
  git(repo, "add", "-A");
  git(repo, "commit", "-q", "-m", "app change");
  write(repo, "site/x.txt", "1");
  git(repo, "add", "-A");
  git(repo, "commit", "-q", "-m", "site only");
  assert.equal(runCli(repo), "app=true");
});
