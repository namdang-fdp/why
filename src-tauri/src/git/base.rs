//! Base detection: "what is this worktree's work being compared against?"
//!
//! WHY shows a reviewer *what the agent changed on this branch*. That needs a
//! **base**: the branch the work will be merged into (usually `main`). We detect
//! it automatically and let the user override it.
//!
//! ## Why the diff starts at the merge-base, not at the base tip
//!
//! ```text
//!          A---B---C   feature (HEAD)
//!         /
//!    ----O---X---Y     main (base)
//!         ^
//!         merge-base(main, feature) = O
//! ```
//!
//! `git diff main HEAD` (two-dot) compares the *tips* `Y` and `C`. It would show
//! X and Y's changes **reversed** (as if the agent had deleted them), because
//! main moved on after the branch was cut. The reviewer wants only A, B, C:
//! `git diff main...HEAD` (three-dot) means `git diff $(git merge-base main HEAD) HEAD`,
//! i.e. "changes on this side since the histories diverged". So we compute and
//! keep the merge-base commit itself; later diff code runs against it.
//!
//! `ahead`/`behind` say how far the two sides have moved apart since `O`:
//! `ahead` = commits only on the worktree's side (A, B, C = 3),
//! `behind` = commits only on the base (X, Y = 2).

use std::path::Path;

use super::error::{GitError, ParseError};
use super::oid::Oid;
use super::runner::GitRunner;

/// What a worktree is compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseInfo {
    /// Human-readable name of the base: `origin/main`, `main`, or exactly what
    /// the caller passed as an override.
    pub base_ref: String,
    /// Common ancestor of the base and the worktree's HEAD. Diffs start here.
    pub merge_base: Oid,
    /// Commits reachable from HEAD but not from the base.
    pub ahead: u32,
    /// Commits reachable from the base but not from HEAD.
    pub behind: u32,
}

/// Fully-qualified refs we try, in order, when no override is given. The first
/// entry is special: `refs/remotes/origin/HEAD` is a *symbolic ref* (a pointer
/// to another ref) that `git clone` creates to record the remote's default
/// branch, so its target is the best answer to "what is main called here?".
const ORIGIN_HEAD: &str = "refs/remotes/origin/HEAD";
const FALLBACKS: [&str; 4] = [
    "refs/remotes/origin/main",
    "refs/remotes/origin/master",
    "refs/heads/main",
    "refs/heads/master",
];

/// A ref we can compare against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// Full ref name, e.g. `refs/remotes/origin/main`.
    pub name: String,
    pub oid: Oid,
}

/// One line of `git for-each-ref --format='%(refname)%00%(objectname)%00%(symref)'`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefRow {
    pub name: String,
    pub oid: Oid,
    /// Target ref name if `name` is a symbolic ref.
    pub symref: Option<String>,
}

