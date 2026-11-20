//! `git status`: what has changed in a worktree right now.
//!
//! We parse **porcelain v2** with `-z`:
//!
//! * *porcelain* means "stable, meant for programs". The human format changes
//!   with config and locale; porcelain does not.
//! * *v2* adds the per-file detail we need (modes, object ids, rename scores)
//!   and a `--branch` header block.
//! * *`-z`* terminates every record with NUL (`\0`) instead of newline and turns
//!   off path quoting. Without it a file called `a\nb` or `"quoted"` arrives
//!   as C-style escaped text (`"a\nb"`) that we would have to unescape. With
//!   it, paths are raw bytes and the only delimiter that cannot appear inside a
//!   path is NUL.
//!
//! Record formats (fields separated by single spaces, path is always last):
//!
//! ```text
//! # branch.oid <commit> | (initial)        header lines
//! # branch.head <name> | (detached)
//! # branch.upstream <upstream>
//! # branch.ab +<ahead> -<behind>
//! 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>                 ordinary change
//! 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <Xscore> <path>\0<orig> rename or copy
//! u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>       unmerged (conflict)
//! ? <path>                                                     untracked
//! ! <path>                                                     ignored
//! ```
//!
//! Note the quirk of `2` records: with `-z` the *original* path is not part of
//! the same field run, it is the **next NUL-separated token**.
//!
//! Because the path is the last field and may itself contain spaces, we split
//! each record into a fixed number of fields (`splitn` style) and keep the rest
//! as the path.

use std::borrow::Cow;
use std::path::PathBuf;

use super::error::{GitError, ParseError};
use super::filters::ReadPermit;
use super::oid::{Oid, path_from_bytes};
use super::runner::GitRunner;

/// Everything `git status` reported for one worktree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusSnapshot {
    pub branch: BranchStatus,
    pub entries: Vec<StatusEntry>,
}

/// The `# branch.*` header block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BranchStatus {
    /// Commit HEAD points at. `None` on an unborn branch (`(initial)`).
    pub oid: Option<Oid>,
    /// `None` only if git sent no `branch.head` header (not expected with `--branch`).
    pub head: Option<BranchHead>,
    /// Upstream (tracking) branch such as `origin/main`, if configured.
    pub upstream: Option<String>,
    /// Ahead/behind relative to `upstream`. Present only when the upstream
    /// exists (a configured but deleted upstream has no counts).
    pub ahead_behind: Option<AheadBehind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchHead {
    /// On a branch (short name, e.g. `feature/x`).
    Branch(String),
    /// HEAD is a bare commit id.
    Detached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadBehind {
    pub ahead: u32,
    pub behind: u32,
}

/// One changed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Path relative to the worktree root, kept as the raw bytes git printed
    /// (a `PathBuf`), because file names on Unix need not be valid UTF-8.
    pub path: PathBuf,
    pub kind: StatusKind,
}

impl StatusEntry {
    /// Lossy UTF-8 text for showing in a UI (invalid bytes become U+FFFD).
    /// Do not use it to access the file; use [`StatusEntry::path`].
    pub fn display_path(&self) -> Cow<'_, str> {
        self.path.to_string_lossy()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusKind {
    /// Record type `1`. `staged` = index vs HEAD (the `X` of `XY`), `unstaged`
    /// = working tree vs index (the `Y`). `Unmodified` means "no change on that side".
    Changed {
        staged: FileState,
        unstaged: FileState,
    },
    /// Record type `2`: a rename or copy, with the original path. `path` is the
    /// new location.
    Renamed {
        staged: FileState,
        unstaged: FileState,
        orig_path: PathBuf,
        op: RenameOp,
        /// Similarity percentage, 0..=100.
        score: u8,
    },
    /// Record type `u`: a merge conflict.
    Unmerged { conflict: ConflictKind },
    /// Record type `?`: not tracked and not ignored.
    Untracked,
    /// Record type `!`: ignored. Only appears with `--ignored`; we do not request it.
    Ignored,
}

/// One side of `XY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// `.` in porcelain v2 (a space in the short format).
    Unmodified,
    Modified,
    TypeChanged,
    Added,
    Deleted,
    Renamed,
    Copied,
}

