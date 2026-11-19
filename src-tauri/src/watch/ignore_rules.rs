//! "Is this path gitignored?" for the watcher, so `node_modules/`, `target/`,
//! `dist/` and friends don't flood the UI with events.
//!
//! Built on the `ignore` crate's [`Gitignore`] matcher (the one ripgrep uses).
//! One matcher handles one `.gitignore` file, and its patterns are always
//! relative to the matcher's root directory, so a nested `src/.gitignore` needs
//! its own matcher rooted at `src/`. [`IgnoreRules`] keeps one matcher per
//! directory that has a `.gitignore`, plus `.git/info/exclude`, and asks them
//! deepest-first, like git does: the closest `.gitignore` decides, and a `!`
//! (whitelist) pattern there can re-include something a parent excluded.
//!
//! Not covered (deliberately): the user's global excludes file
//! (`core.excludesFile`). Reading it means trusting the user's git config,
//! which the app avoids; the cost is some extra events for editor swap files.

use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::{Match, WalkBuilder};

/// The ignore rules of one worktree.
#[derive(Debug, Default)]
pub struct IgnoreRules {
    /// `(directory relative to the worktree root, matcher)`, deepest directory
    /// first, then the root, then `info/exclude` (lowest precedence).
    layers: Vec<(PathBuf, Gitignore)>,
    /// Every ignore file read and its content, so a rebuild can tell whether
    /// anything actually changed (`Gitignore` itself is not comparable).
    sources: Vec<(PathBuf, Vec<u8>)>,
}

impl PartialEq for IgnoreRules {
    fn eq(&self, other: &Self) -> bool {
        self.sources == other.sources
    }
}

impl IgnoreRules {
    /// Read every `.gitignore` under `root` plus `<common_dir>/info/exclude`.
    ///
    /// Blocking file I/O (it walks the tree): call it from a blocking context
    /// (`spawn_blocking`, or notify's own thread), never directly on an async task.
    /// The walk itself honours the rules it has found so far, so it never
    /// descends into an ignored `node_modules/` to look for more `.gitignore`s.
    pub fn build(root: &Path, common_dir: &Path) -> Self {
        let mut layers = Vec::new();
        let mut sources = Vec::new();

        let walker = WalkBuilder::new(root)
            .hidden(false) // `.gitignore` and `.github/` are "hidden" files
            .ignore(false) // only git's rules, not ripgrep's `.ignore` files
            .git_global(false) // see the module docs
            .git_ignore(true)
            .git_exclude(true)
            .require_git(false)
            // Only files inside the worktree: git never reads a `.gitignore`
            // above the repository root (the default would walk up to `/`).
            .parents(false)
            .follow_links(false)
            // Never walk into a `.git` directory (ours or a nested repository's).
            .filter_entry(|e| e.file_name() != ".git")
            .build();
        for entry in walker.flatten() {
            let is_file = entry.file_type().is_some_and(|t| t.is_file());
            if !is_file || entry.file_name() != ".gitignore" {
                continue;
            }
            let Some(dir) = entry.path().parent() else {
                continue;
            };
            if let Some(gi) = matcher(dir, entry.path(), &mut sources)
                && let Ok(rel) = dir.strip_prefix(root)
            {
                layers.push((rel.to_path_buf(), gi));
            }
        }
        // Deepest directory first: more components = closer to the file.
        layers.sort_by_key(|(dir, _)| std::cmp::Reverse(dir.components().count()));

        if let Some(gi) = matcher(root, &common_dir.join("info").join("exclude"), &mut sources) {
            layers.push((PathBuf::new(), gi));
        }
        sources.sort();
        Self { layers, sources }
    }

    /// Is `rel` (relative to the worktree root) ignored? A path inside an
    /// ignored directory is ignored too (`node_modules/a/b.js`).
    ///
    /// `is_dir` matters for patterns ending in `/` (`target/` matches a
    /// directory named target, not a file).
    pub fn is_ignored(&self, rel: &Path, is_dir: bool) -> bool {
        for (dir, gi) in &self.layers {
            let Ok(sub) = rel.strip_prefix(dir) else {
                continue; // this .gitignore is not in an ancestor directory
            };
            if sub.as_os_str().is_empty() {
                continue; // the directory itself; its own .gitignore doesn't apply
            }
            // `sub` is relative, which this method requires (it asserts that
            // the path, once stripped of the matcher root, has no leading `/`).
            match gi.matched_path_or_any_parents(sub, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }
}

/// Build a matcher rooted at `dir` from one file; `None` if the file is missing
/// or has no usable patterns. Bad lines are skipped (git ignores them too).
fn matcher(dir: &Path, file: &Path, sources: &mut Vec<(PathBuf, Vec<u8>)>) -> Option<Gitignore> {
    let content = std::fs::read(file).ok()?;
    sources.push((file.to_owned(), content));
    let mut b = GitignoreBuilder::new(dir);
    if let Some(err) = b.add(file) {
        tracing::debug!(file = %file.display(), %err, "some ignore patterns were skipped");
    }
    let gi = b.build().ok()?;
    (!gi.is_empty()).then_some(gi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, s: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn root_nested_and_exclude_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let common = root.join(".git");
        write(root, ".gitignore", "node_modules/\n*.log\ntarget/\n");
        write(root, "web/.gitignore", "dist/\n!keep.log\n");
        write(root, ".git/info/exclude", "secret.txt\n");
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(root.join("web/dist")).unwrap();

        let rules = IgnoreRules::build(root, &common);
        let ign = |p: &str, d: bool| rules.is_ignored(Path::new(p), d);

        assert!(ign("node_modules", true));
        assert!(ign("node_modules/pkg/index.js", false));
        assert!(ign("debug.log", false));
        assert!(ign("target/debug/app", false));
        assert!(ign("web/dist/app.js", false), "nested .gitignore applies");
        assert!(!ign("dist/app.js", false), "nested rule is scoped to web/");
        assert!(!ign("web/keep.log", false), "nested ! re-includes");
        assert!(ign("secret.txt", false), "info/exclude applies");
        assert!(!ign("src/main.rs", false));
        assert!(!ign(".gitignore", false));
    }

    #[test]
    fn rules_compare_by_their_source_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, ".gitignore", "a\n");
        let common = root.join(".git");
        assert!(IgnoreRules::build(root, &common) == IgnoreRules::build(root, &common));
        let before = IgnoreRules::build(root, &common);
        write(root, ".gitignore", "b\n");
        assert!(before != IgnoreRules::build(root, &common));
    }

    #[test]
    fn no_rules_ignores_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let rules = IgnoreRules::build(tmp.path(), &tmp.path().join(".git"));
        assert!(!rules.is_ignored(Path::new("anything/at/all"), false));
    }
}
