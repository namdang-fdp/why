//! Changed files and file contents: "what differs, and what did each side look like?"
//!
//! Three things can be compared, and a worktree has all of them at once:
//!
//! ```text
//!   commit (HEAD / merge-base)  ->  index (staged)  ->  working tree (files on disk)
//! ```
//!
//! * **`Target::WorkingTree`**: old side -> files on disk (staged and unstaged
//!   together), plus untracked files. `git diff <rev>`.
//! * **`Target::Index`**: old side -> what `git commit` would record.
//!   `git diff --cached <rev>`.
//!
//! The old side is a commit: the merge-base with the base branch (vs-base mode)
//! or HEAD (uncommitted / staged mode). See [`super::base`] for why the
//! merge-base and not the base tip.
//!
//! ## One git call for the list
//!
//! `git diff --raw --numstat -z` prints both formats from the *same* diff run:
//! first one `--raw` record per file (status letter, modes, old blob id), then
//! one `--numstat` record per file (added / removed line counts). Running them
//! as two commands would let a file change between the calls. [`parse_diff`]
//! joins the two halves by path.
//!
//! ```text
//! :100644 100644 <old> <new> M\0mod.txt\0                       raw, ordinary
//! :100644 100644 <old> <new> R100\0old.txt\0new.txt\0           raw, rename
//! 2\t1\tmod.txt\0                                               numstat
//! 0\t0\t\0old.txt\0new.txt\0                                    numstat, rename
//! -\t-\tlogo.png\0                                              numstat, binary
//! ```
//!
//! ## Rules that keep WHY a passive observer
//!
//! * `-c diff.autoRefreshIndex=false`: plain `git diff <rev>` quietly *refreshes
//!   the index* (re-records file timestamps in `.git/index`) while holding
//!   `index.lock`, and `GIT_OPTIONAL_LOCKS=0` does **not** stop it (verified on
//!   git 2.56: the index file was rewritten). An agent running `git add` at that
//!   moment would fail. The price: a file that was only *touched* (timestamp
//!   changed, bytes identical) is reported as modified; see [`drop_stat_only`].
//! * `--no-ext-diff --no-textconv`: repo config could otherwise name programs
//!   that git runs to produce the diff.
//! * `--ignore-submodules=dirty`: do not start a nested `git status` inside
//!   every submodule just to list files.
//! * `--end-of-options` before the revision, and `--` after it (the revision is
//!   never a path).

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use tokio::io::AsyncReadExt;

use super::error::{GitError, ParseError};
use super::oid::{Oid, path_bytes, path_from_bytes};
use super::runner::GitRunner;
use super::status::{FileState, StatusKind, StatusSnapshot, read_status};

/// The id of the empty tree in a SHA-1 repository.
///
/// A repository without commits has no HEAD to diff against. Git has a built-in,
/// well-known object, "the tree with no entries", and diffing against it makes
/// every file show up as *added*, which is exactly the truth for an unborn
/// branch. Git knows this object even though it is not stored in the repository.
/// A SHA-256 repository has a different one (`6ef19b41…`), so the code does not
/// use this constant: it asks git ([`empty_tree_id`]). The constant documents
/// the well-known answer and is what the tests compare against.
pub const EMPTY_TREE_SHA1: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Largest file side we load into memory: 1 MiB. A bigger file is reported as
/// `too_large` instead; the webview editor would choke on it anyway.
pub const MAX_CONTENT_BYTES: u64 = 1024 * 1024;

/// A file is treated as binary when a NUL byte appears in its first 8 KiB: the
/// same heuristic git itself uses.
const SNIFF_BYTES: usize = 8 * 1024;

/// Budget for counting lines of untracked files ([`count_untracked_lines`]):
/// an un-ignored build directory must not turn a cheap list into a long scan.
const COUNT_MAX_FILES: usize = 500;
const COUNT_MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;

/// Mode git records for a submodule (a "gitlink"): a commit, not file content.
const MODE_GITLINK: u32 = 0o160000;
const MODE_SYMLINK: u32 = 0o120000;

/// How a file changed. Mirrors the IPC contract's `ChangeStatus` on purpose
/// (the git layer does not import the contracts; `ipc` maps one to the other).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Relative to the worktree root. Raw bytes (a `PathBuf`), as git printed them.
    pub path: PathBuf,
    /// Previous path of a rename or copy.
    pub old_path: Option<PathBuf>,
    pub kind: ChangeKind,
    /// Part of the change is recorded in the index.
    pub staged: bool,
    /// Part of the change exists only in the working tree.
    pub unstaged: bool,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub binary: bool,
    /// Octal file modes before and after (`0` = the file does not exist on that
    /// side). Used to recognise submodules (`160000`).
    pub old_mode: u32,
    pub new_mode: u32,
    /// Object id of the old / new blob, when git printed one. Contents are read
    /// by *id* (see [`read_blob_side`]), never by path, so the size check and
    /// the read see the same object even if an agent stages something else in
    /// between. `new_oid` is only known for [`Target::Index`] (the working tree
    /// has no blob yet); `None` for files that do not exist on that side.
    pub old_oid: Option<String>,
    pub new_oid: Option<String>,
}

impl Change {
    /// A submodule entry: its "content" is a commit id, not text.
    pub fn is_gitlink(&self) -> bool {
        self.old_mode == MODE_GITLINK || self.new_mode == MODE_GITLINK
    }
}

/// What the old side of a diff is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OldSide {
    /// Whatever HEAD is at the time of the call; the empty tree if the branch
    /// has no commits yet.
    Head,
    /// A specific commit (the merge-base).
    Commit(Oid),
}

/// What the new side of a diff is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Files on disk. `include_untracked` adds files git does not know yet.
    WorkingTree { include_untracked: bool },
    /// The index.
    Index,
}

impl Target {
    pub fn is_index(self) -> bool {
        matches!(self, Target::Index)
    }
}

/// The changed files plus the commit they were measured against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    /// Revision of the old side (a commit id, or the empty tree's id). Used to
    /// look up a conflicted file's old content, which has no blob id.
    pub old_rev: String,
    /// Sorted by path (byte order, like git).
    pub changes: Vec<Change>,
}

// ---------------------------------------------------------------------------
// Listing changes
// ---------------------------------------------------------------------------

