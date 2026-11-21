//! Explicitly regenerate the checked-in snapshots from the captured hooks.
//! From the repository root:
//! `cargo run --manifest-path src-tauri/Cargo.toml --example regenerate_claude_goldens`

use anyhow::{Context, Result};

// Fixture construction and assertions in common are used only by the tests.
#[allow(dead_code)]
#[path = "../tests/adapters/common.rs"]
mod common;
#[path = "../tests/adapters/claude_golden/render.rs"]
mod render;

fn main() -> Result<()> {
    tauri::async_runtime::block_on(async {
        for (name, envs) in common::scenarios() {
            let path = common::golden_path(&name);
            let snapshot = render::render(&common::replay(&envs).await);
            let mut bytes = serde_json::to_vec_pretty(&snapshot)?;
            bytes.push(b'\n');
            std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        }
        Ok(())
    })
}
