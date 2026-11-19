//! Read-only probe of the diff service against a real repository.
//!
//! ```text
//! cargo run --release --example diff_probe -- <path inside a git repository>
//! ```
//!
//! Registers the repository in a throw-away in-memory store, lists its
//! worktrees, and for each one times `get_changes` in every mode and opens one
//! file diff. It also proves the passive-observer promise: every worktree's
//! `.git/index` (size, mtime, inode) is compared before and after, and a
//! background task polls for an `index.lock` file the whole time.
//!
//! (`#[tokio::main]` is fine here: an example is its own binary, not library
//! code inside the Tauri app, which owns the app's runtime.)

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use why_lib::contracts::DiffMode;
use why_lib::git::{GitRunner, open_repo};
use why_lib::ipc::changes::{get_changes_impl, get_file_diff_impl, list_worktrees_impl};
use why_lib::ipc::repos::add_repository_impl;
use why_lib::state::AppState;
use why_lib::store::MemoryRepoStore;

#[tokio::main]
async fn main() -> ExitCode {
    let Some(arg) = std::env::args_os().nth(1) else {
        eprintln!("usage: cargo run --example diff_probe -- <repo path>");
        return ExitCode::from(2);
    };
    match run(PathBuf::from(arg)).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Inode number: Unix only (an atomic rewrite of the index changes it even when
/// size and mtime happen to match). Elsewhere size and mtime have to do.
#[cfg(unix)]
fn inode(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.ino()
}

#[cfg(not(unix))]
fn inode(_: &std::fs::Metadata) -> u64 {
    0
}

type Fingerprint = Option<(u64, SystemTime, u64)>;

/// `(size, mtime, inode)` of a file, `None` if it does not exist.
fn fingerprint(path: &Path) -> Fingerprint {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?, inode(&m)))
}

fn ms(d: Duration) -> String {
    format!("{:7.1} ms", d.as_secs_f64() * 1e3)
}

async fn run(path: PathBuf) -> Result<bool, Box<dyn std::error::Error>> {
    let state = AppState::new(GitRunner::new(), Arc::new(MemoryRepoStore::new()));
    let repo = add_repository_impl(&state, path.to_str().ok_or("path is not UTF-8")?).await?;
    println!("repository {} ({})", repo.name, repo.root_path);

    // Setup (not measured): which worktrees exist and where each keeps its
    // private git directory (its own `index` lives there).
    let worktrees = list_worktrees_impl(&state, &repo.id).await?;
    let mut git_dirs: Vec<PathBuf> = Vec::new();
    for w in worktrees.iter().filter(|w| !w.is_prunable) {
        git_dirs.push(open_repo(&state.git, Path::new(&w.path)).await?.git_dir);
    }
    let before: Vec<Fingerprint> = git_dirs
        .iter()
        .map(|d| fingerprint(&d.join("index")))
        .collect();

    // Poll for `index.lock` every millisecond while we work.
    let stop = Arc::new(AtomicBool::new(false));
    let saw_lock = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (stop, saw_lock, dirs) = (stop.clone(), saw_lock.clone(), git_dirs.clone());
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                if dirs.iter().any(|d| d.join("index.lock").exists()) {
                    saw_lock.store(true, Ordering::Relaxed);
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
    };

    // `list_worktrees` is part of what is being checked, so it runs again here,
    // inside the watched window.
    let started = Instant::now();
    let worktrees = list_worktrees_impl(&state, &repo.id).await?;
    println!(
        "list_worktrees: {} in {}",
        worktrees.len(),
        ms(started.elapsed())
    );

    for w in worktrees.iter().filter(|w| !w.is_prunable) {
        println!(
            "\n{}  [{}]  base {}  change_count {}",
            w.path,
            w.branch.as_deref().unwrap_or("(detached)"),
            w.base.as_ref().map_or("-", |b| b.base_ref.as_str()),
            w.change_count
        );
        for mode in [DiffMode::VsBase, DiffMode::Uncommitted, DiffMode::Staged] {
            let t = Instant::now();
            match get_changes_impl(&state, &w.id, mode).await {
                Ok(list) => println!(
                    "  {:<12} {:>4} files  {}",
                    format!("{mode:?}"),
                    list.len(),
                    ms(t.elapsed())
                ),
                Err(e) => println!("  {:<12} {e}", format!("{mode:?}")),
            }
        }
        if let Ok(list) = get_changes_impl(&state, &w.id, DiffMode::Uncommitted).await
            && let Some(first) = list.iter().find(|c| !c.binary && !c.path_lossy)
        {
            let t = Instant::now();
            let d = get_file_diff_impl(&state, &w.id, &first.path, DiffMode::Uncommitted).await?;
            println!(
                "  get_file_diff {:?}: old {} B, new {} B  {}",
                first.path,
                d.old_content.map_or(0, |s| s.len()),
                d.new_content.map_or(0, |s| s.len()),
                ms(t.elapsed())
            );
        }
    }

    stop.store(true, Ordering::Relaxed);
    watcher.await?;

    let after: Vec<Fingerprint> = git_dirs
        .iter()
        .map(|d| fingerprint(&d.join("index")))
        .collect();
    let index_unchanged = before == after;
    let lock_left = git_dirs.iter().any(|d| d.join("index.lock").exists());
    println!("\nindex files unchanged (size, mtime, inode): {index_unchanged}");
    println!(
        "index.lock seen during the run:               {}",
        saw_lock.load(Ordering::Relaxed)
    );
    println!("index.lock present afterwards:                {lock_left}");
    Ok(index_unchanged && !lock_left && !saw_lock.load(Ordering::Relaxed))
}