/// List the changed files of `worktree` ("enumerate"; no line counts for
/// untracked files, see [`count_untracked_lines`] for those).
///
/// Runs `git status` (flags, untracked files, conflicts) and `git diff`
/// (statuses, line counts) and merges the two answers.
pub async fn read_changes(
    runner: &GitRunner,
    worktree: &Path,
    old: &OldSide,
    target: Target,
) -> Result<ChangeSet, GitError> {
    let (old_rev, status, entries) = match old {
        OldSide::Commit(oid) => {
            // The two commands are independent: run them side by side.
            let rev = oid.to_string();
            let (status, diff) = tokio::join!(
                read_status(runner, worktree),
                run_diff(runner, worktree, &rev, target)
            );
            (rev, status?, diff?)
        }
        OldSide::Head => {
            // HEAD comes out of the status header (`# branch.oid`), so status
            // goes first. Pinning the diff to that commit id, instead of the
            // word `HEAD`, keeps both answers about the same commit even if an
            // agent commits in between.
            let status = read_status(runner, worktree).await?;
            let rev = match &status.branch.oid {
                Some(oid) => oid.to_string(),
                None => empty_tree_id(runner, worktree).await?,
            };
            let diff = run_diff(runner, worktree, &rev, target).await?;
            (rev, status, diff)
        }
    };
    let mut entries = entries;
    if !target.is_index() {
        drop_stat_only(&mut entries);
        confirm_binary_changes(runner, worktree, &mut entries).await?;
    }
    let changes = merge_changes(entries, &status, target);
    Ok(ChangeSet { old_rev, changes })
}

/// The id of the empty tree in this repository's object format.
///
/// `git hash-object -t tree --stdin` hashes whatever is on stdin as a tree,
/// without `-w` so nothing is written. The runner gives every child an empty
/// stdin (`Stdio::null()`), and an empty tree is exactly "no entries", so the
/// answer is the empty-tree id of *this* repository: the SHA-1 constant
/// [`EMPTY_TREE_SHA1`] or, in a SHA-256 repository, `6ef19b41…`. Unlike
/// `rev-parse --show-object-format` (git 2.38+) this works on every git we
/// support. Only asked for when HEAD is unborn, so the common path pays nothing.
pub async fn empty_tree_id(runner: &GitRunner, worktree: &Path) -> Result<String, GitError> {
    let out = runner
        .run(worktree, ["hash-object", "-t", "tree", "--stdin"])
        .await?;
    Ok(Oid::from_bytes(out.trim_ascii())?.to_string())
}

async fn run_diff(
    runner: &GitRunner,
    worktree: &Path,
    old_rev: &str,
    target: Target,
) -> Result<Vec<DiffEntry>, GitError> {
    let mut args: Vec<&str> = vec![
        // See the module docs: without this, `git diff` rewrites `.git/index`.
        "-c",
        "diff.autoRefreshIndex=false",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        // Full object ids (the default is 7 characters), so a blob id can be
        // compared with the output of `git hash-object`.
        "--no-abbrev",
        "--ignore-submodules=dirty",
        "-z",
        // Detect renames (a deleted and an added file with similar content).
        "-M",
        "--raw",
        "--numstat",
    ];
    if target.is_index() {
        args.push("--cached");
    }
    args.extend(["--end-of-options", old_rev, "--"]);
    let out = runner.run(worktree, args).await?;
    Ok(parse_diff(&out)?)
}

/// One file of `git diff --raw --numstat`, before it is merged with `status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    pub kind: ChangeKind,
    pub old_mode: u32,
    pub new_mode: u32,
    /// Full id of the old blob (zeros if the file did not exist).
    pub old_oid: String,
    /// Full id of the new blob: zeros for the working tree (not hashed yet),
    /// the staged blob for `--cached`.
    pub new_oid: String,
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub binary: bool,
    /// git printed a `--numstat` record for this file. It leaves out files whose
    /// content did not change; see [`drop_stat_only`].
    pub has_numstat: bool,
}

/// Parse `git diff --raw --numstat -z` output. Pure function.
///
/// Unmerged (`U`) raw records are skipped: conflicts are taken from
/// `git status`, which describes them better (who deleted, who added).
pub fn parse_diff(out: &[u8]) -> Result<Vec<DiffEntry>, ParseError> {
    // `-z`: every field ends in NUL, so the output is a flat list of tokens.
    // A token that starts with ':' begins a raw record; any other starts a
    // numstat record. A path is always consumed *positionally* (as the token
    // after its metadata), so a path that itself starts with ':' or a digit
    // cannot be mistaken for a record start.
    let mut tokens = out.split(|b| *b == 0);
    let mut raws: Vec<DiffEntry> = Vec::new();
    // numstat records by (new) path bytes -> (additions, deletions)
    let mut stats: HashMap<Vec<u8>, (Option<u32>, Option<u32>)> = HashMap::new();

    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue; // the empty piece after the final NUL
        }
        if let Some(meta) = token.strip_prefix(b":") {
            // `<old mode> <new mode> <old id> <new id> <status>`
            let fields: Vec<&[u8]> = meta.split(|b| *b == b' ').collect();
            let [old_mode, new_mode, old_oid, new_oid, status] = fields.as_slice() else {
                return Err(ParseError::new(format!(
                    "malformed raw diff record: {:?}",
                    String::from_utf8_lossy(token)
                )));
            };
            let first = next_token(&mut tokens, "raw record path")?;
            // Renames and copies carry two paths: old, then new.
            let (old_path, path) = if matches!(status.first(), Some(b'R' | b'C')) {
                let second = next_token(&mut tokens, "rename target path")?;
                (Some(path_from_bytes(first)), path_from_bytes(second))
            } else {
                (None, path_from_bytes(first))
            };
            let kind = match status.first() {
                Some(b'A') => ChangeKind::Added,
                Some(b'M') => ChangeKind::Modified,
                Some(b'D') => ChangeKind::Deleted,
                Some(b'R') => ChangeKind::Renamed,
                Some(b'C') => ChangeKind::Copied,
                Some(b'T') => ChangeKind::TypeChanged,
                Some(b'U') => continue, // conflicts come from `git status`
                _ => {
                    return Err(ParseError::new(format!(
                        "unknown diff status {:?}",
                        String::from_utf8_lossy(status)
                    )));
                }
            };
            raws.push(DiffEntry {
                kind,
                old_mode: parse_mode(old_mode)?,
                new_mode: parse_mode(new_mode)?,
                old_oid: String::from_utf8_lossy(old_oid).into_owned(),
                new_oid: String::from_utf8_lossy(new_oid).into_owned(),
                path,
                old_path,
                additions: None,
                deletions: None,
                binary: false,
                has_numstat: false,
            });
        } else {
            // `<added>\t<removed>\t<path>`; for a rename the path field is
            // empty and the next two tokens are the old and new path.
            let mut parts = token.splitn(3, |b| *b == b'\t');
            let (Some(added), Some(removed), Some(path_field)) =
                (parts.next(), parts.next(), parts.next())
            else {
                return Err(ParseError::new(format!(
                    "malformed numstat record: {:?}",
                    String::from_utf8_lossy(token)
                )));
            };
            let key = if path_field.is_empty() {
                let _old = next_token(&mut tokens, "numstat rename old path")?;
                next_token(&mut tokens, "numstat rename new path")?.to_vec()
            } else {
                path_field.to_vec()
            };
            stats.insert(key, (parse_count(added)?, parse_count(removed)?));
        }
    }

    for entry in &mut raws {
        if let Some((additions, deletions)) = stats.remove(path_bytes(&entry.path).as_ref()) {
            entry.has_numstat = true;
            entry.additions = additions;
            entry.deletions = deletions;
            // numstat prints `-` for both counts when the file is binary.
            entry.binary = additions.is_none() && deletions.is_none();
        }
    }
    Ok(raws)
}

