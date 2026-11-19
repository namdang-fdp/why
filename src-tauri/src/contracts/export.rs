//! Generating the TypeScript bindings into `ui/src/contracts/`, and the guard that
//! fails when the committed files are out of date.
//!
//! We do not use `#[ts(export)]` on the types: that makes every `cargo test` rewrite
//! the files silently, which would hide drift instead of reporting it. Instead,
//! generation is explicit (`pnpm gen:contracts` runs [`regenerate_bindings`]) and
//! [`committed_bindings_are_up_to_date`] compares a fresh export with what is committed.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use ts_rs::{Config, TS};

use super::{
    DiffMode, FileChange, FileDiff, IpcError, REPOSITORY_CHANGED_EVENT, Repository,
    RepositoryChanged, StorageStatus, WORKTREE_CHANGED_EVENT, Worktree, WorktreeChanged,
};

/// Where the committed bindings live, relative to this crate (`src-tauri/`).
fn committed_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/src/contracts")
}

/// Write every contract type, plus `events.ts` and an `index.ts` barrel, into `dir`.
fn export_all(dir: &Path) -> Result<()> {
    let cfg = Config::new().with_out_dir(dir);

    // `export_all` writes the type and, recursively, everything it depends on
    // (ids, BranchInfo, ChangeStatus, ...). So we only list the "root" types.
    Repository::export_all(&cfg)?;
    Worktree::export_all(&cfg)?;
    DiffMode::export_all(&cfg)?;
    FileChange::export_all(&cfg)?;
    FileDiff::export_all(&cfg)?;
    WorktreeChanged::export_all(&cfg)?;
    RepositoryChanged::export_all(&cfg)?;
    IpcError::export_all(&cfg)?;
    StorageStatus::export_all(&cfg)?;

    const NOTE: &str = "// This file was generated from the Rust contracts in src-tauri/src/contracts. Do not edit it manually.\n// Regenerate with `pnpm gen:contracts`.\n";

    fs::write(
        dir.join("events.ts"),
        format!(
            "{NOTE}\nexport const WORKTREE_CHANGED_EVENT = \"{WORKTREE_CHANGED_EVENT}\";\nexport const REPOSITORY_CHANGED_EVENT = \"{REPOSITORY_CHANGED_EVENT}\";\n"
        ),
    )?;

    // Barrel: one `export type` line per generated type file, sorted for stable output.
    let mut names: Vec<String> = read_ts_files(dir)?
        .into_keys()
        .filter_map(|file| file.strip_suffix(".ts").map(str::to_owned))
        .filter(|stem| stem != "index" && stem != "events")
        .collect();
    names.sort();
    let mut index = format!("{NOTE}\nexport * from \"./events\";\n");
    for name in names {
        index.push_str(&format!("export type {{ {name} }} from \"./{name}\";\n"));
    }
    fs::write(dir.join("index.ts"), index)?;
    Ok(())
}

/// Read every `*.ts` file of `dir` into a map of file name -> contents.
/// A missing directory counts as empty.
fn read_ts_files(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(files);
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "ts") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = fs::read_to_string(&path).with_context(|| format!("reading {path:?}"))?;
            // Compare line content, not line endings: with `core.autocrlf=true` git may
            // check the committed files out with CRLF while the generator writes LF.
            // `.gitattributes` pins LF too; this keeps the guard robust if it is bypassed.
            files.insert(name, text.replace("\r\n", "\n"));
        }
    }
    Ok(files)
}

/// A fresh, empty directory under the OS temp dir (no extra crate needed).
fn scratch_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("why-contracts-{}-{nanos}", std::process::id()))
}

/// Run with `pnpm gen:contracts`. It is `#[ignore]`d so a plain `cargo test` never
/// writes into the source tree; `-- --ignored` opts in.
#[test]
#[ignore = "writes ui/src/contracts; run via `pnpm gen:contracts`"]
fn regenerate_bindings() {
    let dir = committed_dir();
    // Delete old files first so a renamed or removed Rust type does not leave a stale `.ts` behind.
    for name in read_ts_files(&dir).unwrap().into_keys() {
        fs::remove_file(dir.join(name)).unwrap();
    }
    export_all(&dir).unwrap();
}

#[test]
fn committed_bindings_are_up_to_date() {
    let fresh_dir = scratch_dir();
    export_all(&fresh_dir).unwrap();
    let fresh = read_ts_files(&fresh_dir).unwrap();
    let committed = read_ts_files(&committed_dir()).unwrap();
    fs::remove_dir_all(&fresh_dir).ok();

    let mut problems = Vec::new();
    for (name, text) in &fresh {
        match committed.get(name) {
            None => problems.push(format!("  missing: {name}")),
            Some(old) if old != text => problems.push(format!("  changed: {name}")),
            Some(_) => {}
        }
    }
    for name in committed.keys().filter(|n| !fresh.contains_key(*n)) {
        problems.push(format!("  stale (no Rust type any more): {name}"));
    }

    assert!(
        problems.is_empty(),
        "TypeScript contracts in ui/src/contracts are out of date with the Rust types.\n\
         Run `pnpm gen:contracts` and commit the result.\n{}",
        problems.join("\n")
    );
}

#[test]
fn bindings_never_use_bigint() {
    // JSON numbers become plain JS `number`s, so a `bigint` type would lie at runtime.
    let dir = scratch_dir();
    export_all(&dir).unwrap();
    for (name, text) in read_ts_files(&dir).unwrap() {
        assert!(
            !text.contains("bigint"),
            "{name} uses bigint; use u32/i32 in contracts"
        );
    }
    fs::remove_dir_all(&dir).ok();
}