impl FileState {
    fn from_byte(b: u8) -> Result<Self, ParseError> {
        Ok(match b {
            b'.' => FileState::Unmodified,
            b'M' => FileState::Modified,
            b'T' => FileState::TypeChanged,
            b'A' => FileState::Added,
            b'D' => FileState::Deleted,
            b'R' => FileState::Renamed,
            b'C' => FileState::Copied,
            other => {
                return Err(ParseError::new(format!(
                    "unknown status code {:?}",
                    other as char
                )));
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameOp {
    Rename,
    Copy,
}

/// The seven unmerged `XY` pairs. "Us" is the branch you are on, "them" the
/// one being merged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// `DD`
    BothDeleted,
    /// `AU`
    AddedByUs,
    /// `UD`
    DeletedByThem,
    /// `UA`
    AddedByThem,
    /// `DU`
    DeletedByUs,
    /// `AA`
    BothAdded,
    /// `UU`
    BothModified,
}

/// Run `git status` in the permitted worktree and parse it.
///
/// `status` reads (and may hash) tracked files, which is exactly what can run a
/// repository's filter drivers, so it needs a [`ReadPermit`] from the filter
/// guard (`git::filters`).
///
/// Flags: `--branch` for the header block, `--untracked-files=all` so a new
/// directory is reported file by file (not as a single `dir/` entry the
/// reviewer cannot open). The runner sets `GIT_OPTIONAL_LOCKS=0`, so this
/// never writes to the index.
///
/// `--ignore-submodules=dirty`: without it `status` starts a nested `git status`
/// inside every submodule to look for edits, and that nested git reads the
/// *submodule's own* config (`.git/modules/<name>/config`), which the guard has
/// not scanned. Measured: a clean filter configured only in a submodule ran on
/// an outer `git status`. With `dirty` the submodule is compared by commit id
/// only, which is also what `git diff` does in `diff.rs`.
pub async fn read_status(
    runner: &GitRunner,
    permit: &ReadPermit,
) -> Result<StatusSnapshot, GitError> {
    let out = runner
        .run(
            permit.worktree(),
            [
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--untracked-files=all",
                "--ignore-submodules=dirty",
            ],
        )
        .await?;
    Ok(parse_status(&out)?)
}

/// Parse the output of `git status --porcelain=v2 -z --branch`. Pure function
/// over bytes; never panics on malformed input (returns [`ParseError`]).
pub fn parse_status(out: &[u8]) -> Result<StatusSnapshot, ParseError> {
    let mut snapshot = StatusSnapshot::default();
    // Every record ends with NUL, so drop the final one first. Otherwise the
    // empty slice after it would look like a record, and (worse) would be taken
    // as the "original path" of a truncated rename record at the very end.
    let mut records = out.strip_suffix(&[0]).unwrap_or(out).split(|&b| b == 0);

    while let Some(rec) = records.next() {
        if rec.is_empty() {
            continue; // empty input
        }
        // Every record starts with a one-byte tag followed by a space.
        if rec.get(1) != Some(&b' ') {
            return Err(bad_record(rec));
        }
        match rec[0] {
            b'#' => parse_header(&rec[2..], &mut snapshot.branch)?,
            b'1' => {
                let [_, xy, _sub, _mh, _mi, _mw, _hh, _hi, path] =
                    split_fields::<9>(rec).ok_or_else(|| bad_record(rec))?;
                let (staged, unstaged) = parse_xy(xy)?;
                snapshot.entries.push(StatusEntry {
                    path: path_from_bytes(path),
                    kind: StatusKind::Changed { staged, unstaged },
                });
            }
            b'2' => {
                let [_, xy, _sub, _mh, _mi, _mw, _hh, _hi, score, path] =
                    split_fields::<10>(rec).ok_or_else(|| bad_record(rec))?;
                let (staged, unstaged) = parse_xy(xy)?;
                let (op, score) = parse_score(score)?;
                // The original path is the NEXT token, by position: not
                // recognised by looking at its first byte (a file can be called
                // `1 x` or `? y`).
                let orig = records
                    .next()
                    .filter(|orig| !orig.is_empty())
                    .ok_or_else(|| ParseError::new("rename record without original path"))?;
                snapshot.entries.push(StatusEntry {
                    path: path_from_bytes(path),
                    kind: StatusKind::Renamed {
                        staged,
                        unstaged,
                        orig_path: path_from_bytes(orig),
                        op,
                        score,
                    },
                });
            }
            b'u' => {
                let [_, xy, _sub, _m1, _m2, _m3, _mw, _h1, _h2, _h3, path] =
                    split_fields::<11>(rec).ok_or_else(|| bad_record(rec))?;
                snapshot.entries.push(StatusEntry {
                    path: path_from_bytes(path),
                    kind: StatusKind::Unmerged {
                        conflict: parse_conflict(xy)?,
                    },
                });
            }
            b'?' => snapshot.entries.push(StatusEntry {
                path: path_from_bytes(&rec[2..]),
                kind: StatusKind::Untracked,
            }),
            b'!' => snapshot.entries.push(StatusEntry {
                path: path_from_bytes(&rec[2..]),
                kind: StatusKind::Ignored,
            }),
            _ => return Err(bad_record(rec)),
        }
    }
    Ok(snapshot)
}

/// Apply one `# key value` header line (without the leading `# `). Unknown
/// headers (e.g. `stash`, or ones added by newer git) are ignored, as the git
/// documentation tells parsers to do.
fn parse_header(line: &[u8], branch: &mut BranchStatus) -> Result<(), ParseError> {
    let (key, value) = match line.iter().position(|&b| b == b' ') {
        Some(i) => (&line[..i], &line[i + 1..]),
        None => (line, &[][..]),
    };
    match key {
        b"branch.oid" => {
            branch.oid = if value == b"(initial)" {
                None // unborn branch: no commit yet
            } else {
                Some(Oid::from_bytes(value)?)
            };
        }
        b"branch.head" => {
            branch.head = Some(if value == b"(detached)" {
                BranchHead::Detached
            } else {
                BranchHead::Branch(String::from_utf8_lossy(value).into_owned())
            });
        }
        b"branch.upstream" => {
            branch.upstream = Some(String::from_utf8_lossy(value).into_owned());
        }
        b"branch.ab" => {
            // `+<ahead> -<behind>`
            let text = std::str::from_utf8(value)
                .map_err(|_| ParseError::new("branch.ab is not UTF-8"))?;
            let ahead = text
                .split(' ')
                .next()
                .and_then(|s| s.strip_prefix('+'))
                .and_then(|s| s.parse().ok());
            let behind = text
                .split(' ')
                .nth(1)
                .and_then(|s| s.strip_prefix('-'))
                .and_then(|s| s.parse().ok());
            let (Some(ahead), Some(behind)) = (ahead, behind) else {
                return Err(ParseError::new(format!("bad branch.ab value {text:?}")));
            };
            branch.ahead_behind = Some(AheadBehind { ahead, behind });
        }
        _ => {}
    }
    Ok(())
}

/// Split `line` at single spaces into exactly `N` fields; the last field is the
/// remainder (so it may contain spaces). `None` if there are too few fields.
fn split_fields<const N: usize>(line: &[u8]) -> Option<[&[u8]; N]> {
    let mut fields: [&[u8]; N] = [&[]; N];
    let mut rest = line;
    for slot in fields.iter_mut().take(N - 1) {
        let i = rest.iter().position(|&b| b == b' ')?;
        *slot = &rest[..i];
        rest = &rest[i + 1..];
    }
    fields[N - 1] = rest;
    Some(fields)
}

fn parse_xy(xy: &[u8]) -> Result<(FileState, FileState), ParseError> {
    match xy {
        [x, y] => Ok((FileState::from_byte(*x)?, FileState::from_byte(*y)?)),
        _ => Err(ParseError::new(format!(
            "XY field is not 2 bytes: {:?}",
            String::from_utf8_lossy(xy)
        ))),
    }
}

/// `R100`, `C75` -> (Rename, 100), (Copy, 75).
fn parse_score(field: &[u8]) -> Result<(RenameOp, u8), ParseError> {
    let bad = || {
        ParseError::new(format!(
            "bad rename score {:?}",
            String::from_utf8_lossy(field)
        ))
    };
    let (&op, digits) = field.split_first().ok_or_else(bad)?;
    let op = match op {
        b'R' => RenameOp::Rename,
        b'C' => RenameOp::Copy,
        _ => return Err(bad()),
    };
    let score: u8 = std::str::from_utf8(digits)
        .ok()
        .and_then(|d| d.parse().ok())
        .filter(|&s| s <= 100)
        .ok_or_else(bad)?;
    Ok((op, score))
}

fn parse_conflict(xy: &[u8]) -> Result<ConflictKind, ParseError> {
    Ok(match xy {
        b"DD" => ConflictKind::BothDeleted,
        b"AU" => ConflictKind::AddedByUs,
        b"UD" => ConflictKind::DeletedByThem,
        b"UA" => ConflictKind::AddedByThem,
        b"DU" => ConflictKind::DeletedByUs,
        b"AA" => ConflictKind::BothAdded,
        b"UU" => ConflictKind::BothModified,
        other => {
            return Err(ParseError::new(format!(
                "unknown unmerged XY {:?}",
                String::from_utf8_lossy(other)
            )));
        }
    })
}

fn bad_record(rec: &[u8]) -> ParseError {
    // Show a bounded, lossy prefix: enough to debug, not a wall of text.
    let shown: String = String::from_utf8_lossy(rec).chars().take(80).collect();
    ParseError::new(format!("malformed status record: {shown:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const H: &str = "49b4b03d7dd07d73f8b334a20f142f4f8c913376";
    const Z: &str = "0000000000000000000000000000000000000000";

    /// Join records with NUL and add the trailing NUL git prints.
    fn z(records: &[&[u8]]) -> Vec<u8> {
        let mut v = Vec::new();
        for r in records {
            v.extend_from_slice(r);
            v.push(0);
        }
        v
    }

    fn one(xy: &str, path: &str) -> Vec<u8> {
        format!("1 {xy} N... 100644 100644 100644 {H} {H} {path}").into_bytes()
    }

    fn paths(s: &StatusSnapshot) -> Vec<String> {
        s.entries
            .iter()
            .map(|e| e.display_path().into_owned())
            .collect()
    }

    #[test]
    fn headers_on_a_branch_with_upstream() {
        let out = z(&[
            format!("# branch.oid {H}").as_bytes(),
            b"# branch.head feature/x",
            b"# branch.upstream origin/feature/x",
            b"# branch.ab +3 -2",
        ]);
        let s = parse_status(&out).unwrap();
        assert_eq!(s.branch.oid.unwrap().as_str(), H);
        assert_eq!(s.branch.head, Some(BranchHead::Branch("feature/x".into())));
        assert_eq!(s.branch.upstream.as_deref(), Some("origin/feature/x"));
        assert_eq!(
            s.branch.ahead_behind,
            Some(AheadBehind {
                ahead: 3,
                behind: 2
            })
        );
        assert!(s.entries.is_empty());
    }

    #[test]
    fn headers_initial_and_detached_and_unknown() {
        let s = parse_status(&z(&[
            b"# branch.oid (initial)",
            b"# branch.head main",
            b"# stash 2",
            b"# some.future header",
        ]))
        .unwrap();
        assert_eq!(s.branch.oid, None);
        assert_eq!(s.branch.head, Some(BranchHead::Branch("main".into())));
        assert_eq!(s.branch.upstream, None);
        assert_eq!(s.branch.ahead_behind, None);

        let s = parse_status(&z(&[
            format!("# branch.oid {H}").as_bytes(),
            b"# branch.head (detached)",
        ]))
        .unwrap();
        assert_eq!(s.branch.head, Some(BranchHead::Detached));
    }

    #[test]
    fn ordinary_entries_staged_and_unstaged() {
        let out = z(&[
            &one("M.", "staged.rs"),
            &one(".M", "unstaged.rs"),
            &one("MM", "both.rs"),
            &one("A.", "new.rs"),
            &one(".D", "gone.rs"),
            &one("T.", "type.rs"),
        ]);
        let s = parse_status(&out).unwrap();
        assert_eq!(s.entries.len(), 6);
        let kind = |i: usize| s.entries[i].kind.clone();
        assert_eq!(
            kind(0),
            StatusKind::Changed {
                staged: FileState::Modified,
                unstaged: FileState::Unmodified
            }
        );
        assert_eq!(
            kind(1),
            StatusKind::Changed {
                staged: FileState::Unmodified,
                unstaged: FileState::Modified
            }
        );
        assert_eq!(
            kind(2),
            StatusKind::Changed {
                staged: FileState::Modified,
                unstaged: FileState::Modified
            }
        );
        assert_eq!(
            kind(3),
            StatusKind::Changed {
                staged: FileState::Added,
                unstaged: FileState::Unmodified
            }
        );
        assert_eq!(
            kind(4),
            StatusKind::Changed {
                staged: FileState::Unmodified,
                unstaged: FileState::Deleted
            }
        );
        assert_eq!(
            kind(5),
            StatusKind::Changed {
                staged: FileState::TypeChanged,
                unstaged: FileState::Unmodified
            }
        );
    }

    #[test]
    fn paths_with_spaces_unicode_and_newline() {
        let out = z(&[
            &one("M.", "my file.rs"),
            &one(".M", "tệp mới.rs"),
            &one("A.", "line1\nline2.txt"),
            b"? dir with space/new file.txt",
            &one("M.", "  leading and trailing  "),
        ]);
        let s = parse_status(&out).unwrap();
        assert_eq!(
            paths(&s),
            [
                "my file.rs",
                "tệp mới.rs",
                "line1\nline2.txt",
                "dir with space/new file.txt",
                "  leading and trailing  ",
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_does_not_panic_and_round_trips() {
        use std::os::unix::ffi::OsStrExt;
        let mut rec = format!("1 .M N... 100644 100644 100644 {H} {H} caf").into_bytes();
        rec.extend_from_slice(b"\xe9\xff.txt"); // Latin-1 e-acute + an impossible UTF-8 byte
        let s = parse_status(&z(&[&rec, b"? \xfe"])).unwrap();
        assert_eq!(s.entries[0].path.as_os_str().as_bytes(), b"caf\xe9\xff.txt");
        assert_eq!(s.entries[0].display_path(), "caf\u{fffd}\u{fffd}.txt");
        assert_eq!(s.entries[1].path.as_os_str().as_bytes(), b"\xfe");
    }

    #[test]
    fn rename_with_spaces_in_both_paths() {
        // The original path is the next NUL-separated token, not part of the record.
        let rec = format!("2 R. N... 100644 100644 100644 {H} {H} R100 new dir/new name.rs");
        let out = z(&[
            rec.as_bytes(),
            b"old dir/old name.rs",
            &one(".M", "after.rs"),
        ]);
        let s = parse_status(&out).unwrap();
        assert_eq!(s.entries.len(), 2);
        assert_eq!(s.entries[0].display_path(), "new dir/new name.rs");
        assert_eq!(
            s.entries[0].kind,
            StatusKind::Renamed {
                staged: FileState::Renamed,
                unstaged: FileState::Unmodified,
                orig_path: PathBuf::from("old dir/old name.rs"),
                op: RenameOp::Rename,
                score: 100,
            }
        );
        // The record after the rename is still parsed normally.
        assert_eq!(s.entries[1].display_path(), "after.rs");
    }

    #[test]
    fn rename_orig_path_that_looks_like_a_record_is_not_misread() {
        let rec = format!("2 RM N... 100644 100644 100644 {H} {H} R85 b");
        let out = z(&[rec.as_bytes(), b"? tricky"]);
        let s = parse_status(&out).unwrap();
        assert_eq!(s.entries.len(), 1);
        let StatusKind::Renamed {
            orig_path,
            score,
            unstaged,
            ..
        } = &s.entries[0].kind
        else {
            panic!("expected rename");
        };
        assert_eq!(orig_path, Path::new("? tricky"));
        assert_eq!(*score, 85);
        assert_eq!(*unstaged, FileState::Modified);
    }

    #[test]
    fn copy_record() {
        let rec = format!("2 C. N... 100644 100644 100644 {H} {H} C75 copy.rs");
        let s = parse_status(&z(&[rec.as_bytes(), b"orig.rs"])).unwrap();
        let StatusKind::Renamed { op, score, .. } = &s.entries[0].kind else {
            panic!()
        };
        assert_eq!((*op, *score), (RenameOp::Copy, 75));
    }

    #[test]
    fn unmerged_kinds() {
        let u = |xy: &str, p: &str| {
            format!("u {xy} N... 100644 100644 100644 100644 {H} {H} {H} {p}").into_bytes()
        };
        let all = [
            ("DD", ConflictKind::BothDeleted),
            ("AU", ConflictKind::AddedByUs),
            ("UD", ConflictKind::DeletedByThem),
            ("UA", ConflictKind::AddedByThem),
            ("DU", ConflictKind::DeletedByUs),
            ("AA", ConflictKind::BothAdded),
            ("UU", ConflictKind::BothModified),
        ];
        for (xy, expected) in all {
            let s = parse_status(&z(&[&u(xy, "src/a b.rs")])).unwrap();
            assert_eq!(s.entries[0].display_path(), "src/a b.rs");
            assert_eq!(
                s.entries[0].kind,
                StatusKind::Unmerged { conflict: expected }
            );
        }
    }

    #[test]
    fn untracked_and_ignored() {
        let s = parse_status(&z(&[b"? a/b/c.txt", b"! target/out.o"])).unwrap();
        assert_eq!(s.entries[0].kind, StatusKind::Untracked);
        assert_eq!(s.entries[1].kind, StatusKind::Ignored);
        assert_eq!(paths(&s), ["a/b/c.txt", "target/out.o"]);
    }

    #[test]
    fn empty_output_is_empty_snapshot() {
        assert_eq!(parse_status(b"").unwrap(), StatusSnapshot::default());
    }

    #[test]
    fn full_example_mixed_order() {
        let rec2 = format!("2 R. N... 100644 100644 100644 {H} {H} R100 b.rs");
        let out = z(&[
            format!("# branch.oid {Z}").as_bytes(),
            b"# branch.head main",
            rec2.as_bytes(),
            b"a.rs",
            b"? z.txt",
            &one(".M", "m.rs"),
        ]);
        let s = parse_status(&out).unwrap();
        assert_eq!(paths(&s), ["b.rs", "z.txt", "m.rs"]);
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        // truncated rename (no orig path), unknown tag, short record, bad XY, bad score
        assert!(parse_status(&z(&[format!("2 R. N... 1 1 1 {H} {H} R100 b").as_bytes()])).is_err());
        assert!(parse_status(&z(&[b"X nope"])).is_err());
        assert!(parse_status(&z(&[b"1"])).is_err());
        assert!(parse_status(&z(&[b"1 .M N..."])).is_err());
        assert!(parse_status(&z(&[&one("Q.", "x")])).is_err());
        assert!(parse_status(&z(&[&one("M", "x")])).is_err());
        let bad_score = format!("2 R. N... 1 1 1 {H} {H} R999 b");
        assert!(parse_status(&z(&[bad_score.as_bytes(), b"a"])).is_err());
        assert!(parse_status(&z(&[b"# branch.ab +x -y"])).is_err());
        assert!(parse_status(&z(&[b"# branch.oid nothex"])).is_err());
        assert!(parse_status(&z(&[b"u ZZ N... 1 1 1 1 a a a p"])).is_err());
    }

    #[test]
    fn split_fields_keeps_remainder_with_spaces() {
        let f = split_fields::<3>(b"a b c d e").unwrap();
        assert_eq!(f, [&b"a"[..], &b"b"[..], &b"c d e"[..]]);
        assert!(split_fields::<3>(b"a b").is_none());
    }
}