fn next_token<'a>(
    tokens: &mut impl Iterator<Item = &'a [u8]>,
    what: &str,
) -> Result<&'a [u8], ParseError> {
    // `split` yields an empty piece after the final NUL, so "the output ended"
    // looks like an empty token. A path is never empty: treat both as truncation.
    tokens
        .next()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ParseError::new(format!("diff output ended before the {what}")))
}

fn parse_mode(field: &[u8]) -> Result<u32, ParseError> {
    std::str::from_utf8(field)
        .ok()
        .and_then(|s| u32::from_str_radix(s, 8).ok())
        .ok_or_else(|| {
            ParseError::new(format!(
                "bad file mode {:?}",
                String::from_utf8_lossy(field)
            ))
        })
}

/// `-` (binary) is `None`; anything else must be a number.
fn parse_count(field: &[u8]) -> Result<Option<u32>, ParseError> {
    if field == b"-" {
        return Ok(None);
    }
    std::str::from_utf8(field)
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Some)
        .ok_or_else(|| {
            ParseError::new(format!(
                "bad line count {:?}",
                String::from_utf8_lossy(field)
            ))
        })
}

/// Remove "modified" files whose content did not actually change.
///
/// Because the diff runs with `diff.autoRefreshIndex=false`, git does not
/// re-check files whose timestamps differ from the index. Such a file (touched
/// by a build tool, or just checked out) is listed as `M`, but `--numstat`
/// leaves out files with identical content. So: `M`, same mode on both sides,
/// no numstat record = not really changed. A mode change (`chmod +x`) differs
/// in mode and does get a numstat record (`0 0`), so it stays.
///
/// Binary files are not covered by this rule (numstat prints `- -` for them
/// either way); [`confirm_binary_changes`] handles those.
pub fn drop_stat_only(entries: &mut Vec<DiffEntry>) {
    entries.retain(|e| {
        !(e.kind == ChangeKind::Modified && e.old_mode == e.new_mode && !e.has_numstat)
    });
}

/// For each "modified binary file", compare the bytes on disk with the old blob
/// and drop the entry when they are identical (a touched, unchanged file).
///
/// `git hash-object --no-filters` computes the blob id of the file as it is on
/// disk, without converting line endings or running a clean filter (which could
/// execute a configured program). It only reads; without `-w` it writes nothing.
async fn confirm_binary_changes(
    runner: &GitRunner,
    worktree: &Path,
    entries: &mut Vec<DiffEntry>,
) -> Result<(), GitError> {
    let mut identical = HashSet::new();
    for entry in entries.iter() {
        let candidate = entry.kind == ChangeKind::Modified
            && entry.binary
            && entry.old_mode == entry.new_mode
            && entry.new_mode != MODE_GITLINK
            && entry.new_mode != MODE_SYMLINK;
        if !candidate {
            continue;
        }
        let args: [OsString; 4] = [
            "hash-object".into(),
            "--no-filters".into(),
            "--".into(),
            entry.path.clone().into_os_string(),
        ];
        // If git cannot hash the path (a FIFO or device where a file used to
        // be: "unsupported file type") we cannot prove it unchanged, so the
        // entry stays in the list. Only a successful, equal hash drops it.
        match runner.run(worktree, args).await {
            Ok(out) if out.trim_ascii() == entry.old_oid.as_bytes() => {
                identical.insert(entry.path.clone());
            }
            Ok(_) | Err(GitError::NonZeroExit { .. }) => {}
            Err(other) => return Err(other),
        }
    }
    entries.retain(|e| !identical.contains(&e.path));
    Ok(())
}

/// A blob id from git's raw output as something usable: `None` for the all-zero
/// id (git's "no object here") and for anything that is not a full object id.
fn blob_id(raw: &str) -> Option<String> {
    let oid = Oid::parse(raw).ok()?;
    (!oid.is_null()).then(|| oid.to_string())
}