/// Find the base of the worktree at `worktree` and measure the divergence.
///
/// ## Which base wins
///
/// 1. **`override_ref`**, if it names a commit that is not the worktree's own
///    branch (or `HEAD`) and shares history with HEAD. It wins outright.
/// 2. Otherwise every existing candidate is measured, and the one with the
///    **smallest `ahead`** wins; ties go to the earlier candidate in this
///    priority order: target of `refs/remotes/origin/HEAD`, `origin/main`,
///    `origin/master`, `main`, `master`.
///
/// Why "smallest ahead" instead of a fixed priority: agent worktrees are cut
/// from the *local* `main`, which is often ahead of `origin/main` (work merged
/// but not pushed). Against `origin/main` such a worktree would show those
/// already-merged commits as if the agent had written them. Against the local
/// `main` it shows only the agent's own commits, and `ahead` counts exactly
/// the commits that are not in the candidate, so the candidate the branch was
/// really cut from has the smallest `ahead`. When the numbers tie (e.g. a clean
/// clone where `main == origin/main`) the conventional order decides, which
/// prefers the remote's idea of the default branch.
///
/// A candidate is skipped if it does not exist, is the worktree's own branch
/// (comparing a branch with itself says nothing), or shares no history with HEAD.
///
/// Returns `Ok(None)` when there is no usable base: nothing matched, the branch
/// has no commits yet, or the histories are unrelated. That is a normal state
/// for a fresh repo, not an error. `Err` is reserved for git failing (timeout,
/// not a repository, ...).
pub async fn detect_base(
    runner: &GitRunner,
    worktree: &Path,
    override_ref: Option<&str>,
) -> Result<Option<BaseInfo>, GitError> {
    // Resolve HEAD to a commit id ONCE and use the id for every later command.
    // An agent may commit while we run; using the id keeps all numbers
    // consistent with each other. `--quiet --verify` exits 1 silently when HEAD
    // is unborn (a repo with no commits).
    let head = match runner
        .run(worktree, ["rev-parse", "--verify", "--quiet", "HEAD"])
        .await
    {
        Ok(out) => Oid::from_bytes(out.trim_ascii())?,
        Err(e) if e.is_exit_code(1) => return Ok(None),
        Err(e) => return Err(e),
    };

    // Full name of the branch HEAD points at (`refs/heads/x`); exit 1 if detached.
    let own_branch = match runner
        .run(worktree, ["symbolic-ref", "--quiet", "HEAD"])
        .await
    {
        Ok(out) => Some(String::from_utf8_lossy(out.trim_ascii()).into_owned()),
        Err(e) if e.is_exit_code(1) => None,
        Err(e) => return Err(e),
    };

    // 1. An explicit override wins outright (if it is usable).
    if let Some(name) = override_ref
        && let Some(oid) = resolve_override(runner, worktree, name, own_branch.as_deref()).await?
        && let Some(info) = measure(runner, worktree, name.to_string(), &oid, &head).await?
    {
        return Ok(Some(info));
    }

    // 2. Auto-detect. One `for-each-ref` call answers "which of these exist,
    //    what do they point at, and where does origin/HEAD point" at once.
    let mut args = vec![
        "for-each-ref".to_string(),
        "--format=%(refname)%00%(objectname)%00%(symref)".to_string(),
        ORIGIN_HEAD.to_string(),
    ];
    args.extend(FALLBACKS.iter().map(|s| s.to_string()));
    let rows = parse_for_each_ref(&runner.run(worktree, &args).await?)?;

    let mut measured = Vec::new();
    let mut seen: Vec<Oid> = Vec::new();
    for c in auto_candidates(&rows, own_branch.as_deref()) {
        // Same commit as a higher-priority candidate (typically `main` ==
        // `origin/main`): identical numbers, and the earlier one wins ties, so
        // skip the two git calls.
        if seen.contains(&c.oid) {
            continue;
        }
        seen.push(c.oid.clone());
        let name = short_ref_name(&c.name).to_string();
        if let Some(info) = measure(runner, worktree, name, &c.oid, &head).await? {
            measured.push(info);
        }
    }
    Ok(pick_best(measured))
}

/// Of the measured candidates (given in priority order) pick the one with the
/// smallest `ahead`; the first one wins a tie. Pure.
pub(crate) fn pick_best(measured: Vec<BaseInfo>) -> Option<BaseInfo> {
    // `Iterator::min_by_key` returns the FIRST minimum, which is what we want.
    measured.into_iter().min_by_key(|b| b.ahead)
}

/// Compute merge-base and ahead/behind of `head` against `base_oid`.
/// `None` if they share no history.
async fn measure(
    runner: &GitRunner,
    worktree: &Path,
    base_ref: String,
    base_oid: &Oid,
    head: &Oid,
) -> Result<Option<BaseInfo>, GitError> {
    // `git merge-base A B` prints the best common ancestor; exit 1 = none.
    let merge_base = match runner
        .run(worktree, ["merge-base", base_oid.as_str(), head.as_str()])
        .await
    {
        Ok(out) => Oid::from_bytes(out.trim_ascii())?,
        Err(e) if e.is_exit_code(1) => return Ok(None),
        Err(e) => return Err(e),
    };

    // `A...B` in rev-list means "symmetric difference"; with --left-right
    // --count git prints "<only in A>\t<only in B>". Here A = base, B = HEAD,
    // so the first number is `behind` and the second is `ahead`.
    let range = format!("{base_oid}...{head}");
    let out = runner
        .run(
            worktree,
            ["rev-list", "--left-right", "--count", range.as_str()],
        )
        .await?;
    let (behind, ahead) = parse_left_right_count(&out)?;

    Ok(Some(BaseInfo {
        base_ref,
        merge_base,
        ahead,
        behind,
    }))
}

/// Resolve a user-supplied base to a commit id, or `None` if it does not exist
/// or must not be used (it IS the worktree's own branch, or `HEAD` itself).
///
/// "Is it the own branch" is decided by what the revision *resolves to*, not by
/// how it is spelled: `git rev-parse --symbolic-full-name` turns `main`,
/// `heads/main`, `refs/heads/main` and `HEAD` (on that branch) all into
/// `refs/heads/main`. We deliberately do NOT compare commit ids: two distinct
/// branches may legitimately point at the same commit.
async fn resolve_override(
    runner: &GitRunner,
    worktree: &Path,
    rev: &str,
    own_branch: Option<&str>,
) -> Result<Option<Oid>, GitError> {
    // `--end-of-options` stops a value that starts with `-` from being read as a flag.
    let Some(oid) = resolve_commit(runner, worktree, rev).await? else {
        return Ok(None);
    };
    // Prints the full ref name, or nothing for a bare commit id / `HEAD~1`.
    // `--verify` matters: without it rev-parse also echoes `--end-of-options`
    // back as an output line.
    let full = runner
        .run(
            worktree,
            [
                "rev-parse",
                "--verify",
                "--symbolic-full-name",
                "--end-of-options",
                rev,
            ],
        )
        .await?;
    let full = String::from_utf8_lossy(full.trim_ascii());
    Ok((!is_own_ref(&full, own_branch)).then_some(oid))
}

