//! Regenerate the Codex timeline snapshots after an intentional change.
//! Run `cargo run --manifest-path src-tauri/Cargo.toml --example update_codex_golden`
//! from the repo root, then review the snapshot diff.

#[path = "../tests/adapters/codex_common.rs"]
mod codex_common;
#[path = "../tests/adapters/codex_snapshot.rs"]
mod codex_snapshot;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    for (name, hooks) in codex_common::scenarios() {
        let path = codex_snapshot::golden_path(&name);
        let actual = codex_snapshot::snapshot(&hooks);
        if std::fs::read_to_string(&path).is_ok_and(|expected| expected == actual) {
            continue;
        }
        std::fs::write(&path, actual).with_context(|| format!("writing {}", path.display()))?;
        println!("Updated {name}");
    }
    Ok(())
}