/// Combine the diff entries with `git status`. Pure function.
///
/// * `staged` / `unstaged` come from status (`XY`: `X` index vs HEAD, `Y` working
///   tree vs index). A file that differs from the base only through commits on
///   the branch has neither flag set.
/// * Unmerged paths from status become [`ChangeKind::Conflicted`] and replace
///   whatever the diff said about them.
/// * Untracked paths (`?`) are added for [`Target::WorkingTree`] with
///   `include_untracked`.
///
/// # One row per path
///
/// `git rm --cached f` (file stays on disk) makes git report `f` twice in the
/// working-tree modes: as *deleted* by the diff (it is gone from the index) and
/// as *untracked* by status (it is on disk but unknown to git). The rule:
///
/// * **`WorkingTree` modes** show the working-tree view: the file exists on
///   disk, so the single row is [`ChangeKind::Modified`] with `staged: true`
///   (the removal from the index is staged) and `unstaged: true`. It keeps the
///   old blob id, so the file opens with its baseline and its current content.
///   Line counts are `None`: the diff counted a deletion, which would be wrong.
/// * **`Index`** shows the index view: *deleted*, as git says. Untracked files
///   are not part of that mode, so there is no second row.
pub fn merge_changes(
    entries: Vec<DiffEntry>,
    status: &StatusSnapshot,
    target: Target,
) -> Vec<Change> {
    let mut flags: HashMap<&Path, (bool, bool)> = HashMap::new();
    let mut conflicts: Vec<&Path> = Vec::new();
    let mut untracked: Vec<&Path> = Vec::new();
    for entry in &status.entries {
        match &entry.kind {
            StatusKind::Changed { staged, unstaged }
            | StatusKind::Renamed {
                staged, unstaged, ..
            } => {
                flags.insert(
                    entry.path.as_path(),
                    (
                        *staged != FileState::Unmodified,
                        *unstaged != FileState::Unmodified,
                    ),
                );
            }
            StatusKind::Unmerged { .. } => conflicts.push(entry.path.as_path()),
            StatusKind::Untracked => untracked.push(entry.path.as_path()),
            StatusKind::Ignored => {}
        }
    }
    let conflicted: HashSet<&Path> = conflicts.iter().copied().collect();
    let include_untracked = matches!(
        target,
        Target::WorkingTree {
            include_untracked: true
        }
    );
    let untracked_set: HashSet<&Path> = untracked.iter().copied().collect();
    // Untracked paths already represented by a (reconciled) diff row.
    let mut reconciled: HashSet<&Path> = HashSet::new();

    let mut changes: Vec<Change> = Vec::new();
    for e in entries {
        if conflicted.contains(e.path.as_path()) {
            continue;
        }
        let (old_oid, new_oid) = (blob_id(&e.old_oid), blob_id(&e.new_oid));

        if include_untracked
            && e.kind == ChangeKind::Deleted
            && let Some(on_disk) = untracked_set.get(e.path.as_path())
        {
            reconciled.insert(on_disk);
            changes.push(Change {
                path: e.path,
                old_path: None,
                kind: ChangeKind::Modified,
                staged: true,
                unstaged: true,
                additions: None,
                deletions: None,
                binary: false,
                old_mode: e.old_mode,
                // The file is on disk; its real mode is unknown, and only the
                // gitlink check looks at it. Mirror the old mode.
                new_mode: e.old_mode,
                old_oid,
                new_oid: None,
            });
            continue;
        }

        let (status_staged, status_unstaged) = flags
            .get(e.path.as_path())
            .copied()
            .unwrap_or((false, false));
        changes.push(Change {
            staged: target.is_index() || status_staged,
            unstaged: status_unstaged,
            path: e.path,
            old_path: e.old_path,
            kind: e.kind,
            additions: e.additions,
            deletions: e.deletions,
            binary: e.binary,
            old_mode: e.old_mode,
            new_mode: e.new_mode,
            old_oid,
            new_oid,
        });
    }
    for path in conflicts {
        changes.push(Change {
            path: path.to_path_buf(),
            old_path: None,
            kind: ChangeKind::Conflicted,
            staged: false,
            unstaged: true,
            additions: None,
            deletions: None,
            binary: false,
            old_mode: 0,
            new_mode: 0,
            old_oid: None,
            new_oid: None,
        });
    }
    if include_untracked {
        for path in untracked {
            if reconciled.contains(path) {
                continue;
            }
            changes.push(Change {
                path: path.to_path_buf(),
                old_path: None,
                kind: ChangeKind::Untracked,
                staged: false,
                unstaged: true,
                additions: None,
                deletions: None,
                binary: false,
                old_mode: 0,
                new_mode: 0,
                old_oid: None,
                new_oid: None,
            });
        }
    }

    // Deterministic order: byte order of the path, which is also git's order.
    changes.sort_by(|a, b| path_bytes(&a.path).cmp(&path_bytes(&b.path)));
    changes
}

// ---------------------------------------------------------------------------
// Line counts for untracked files
// ---------------------------------------------------------------------------

/// Fill in `additions` (and `binary`) for untracked files: git has no diff for
/// them, so their "added lines" are simply their line count.
///
/// Bounded on purpose: at most [`COUNT_MAX_FILES`] files and
/// [`COUNT_MAX_TOTAL_BYTES`] bytes in total, [`MAX_CONTENT_BYTES`] per file.
/// What is past the budget keeps `additions: None` ("not computed").
/// Files are opened with [`open_in_worktree`], the same guarded open as
/// `get_file_diff` uses: no symlink following, no FIFOs, parent directory kept
/// inside the worktree.
pub async fn count_untracked_lines(worktree: &Path, changes: &mut [Change]) {
    let Ok(root) = tokio::fs::canonicalize(worktree).await else {
        return; // the worktree is gone: nothing to count
    };
    let mut bytes = 0u64;
    for (visited, change) in changes
        .iter_mut()
        .filter(|c| c.kind == ChangeKind::Untracked)
        .enumerate()
    {
        if visited >= COUNT_MAX_FILES || bytes >= COUNT_MAX_TOTAL_BYTES {
            break;
        }
        let Ok(counted) = count_file(&root, &change.path).await else {
            continue; // vanished, unreadable or refused: leave it uncounted
        };
        match counted {
            Counted::Lines { lines, bytes: n } => {
                bytes += n;
                change.additions = Some(lines);
            }
            Counted::Binary { bytes: n } => {
                bytes += n;
                change.binary = true;
            }
            Counted::Skipped => {}
        }
    }
}

enum Counted {
    Lines {
        lines: u32,
        bytes: u64,
    },
    Binary {
        bytes: u64,
    },
    /// Not a regular file, or too large.
    Skipped,
}

