//! Read-only probe of the git layer.
//!
//! ```text
//! cargo run --example git_probe -- <path inside a git repository>
//! ```
//!
//! Prints the repository, every worktree (id, branch, base, ahead/behind) and a
//! status summary per worktree. Everything goes through `GitRunner`, which sets
//! `GIT_OPTIONAL_LOCKS=0`, so running this against a repo where an agent is
//! working changes nothing and takes no locks.
//!
//! (`#[tokio::main]` is fine here: an example is its own binary, not library
//! code inside the Tauri app, which owns the app's runtime.)

use std::path::PathBuf;
use std::process::ExitCode;

use why_lib::git::{
    BranchHead, FileState, GitError, GitRunner, StatusKind, StatusSnapshot, WorktreeEntry,
    detect_base, list_worktrees, open_repo, read_status,
};

#[tokio::main]
async fn main() -> ExitCode {
    let Some(arg) = std::env::args_os().nth(1) else {
        eprintln!("usage: cargo run --example git_probe -- <repo path>");
        return ExitCode::from(2);
    };
    match run(PathBuf::from(arg)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(path: PathBuf) -> Result<(), GitError> {
    let runner = GitRunner::new();
    let repo = open_repo(&runner, &path).await?;

    println!("repository");
    println!(
        "  toplevel   {}",
        repo.toplevel
            .as_ref()
            .map_or("(bare)".into(), |p| p.display().to_string())
    );
    println!("  common_dir {}", repo.common_dir.display());
    println!("  git_dir    {}", repo.git_dir.display());
    println!("  bare       {}", repo.is_bare);

    let worktrees = list_worktrees(&runner, &repo).await?;
    println!("\nworktrees ({})", worktrees.len());

    for wt in &worktrees {
        println!("\n  id {}  {}", wt.id(&repo.common_dir), wt.path.display());
        println!("     {}", describe(wt));

        // A prunable worktree's directory is gone and a bare repo has no
        // working files: git cannot run status/base there. A *locked* worktree
        // whose directory was deleted is NOT reported as prunable (the lock
        // exists to prevent pruning), so also check the directory itself.
        if wt.prunable.is_some() || wt.is_bare || !wt.path.is_dir() {
            println!("     (no working directory: skipping base/status)");
            continue;
        }

        // One broken worktree must not hide the others: report and carry on.
        if let Err(e) = probe_worktree(&runner, wt).await {
            println!("     error  {e}");
        }
    }
    Ok(())
}

async fn probe_worktree(runner: &GitRunner, wt: &WorktreeEntry) -> Result<(), GitError> {
    match detect_base(runner, &wt.path, None).await? {
        Some(b) => println!(
            "     base   {} (merge-base {}), ahead {} / behind {}",
            b.base_ref,
            b.merge_base.short(),
            b.ahead,
            b.behind
        ),
        None => println!("     base   none"),
    }
    let status = read_status(runner, &wt.path).await?;
    println!("     status {}", summarize(&status));
    Ok(())
}

fn describe(wt: &WorktreeEntry) -> String {
    let mut parts = Vec::new();
    if wt.is_main {
        parts.push("main".to_string());
    }
    match (wt.branch_short(), wt.is_detached) {
        (Some(b), _) => parts.push(format!("branch {b}")),
        (None, true) => parts.push("detached".into()),
        (None, false) => {}
    }
    if let Some(h) = &wt.head {
        parts.push(format!("HEAD {}", h.short()));
    }
    if let Some(r) = &wt.locked {
        parts.push(format!("locked ({r})"));
    }
    if let Some(r) = &wt.prunable {
        parts.push(format!("prunable ({r})"));
    }
    parts.join(", ")
}

fn summarize(s: &StatusSnapshot) -> String {
    let (mut staged, mut unstaged, mut untracked, mut conflicts) = (0, 0, 0, 0);
    for e in &s.entries {
        match &e.kind {
            StatusKind::Changed {
                staged: x,
                unstaged: y,
            }
            | StatusKind::Renamed {
                staged: x,
                unstaged: y,
                ..
            } => {
                staged += usize::from(*x != FileState::Unmodified);
                unstaged += usize::from(*y != FileState::Unmodified);
            }
            StatusKind::Unmerged { .. } => conflicts += 1,
            StatusKind::Untracked => untracked += 1,
            StatusKind::Ignored => {}
        }
    }
    let head = match &s.branch.head {
        Some(BranchHead::Branch(b)) => b.clone(),
        Some(BranchHead::Detached) => "(detached)".into(),
        None => "?".into(),
    };
    let upstream = match (&s.branch.upstream, s.branch.ahead_behind) {
        (Some(u), Some(ab)) => format!(" tracking {u} +{} -{}", ab.ahead, ab.behind),
        (Some(u), None) => format!(" tracking {u} (gone)"),
        _ => String::new(),
    };
    format!(
        "{head}{upstream}: {} entries = {staged} staged, {unstaged} unstaged, {untracked} untracked, {conflicts} conflicts",
        s.entries.len()
    )
}