/// Is a resolved full ref name the worktree's own branch (or `HEAD` itself)?
fn is_own_ref(resolved: &str, own_branch: Option<&str>) -> bool {
    resolved == "HEAD" || (!resolved.is_empty() && own_branch == Some(resolved))
}

/// Resolve any revision (branch, tag, sha, `HEAD~3`) to a commit id, or `None`
/// if it does not exist.
async fn resolve_commit(
    runner: &GitRunner,
    worktree: &Path,
    rev: &str,
) -> Result<Option<Oid>, GitError> {
    let spec = format!("{rev}^{{commit}}");
    match runner
        .run(
            worktree,
            [
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                spec.as_str(),
            ],
        )
        .await
    {
        Ok(out) => Ok(Some(Oid::from_bytes(out.trim_ascii())?)),
        Err(e) if e.is_exit_code(1) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `refs/remotes/origin/main` -> `origin/main`, `refs/heads/main` -> `main`.
fn short_ref_name(full: &str) -> &str {
    full.strip_prefix("refs/remotes/")
        .or_else(|| full.strip_prefix("refs/heads/"))
        .unwrap_or(full)
}

/// Apply the auto-detection order to the rows `for-each-ref` returned. Pure.
pub(crate) fn auto_candidates(rows: &[RefRow], own_branch: Option<&str>) -> Vec<Candidate> {
    let mut out = Vec::new();
    let skip = |name: &str| own_branch == Some(name);

    // 1. Where origin/HEAD points. `for-each-ref` reports the *target's* commit
    //    as `objectname` for a symbolic ref, so we need no second lookup.
    if let Some(row) = rows.iter().find(|r| r.name == ORIGIN_HEAD)
        && let Some(target) = &row.symref
        && !skip(target)
    {
        out.push(Candidate {
            name: target.clone(),
            oid: row.oid.clone(),
        });
    }
    // 2..5. Conventional names, in order.
    for wanted in FALLBACKS {
        if skip(wanted) {
            continue;
        }
        if let Some(row) = rows.iter().find(|r| r.name == wanted) {
            out.push(Candidate {
                name: row.name.clone(),
                oid: row.oid.clone(),
            });
        }
    }
    // origin/HEAD usually points at origin/main, which FALLBACKS lists again;
    // drop later duplicates (same ref name).
    let mut seen = Vec::new();
    out.retain(|c| {
        if seen.contains(&c.name) {
            false
        } else {
            seen.push(c.name.clone());
            true
        }
    });
    out
}

/// Parse the `for-each-ref` output above: one ref per line, fields separated by
/// NUL. (Ref names cannot contain newlines or control characters, so splitting
/// on `\n` is safe here.)
pub(crate) fn parse_for_each_ref(out: &[u8]) -> Result<Vec<RefRow>, ParseError> {
    let mut rows = Vec::new();
    for line in out.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let mut fields = line.split(|&b| b == 0);
        let (Some(name), Some(oid), Some(symref), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(ParseError::new("for-each-ref line does not have 3 fields"));
        };
        rows.push(RefRow {
            name: String::from_utf8_lossy(name).into_owned(),
            oid: Oid::from_bytes(oid)?,
            symref: (!symref.is_empty()).then(|| String::from_utf8_lossy(symref).into_owned()),
        });
    }
    Ok(rows)
}

/// Parse `git rev-list --left-right --count A...B` output `"<left>\t<right>\n"`.
/// Returns `(left, right)`.
pub(crate) fn parse_left_right_count(out: &[u8]) -> Result<(u32, u32), ParseError> {
    let text = std::str::from_utf8(out)
        .map_err(|_| ParseError::new("rev-list --count output is not UTF-8"))?;
    let mut parts = text.split_whitespace();
    let (Some(left), Some(right), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(ParseError::new(format!(
            "unexpected rev-list count {text:?}"
        )));
    };
    let num = |s: &str| {
        s.parse::<u32>()
            .map_err(|_| ParseError::new(format!("not a number in rev-list count: {s:?}")))
    };
    Ok((num(left)?, num(right)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(c: char) -> Oid {
        Oid::parse(&c.to_string().repeat(40)).unwrap()
    }

    fn row(name: &str, c: char, symref: Option<&str>) -> RefRow {
        RefRow {
            name: name.into(),
            oid: oid(c),
            symref: symref.map(Into::into),
        }
    }

    #[test]
    fn parses_for_each_ref_rows() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let out = format!(
            "refs/remotes/origin/HEAD\0{a}\0refs/remotes/origin/main\n\
             refs/remotes/origin/main\0{a}\0\n\
             refs/heads/main\0{b}\0\n"
        );
        let rows = parse_for_each_ref(out.as_bytes()).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].symref.as_deref(), Some("refs/remotes/origin/main"));
        assert_eq!(rows[1].symref, None);
        assert_eq!(rows[2].oid.as_str(), b);
        assert!(parse_for_each_ref(b"").unwrap().is_empty());
        assert!(parse_for_each_ref(b"refs/heads/x\0nothex\0\n").is_err());
        assert!(parse_for_each_ref(b"refs/heads/x\n").is_err());
    }

    #[test]
    fn left_right_count_order_is_left_then_right() {
        assert_eq!(parse_left_right_count(b"2\t1\n").unwrap(), (2, 1));
        assert_eq!(parse_left_right_count(b"0\t0\n").unwrap(), (0, 0));
        assert!(parse_left_right_count(b"1\n").is_err());
        assert!(parse_left_right_count(b"a\tb\n").is_err());
        assert!(parse_left_right_count(b"1\t2\t3\n").is_err());
    }

    #[test]
    fn origin_head_target_wins_and_duplicates_collapse() {
        let rows = vec![
            row(
                "refs/remotes/origin/HEAD",
                'a',
                Some("refs/remotes/origin/develop"),
            ),
            row("refs/remotes/origin/main", 'b', None),
            row("refs/heads/main", 'c', None),
        ];
        let names: Vec<_> = auto_candidates(&rows, Some("refs/heads/feat"))
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            names,
            [
                "refs/remotes/origin/develop",
                "refs/remotes/origin/main",
                "refs/heads/main"
            ]
        );
        // The develop candidate carries origin/HEAD's own object id.
        let first = &auto_candidates(&rows, None)[0];
        assert_eq!(first.oid, oid('a'));
    }

    #[test]
    fn typical_clone_has_origin_head_pointing_at_main() {
        let rows = vec![
            row(
                "refs/remotes/origin/HEAD",
                'a',
                Some("refs/remotes/origin/main"),
            ),
            row("refs/remotes/origin/main", 'a', None),
        ];
        let c = auto_candidates(&rows, None);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "refs/remotes/origin/main");
    }

    #[test]
    fn own_branch_is_skipped() {
        let rows = vec![
            row("refs/heads/main", 'a', None),
            row("refs/heads/master", 'b', None),
        ];
        let c = auto_candidates(&rows, Some("refs/heads/main"));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "refs/heads/master");
        assert!(auto_candidates(&rows[..1], Some("refs/heads/main")).is_empty());
    }

    #[test]
    fn own_ref_detection() {
        assert!(is_own_ref("refs/heads/main", Some("refs/heads/main")));
        assert!(is_own_ref("HEAD", Some("refs/heads/main")));
        assert!(is_own_ref("HEAD", None)); // detached: HEAD vs HEAD
        assert!(!is_own_ref(
            "refs/remotes/origin/main",
            Some("refs/heads/main")
        ));
        assert!(!is_own_ref("refs/heads/other", None));
        // A bare commit id resolves to no ref name: usable as a base.
        assert!(!is_own_ref("", Some("refs/heads/main")));
        assert!(!is_own_ref("", None));
    }

    fn info(name: &str, ahead: u32, behind: u32) -> BaseInfo {
        BaseInfo {
            base_ref: name.into(),
            merge_base: oid('a'),
            ahead,
            behind,
        }
    }

    #[test]
    fn pick_best_prefers_smallest_ahead_then_priority() {
        let best = pick_best(vec![info("origin/main", 3, 0), info("main", 1, 2)]).unwrap();
        assert_eq!(best.base_ref, "main");
        // Tie on ahead: the earlier (higher priority) candidate wins, even if
        // it is further behind.
        let best = pick_best(vec![info("origin/main", 2, 5), info("main", 2, 0)]).unwrap();
        assert_eq!(best.base_ref, "origin/main");
        assert_eq!(pick_best(vec![]), None);
    }

    #[test]
    fn short_names() {
        assert_eq!(short_ref_name("refs/remotes/origin/main"), "origin/main");
        assert_eq!(short_ref_name("refs/heads/feat/x"), "feat/x");
        assert_eq!(short_ref_name("refs/tags/v1"), "refs/tags/v1");
    }
}