async fn count_file(root: &Path, rel: &Path) -> io::Result<Counted> {
    let (file, len) = match open_in_worktree(root, rel).await? {
        // Like git, a symlink's content is its target: one line.
        Opened::Symlink(_) => return Ok(Counted::Lines { lines: 1, bytes: 0 }),
        Opened::Absent | Opened::NotAFile => return Ok(Counted::Skipped),
        Opened::File { file, len } => (file, len),
    };
    if len > MAX_CONTENT_BYTES {
        return Ok(Counted::Skipped);
    }
    let Some(buf) = read_capped(file).await? else {
        return Ok(Counted::Skipped);
    };
    let n = buf.len() as u64;
    if looks_binary(&buf) {
        return Ok(Counted::Binary { bytes: n });
    }
    Ok(Counted::Lines {
        lines: count_lines(&buf),
        bytes: n,
    })
}

/// Read at most [`MAX_CONTENT_BYTES`] from `file`; `None` if there is more.
/// `take` bounds the read itself: the file may grow after the size check.
async fn read_capped(file: tokio::fs::File) -> io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    file.take(MAX_CONTENT_BYTES + 1)
        .read_to_end(&mut buf)
        .await?;
    Ok((buf.len() as u64 <= MAX_CONTENT_BYTES).then_some(buf))
}

/// Number of lines like `git diff --numstat` counts them: every `\n`, plus one
/// for a final line without a trailing newline.
pub fn count_lines(content: &[u8]) -> u32 {
    let newlines = content.iter().filter(|b| **b == b'\n').count();
    let unterminated = content.last().is_some_and(|b| *b != b'\n');
    u32::try_from(newlines + usize::from(unterminated)).unwrap_or(u32::MAX)
}

/// NUL byte in the first [`SNIFF_BYTES`] bytes.
pub fn looks_binary(content: &[u8]) -> bool {
    content[..content.len().min(SNIFF_BYTES)].contains(&0)
}

// ---------------------------------------------------------------------------
// File contents
// ---------------------------------------------------------------------------

/// One side of a file diff, as we could load it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    /// The file does not exist on this side.
    Absent,
    /// Valid UTF-8 text within the size limit.
    Text(String),
    /// Contains NUL bytes **or is not valid UTF-8**. The editor in the webview
    /// needs a real string; decoding Latin-1 text with replacement characters
    /// would show the reviewer content that is not what is on disk. Showing
    /// "binary" is the honest choice.
    Binary,
    /// Over [`MAX_CONTENT_BYTES`]; not loaded.
    TooLarge,
    /// Not a file git has content for: a directory (nested repository) or a
    /// submodule. Shown like binary.
    NotAFile,
}

/// Sort loaded bytes into [`Side::Text`] or [`Side::Binary`].
pub fn classify(content: Vec<u8>) -> Side {
    if looks_binary(&content) {
        return Side::Binary;
    }
    match String::from_utf8(content) {
        Ok(text) => Side::Text(text),
        Err(_) => Side::Binary,
    }
}

/// Read a blob from git's object database.
///
/// `spec` is a git "object name". Callers pass the **blob id** that `git diff`
/// printed ([`oid_spec`]): both `cat-file` calls below then address the very
/// same immutable object, whatever an agent stages in between, and no path is
/// ever parsed as revision syntax (so a file called `0:foo` or `:x` is just a
/// file). Only a conflicted file has no blob id; it falls back to
/// [`commit_spec`] (`<commit>:<path>`).
///
/// With `GIT_NO_LAZY_FETCH=1` (set by the runner) a blob that is missing from a
/// partial clone is an error here (exit 128), not a download.
///
/// Size is checked first (`cat-file -s`): the runner collects all output in
/// memory, so we must not ask for a huge blob only to throw it away.
///
/// `absent_ok`: whether "no such path" (git exits 128) means "this side does not
/// exist" ([`Side::Absent`]). Only a conflicted file legitimately lacks a side
/// (no merge-base version, no stage-0 index entry). For every other change the
/// side must exist, so a 128 is a real failure (a race with the agent, a corrupt
/// repository) and is returned as an error instead of being shown as "added".
pub async fn read_blob_side(
    runner: &GitRunner,
    worktree: &Path,
    spec: OsString,
    absent_ok: bool,
) -> Result<Side, GitError> {
    let size_out = match runner
        .run(
            worktree,
            [
                OsString::from("cat-file"),
                "-s".into(),
                "--end-of-options".into(),
                spec.clone(),
            ],
        )
        .await
    {
        Ok(out) => out,
        // Exit 128: "path does not exist in <rev>".
        Err(e) if absent_ok && e.is_exit_code(128) => return Ok(Side::Absent),
        Err(e) => return Err(e),
    };
    let size: u64 = std::str::from_utf8(size_out.trim_ascii())
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| ParseError::new("cat-file -s did not print a size"))?;
    if size > MAX_CONTENT_BYTES {
        return Ok(Side::TooLarge);
    }
    let blob = runner
        .run(
            worktree,
            [
                OsString::from("cat-file"),
                "blob".into(),
                "--end-of-options".into(),
                spec,
            ],
        )
        .await?;
    // Belt and braces: the id is immutable, so this cannot happen; but never
    // hand a bigger buffer on than the limit promises.
    if blob.len() as u64 > MAX_CONTENT_BYTES {
        return Ok(Side::TooLarge);
    }
    Ok(classify(blob))
}

/// `<rev>:<path>` as an argument (bytes, because the path may not be UTF-8).
pub fn commit_spec(rev: &str, path: &Path) -> OsString {
    let mut bytes = rev.as_bytes().to_vec();
    bytes.push(b':');
    bytes.extend_from_slice(&path_bytes(path));
    path_from_bytes(&bytes).into_os_string()
}

/// A blob id as a `cat-file` argument; `None` if it is not a full object id.
pub fn oid_spec(oid: &str) -> Option<OsString> {
    Oid::parse(oid).ok().map(|o| OsString::from(o.to_string()))
}

/// Read a file of the working tree, safely (see [`open_in_worktree`]).
pub async fn read_worktree_side(canonical_root: &Path, rel: &Path) -> io::Result<Side> {
    match open_in_worktree(canonical_root, rel).await? {
        Opened::Absent => Ok(Side::Absent),
        Opened::NotAFile => Ok(Side::NotAFile),
        // A symlink is never followed: its target text is the content, like git
        // stores it.
        Opened::Symlink(target) => Ok(classify(path_bytes(&target).into_owned())),
        Opened::File { file, len } => {
            if len > MAX_CONTENT_BYTES {
                return Ok(Side::TooLarge);
            }
            Ok(match read_capped(file).await? {
                Some(buf) => classify(buf),
                None => Side::TooLarge,
            })
        }
    }
}

/// What [`open_in_worktree`] found at a path.
#[derive(Debug)]
pub enum Opened {
    /// Nothing there (or a parent directory is missing).
    Absent,
    /// A symlink; the link target, not followed.
    Symlink(PathBuf),
    /// A directory, FIFO, socket or device: nothing to read.
    NotAFile,
    /// A regular file, opened, with its size from `fstat` on the open handle.
    File { file: tokio::fs::File, len: u64 },
}

/// Open the working-tree file at `rel` (a path from git's own change list)
/// inside `canonical_root`, without being fooled by symlinks or special files.
///
/// An agent (or anything else) can change the worktree while we read it, so
/// every check is repeated on the thing we actually opened:
///
/// 1. `rel` must consist of plain names only (no `..`, no root).
/// 2. The *parent directory* is canonicalized (symlinks resolved) and must be
///    inside the root: `d/f` is refused when `d` has become a symlink elsewhere.
/// 3. The last component is inspected with `lstat`: a symlink is reported, never
///    opened; a directory or FIFO is reported as [`Opened::NotAFile`].
/// 4. The file is opened with `O_NOFOLLOW` (fails if the last component was
///    swapped for a symlink after step 3) and `O_NONBLOCK` (opening a FIFO that
///    appeared meanwhile returns at once instead of waiting for a writer).
/// 5. `fstat` on the **open handle**: it must be a regular file, and (Unix) the
///    same device and inode that step 3 saw, so it is the file we vetted.
/// 6. The canonical parent is computed again and must be unchanged: a parent
///    directory swapped for a symlink while we were opening is caught here,
///    before any byte is read.
///
/// Errors: a refusal (not plain, leaves the worktree, changed underneath us) is
/// `PermissionDenied` or `InvalidInput` *without* an OS error code, which the
/// IPC layer maps to `InvalidInput`.
pub async fn open_in_worktree(canonical_root: &Path, rel: &Path) -> io::Result<Opened> {
    if !is_plain_relative(rel) {
        return Err(refusal(
            io::ErrorKind::InvalidInput,
            "path is not a plain relative path",
        ));
    }
    let Some(file_name) = rel.file_name() else {
        return Err(refusal(
            io::ErrorKind::InvalidInput,
            "path has no file name",
        ));
    };
    let parent = match rel.parent() {
        Some(p) if !p.as_os_str().is_empty() => canonical_root.join(p),
        _ => canonical_root.to_path_buf(),
    };
    let parent = match tokio::fs::canonicalize(&parent).await {
        Ok(p) => p,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Opened::Absent),
        Err(e) => return Err(e),
    };
    if !parent.starts_with(canonical_root) {
        return Err(refusal(
            io::ErrorKind::PermissionDenied,
            "path leaves the worktree",
        ));
    }
    let leaf = parent.join(file_name);
    let vetted = match tokio::fs::symlink_metadata(&leaf).await {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Opened::Absent),
        Err(e) => return Err(e),
    };
    if vetted.file_type().is_symlink() {
        return Ok(Opened::Symlink(tokio::fs::read_link(&leaf).await?));
    }
    if !vetted.is_file() {
        return Ok(Opened::NotAFile);
    }

    let file = match open_nofollow(&leaf).await {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Opened::Absent),
        Err(e) => return Err(e),
    };
    let opened = file.metadata().await?;
    if !opened.is_file() {
        return Ok(Opened::NotAFile);
    }
    if !same_file(&vetted, &opened) {
        return Err(refusal(
            io::ErrorKind::PermissionDenied,
            "file changed while it was being opened",
        ));
    }
    match tokio::fs::canonicalize(&parent).await {
        Ok(again) if again == parent => {}
        _ => {
            return Err(refusal(
                io::ErrorKind::PermissionDenied,
                "directory changed while the file was being opened",
            ));
        }
    }
    Ok(Opened::File {
        file,
        len: opened.len(),
    })
}

/// An error that is our own decision (no OS error code), not an I/O failure.
fn refusal(kind: io::ErrorKind, message: &'static str) -> io::Error {
    io::Error::new(kind, message)
}

/// Open read-only without following a symlink in the last component and without
/// blocking on a FIFO (Unix: `O_NOFOLLOW | O_NONBLOCK`). Public so tests can
/// exercise the flags directly; use [`open_in_worktree`] in real code.
#[cfg(unix)]
pub async fn open_nofollow(path: &Path) -> io::Result<tokio::fs::File> {
    tokio::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .await
        .map_err(|e| {
            if e.raw_os_error() == Some(libc::ELOOP) {
                // The kernel refuses to follow a symlink: it was swapped in.
                refusal(io::ErrorKind::PermissionDenied, "path is a symlink")
            } else {
                e
            }
        })
}

/// Without `O_NOFOLLOW` there is no race-free way to open; the `lstat` and
/// `fstat` checks in [`open_in_worktree`] still apply, with a small window.
#[cfg(not(unix))]
pub async fn open_nofollow(path: &Path) -> io::Result<tokio::fs::File> {
    tokio::fs::File::open(path).await
}

/// Do two `stat` results describe the same file (same device and inode)?
#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    true
}

/// Only ordinary names: no `..`, `.`, root (`/`) or drive prefix.
fn is_plain_relative(path: &Path) -> bool {
    path.components().next().is_some()
        && path.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Why a path from the webview was refused (see [`check_request_path`]).
///
/// Only what could escape the worktree is refused: empty, NUL, absolute, or a
/// `..` component. "Absolute" and "component" are decided by [`Path`] for the
/// *host* platform, so on Unix a name like `0:foo`, `:x` or `a\b` is an ordinary
/// file name (and works: contents are read by object id, never by pasting the
/// path into `rev:path` syntax), while on Windows `C:\x` and `\\server\share`
/// are refused as the absolute paths they are. The request is then still only a
/// lookup key into the change list, and file reads are re-checked on open.
pub fn check_request_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Err("path is empty");
    }
    if path.contains('\0') {
        return Err("path contains a NUL byte");
    }
    if Path::new(path)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("path must not contain `..`");
    }
    if !is_plain_relative(Path::new(path)) {
        return Err("path must be relative to the worktree");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::status::parse_status;

    fn parse(s: &str) -> Vec<DiffEntry> {
        parse_diff(s.as_bytes()).unwrap()
    }

    const ZERO: &str = "0000000000000000000000000000000000000000";

    #[test]
    fn parses_modify_add_delete_with_counts() {
        let out = format!(
            ":100644 100644 aaa {ZERO} M\0mod.txt\0\
             :000000 100644 {ZERO} {ZERO} A\0add.txt\0\
             :100644 000000 bbb {ZERO} D\0del.txt\0\
             2\t1\tmod.txt\0\
             1\t0\tadd.txt\0\
             0\t3\tdel.txt\0"
        );
        let e = parse(&out);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].kind, ChangeKind::Modified);
        assert_eq!((e[0].additions, e[0].deletions), (Some(2), Some(1)));
        assert_eq!(e[1].kind, ChangeKind::Added);
        assert_eq!(e[1].old_mode, 0);
        assert_eq!(e[2].kind, ChangeKind::Deleted);
        assert_eq!(e[2].new_mode, 0);
        assert!(e.iter().all(|x| x.has_numstat && !x.binary));
    }

    #[test]
    fn parses_a_rename_with_both_formats() {
        let out = ":100644 100644 aaa bbb R100\0old name.txt\0new name.txt\0\
                   0\t0\t\0old name.txt\0new name.txt\0";
        let e = parse(out);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].kind, ChangeKind::Renamed);
        assert_eq!(e[0].old_path.as_deref(), Some(Path::new("old name.txt")));
        assert_eq!(e[0].path, Path::new("new name.txt"));
        assert!(e[0].has_numstat);
        assert_eq!(e[0].additions, Some(0));
    }

    #[test]
    fn binary_files_have_no_counts() {
        let e = parse(&format!(
            ":000000 100644 {ZERO} {ZERO} A\0logo.png\0-\t-\tlogo.png\0"
        ));
        assert!(e[0].binary);
        assert_eq!((e[0].additions, e[0].deletions), (None, None));
    }

    #[test]
    fn unusual_paths_do_not_confuse_the_tokenizer() {
        // A path that starts with ':' or a digit, and one containing a tab.
        let out = ":100644 100644 a b M\0:colon\0\
                   :100644 100644 a b M\x001\t2\tx\0\
                   :100644 100644 a b M\0tab\there\0\
                   1\t1\t:colon\0\
                   1\t1\t1\t2\tx\0\
                   1\t1\ttab\there\0";
        let e = parse(out);
        let paths: Vec<_> = e.iter().map(|x| x.path.to_str().unwrap()).collect();
        assert_eq!(paths, [":colon", "1\t2\tx", "tab\there"]);
        assert!(e.iter().all(|x| x.has_numstat));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_survive_as_bytes() {
        let mut out = b":100644 100644 a b M\0caf\xe9.txt\0".to_vec();
        out.extend_from_slice(b"1\t0\tcaf\xe9.txt\0");
        let e = parse_diff(&out).unwrap();
        assert_eq!(path_bytes(&e[0].path).as_ref(), b"caf\xe9.txt");
        assert!(e[0].has_numstat);
    }

    #[test]
    fn unmerged_records_are_skipped_but_consume_their_path() {
        let out = format!(
            ":100644 000000 aaa {ZERO} U\0c.txt\0\
             :100644 100644 aaa {ZERO} M\0ok.txt\0\
             0\t0\tc.txt\0\
             1\t1\tok.txt\0"
        );
        let e = parse(&out);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].path, Path::new("ok.txt"));
    }

    #[test]
    fn malformed_output_is_an_error_not_a_panic() {
        assert!(parse_diff(b":100644 100644 a b\0x\0").is_err()); // too few fields
        assert!(parse_diff(b":100644 100644 a b M\0").is_err()); // path missing
        assert!(parse_diff(b":100644 100644 a b R100\0only-one\0").is_err());
        assert!(parse_diff(b":100644 100644 a b Q\0x\0").is_err()); // unknown status
        assert!(parse_diff(b"nonsense\0").is_err()); // numstat without tabs
        assert!(parse_diff(b"x\ty\tfile\0").is_err()); // counts are not numbers
        assert!(parse_diff(b"").unwrap().is_empty());
    }

    #[test]
    fn touched_but_identical_files_are_dropped() {
        let out = format!(
            ":100644 100644 a {ZERO} M\0touched.txt\0\
             :100644 100644 a {ZERO} M\0real.txt\0\
             :100755 100644 a {ZERO} M\0chmod.txt\0\
             1\t1\treal.txt\0\
             0\t0\tchmod.txt\0"
        );
        // `touched.txt`: same mode, no numstat record. `chmod.txt`: mode differs.
        let mut e = parse(&out);
        drop_stat_only(&mut e);
        let paths: Vec<_> = e.iter().map(|x| x.path.to_str().unwrap()).collect();
        assert_eq!(paths, ["real.txt", "chmod.txt"]);
    }

    fn status(raw: &str) -> StatusSnapshot {
        parse_status(raw.as_bytes()).unwrap()
    }

    const OID: &str = "1111111111111111111111111111111111111111";

    #[test]
    fn merge_sets_flags_conflicts_and_untracked() {
        let status = status(&format!(
            "# branch.oid {OID}\0# branch.head main\0\
             1 M. N... 100644 100644 100644 {OID} {OID} staged.txt\0\
             1 .M N... 100644 100644 100644 {OID} {OID} unstaged.txt\0\
             1 MM N... 100644 100644 100644 {OID} {OID} both.txt\0\
             u UU N... 100644 100644 100644 100644 {OID} {OID} {OID} conflict.txt\0\
             ? new/untracked.txt\0"
        ));
        let entry = |p: &str| DiffEntry {
            kind: ChangeKind::Modified,
            old_mode: 0o100644,
            new_mode: 0o100644,
            old_oid: OID.into(),
            new_oid: ZERO.into(),
            path: p.into(),
            old_path: None,
            additions: Some(1),
            deletions: Some(1),
            binary: false,
            has_numstat: true,
        };
        let entries = vec![
            entry("staged.txt"),
            entry("unstaged.txt"),
            entry("both.txt"),
            entry("conflict.txt"),  // diff also reports it (as plain M)
            entry("committed.txt"), // changed on the branch, clean in the worktree
        ];
        let wt = Target::WorkingTree {
            include_untracked: true,
        };
        let changes = merge_changes(entries.clone(), &status, wt);
        let summary: Vec<_> = changes
            .iter()
            .map(|c| (c.path.to_str().unwrap(), c.kind, c.staged, c.unstaged))
            .collect();
        assert_eq!(
            summary,
            [
                ("both.txt", ChangeKind::Modified, true, true),
                ("committed.txt", ChangeKind::Modified, false, false),
                ("conflict.txt", ChangeKind::Conflicted, false, true),
                ("new/untracked.txt", ChangeKind::Untracked, false, true),
                ("staged.txt", ChangeKind::Modified, true, false),
                ("unstaged.txt", ChangeKind::Modified, false, true),
            ]
        );

        // Blob ids: all-zero means "none" and becomes `None`.
        let first = &changes[0];
        assert_eq!(first.old_oid.as_deref(), Some(OID));
        assert_eq!(first.new_oid, None);

        // Index mode: every entry is staged by definition, and no untracked files.
        let staged = merge_changes(entries, &status, Target::Index);
        assert!(
            staged
                .iter()
                .all(|c| c.kind == ChangeKind::Conflicted || c.staged)
        );
        assert!(staged.iter().all(|c| c.kind != ChangeKind::Untracked));
    }

    #[test]
    fn line_counting_matches_numstat() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a"), 1);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a\nb"), 2);
        assert_eq!(count_lines(b"\n\n"), 2);
    }

    #[test]
    fn classify_text_binary_and_invalid_utf8() {
        assert_eq!(classify(b"hello".to_vec()), Side::Text("hello".into()));
        assert_eq!(classify(b"a\0b".to_vec()), Side::Binary);
        assert_eq!(classify(b"caf\xe9".to_vec()), Side::Binary); // Latin-1, not UTF-8
        assert_eq!(classify(Vec::new()), Side::Text(String::new()));
    }

    #[test]
    fn a_path_removed_from_the_index_but_still_on_disk_is_one_row() {
        // `git rm --cached keep.txt`: the diff says deleted, status says untracked.
        let status = status(&format!(
            "# branch.oid {OID}\0# branch.head main\0\
             ? keep.txt\0\
             ? other.txt\0"
        ));
        let deleted = |p: &str| DiffEntry {
            kind: ChangeKind::Deleted,
            old_mode: 0o100644,
            new_mode: 0,
            old_oid: OID.into(),
            new_oid: ZERO.into(),
            path: p.into(),
            old_path: None,
            additions: Some(0),
            deletions: Some(3),
            binary: false,
            has_numstat: true,
        };
        let rows = |target| {
            merge_changes(
                vec![deleted("keep.txt"), deleted("really-gone.txt")],
                &status,
                target,
            )
            .into_iter()
            .map(|c| {
                (
                    c.path.to_str().unwrap().to_string(),
                    c.kind,
                    c.staged,
                    c.unstaged,
                    c.additions,
                )
            })
            .collect::<Vec<_>>()
        };

        // Working-tree view: the file is on disk, so ONE row, modified, with the
        // old blob kept (baseline) and no misleading "-3 lines".
        let wt = Target::WorkingTree {
            include_untracked: true,
        };
        assert_eq!(
            rows(wt),
            [
                (
                    "keep.txt".to_string(),
                    ChangeKind::Modified,
                    true,
                    true,
                    None
                ),
                (
                    "other.txt".to_string(),
                    ChangeKind::Untracked,
                    false,
                    true,
                    None
                ),
                (
                    "really-gone.txt".to_string(),
                    ChangeKind::Deleted,
                    false,
                    false,
                    Some(0)
                ),
            ]
        );
        let kept = merge_changes(vec![deleted("keep.txt")], &status, wt);
        assert_eq!(kept[0].old_oid.as_deref(), Some(OID));

        // Index view: deleted, as git says; untracked files are not part of it.
        assert_eq!(
            rows(Target::Index),
            [
                (
                    "keep.txt".to_string(),
                    ChangeKind::Deleted,
                    true,
                    false,
                    Some(0)
                ),
                (
                    "really-gone.txt".to_string(),
                    ChangeKind::Deleted,
                    true,
                    false,
                    Some(0)
                ),
            ]
        );
    }

    #[test]
    fn request_paths_are_validated() {
        assert!(check_request_path("src/lib.rs").is_ok());
        assert!(check_request_path("a b/ü.txt").is_ok());
        assert!(check_request_path("..foo/bar").is_ok()); // `..foo` is a normal name
        for bad in [
            "",
            "/etc/passwd",
            "../../etc/passwd",
            "a/../../b",
            "a/..",
            "..",
            "a\0b",
        ] {
            assert!(check_request_path(bad).is_err(), "{bad:?} must be refused");
        }
    }

    /// On Unix, `:` and `\` are ordinary file-name characters.
    #[cfg(unix)]
    #[test]
    fn legal_unix_names_are_accepted() {
        for ok in ["0:foo", ":x", "a:b", "C:/x", "a\\b", "x/0:y", "d/:z"] {
            // `C:/x` is the relative path `C:` / `x` here.
            assert!(
                check_request_path(ok).is_ok(),
                "{ok:?} is a legal Unix name"
            );
        }
    }

    /// On Windows these are absolute paths or separators.
    #[cfg(windows)]
    #[test]
    fn windows_absolute_paths_are_refused() {
        for bad in ["\\windows", "C:/x", "C:\\x", "a\\..\\b"] {
            assert!(check_request_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn specs_are_object_ids_or_commit_paths() {
        assert_eq!(
            commit_spec("abc", Path::new("d/f.txt")),
            OsString::from("abc:d/f.txt")
        );
        assert_eq!(oid_spec(OID), Some(OsString::from(OID)));
        assert_eq!(oid_spec(&OID.to_uppercase()), Some(OsString::from(OID)));
        assert_eq!(oid_spec("abc"), None); // abbreviated: refuse, never guess
        assert_eq!(oid_spec("--help"), None);
        assert_eq!(oid_spec(":0:foo"), None);
        assert_eq!(blob_id(ZERO), None);
        assert_eq!(blob_id("abcdef0"), None);
        assert_eq!(blob_id(OID).as_deref(), Some(OID));
    }
}
