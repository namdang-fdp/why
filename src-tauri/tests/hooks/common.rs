//! Fixture: a throw-away home, data dir and fake bundled `why-hook`.

#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::TempDir;
use why_lib::contracts::{ApplyResult, HookHarness, HookPreview, PreviewResult};
use why_lib::hooks::{HookContext, HooksEnv};
use why_wire::{Instance, InstancePaths};

pub const CLAUDE: HookHarness = HookHarness::ClaudeCode;
pub const CODEX: HookHarness = HookHarness::Codex;

pub struct Fx {
    pub tmp: TempDir,
    pub home: PathBuf,
    pub bundled: PathBuf,
    pub ctx: HookContext,
}

/// A fake `why-hook`: records its arguments, one per line, into `$WHY_TEST_OUT`.
pub const FAKE_HOOK: &str =
    "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$WHY_TEST_OUT\"\n";

impl Fx {
    pub fn new() -> Self {
        Self::with_data_dir_name("data")
    }

    /// `name` is the installed data dir's folder name (hostile names allowed).
    pub fn with_data_dir_name(name: &str) -> Self {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir(&home).unwrap();
        let bundle = tmp.path().join("bundle");
        fs::create_dir(&bundle).unwrap();
        let bundled = bundle.join("why-hook");
        fs::write(&bundled, FAKE_HOOK).unwrap();
        fs::set_permissions(&bundled, fs::Permissions::from_mode(0o755)).unwrap();
        let ctx = Self::context(
            &tmp,
            &home,
            Some(bundled.clone()),
            Instance::Installed,
            name,
        );
        Fx {
            tmp,
            home,
            bundled,
            ctx,
        }
    }

    fn context(
        tmp: &TempDir,
        home: &Path,
        bundled: Option<PathBuf>,
        instance: Instance,
        data_name: &str,
    ) -> HookContext {
        let installed_data = tmp.path().join(data_name);
        let paths = InstancePaths::from_installed_data_dir(instance, &installed_data, None);
        HookContext::new(HooksEnv::for_home(home, bundled), paths)
    }

    /// The same machine, seen by a dev instance on `port`.
    pub fn dev(&self, port: u16) -> HookContext {
        let mut ctx = self.ctx.clone();
        let installed_data = ctx.paths.data_dir.clone();
        ctx.paths =
            InstancePaths::from_installed_data_dir(Instance::Dev { port }, &installed_data, None);
        ctx
    }

    pub fn claude_file(&self) -> PathBuf {
        self.home.join(".claude/settings.json")
    }

    pub fn codex_file(&self) -> PathBuf {
        self.home.join(".codex/hooks.json")
    }

    pub fn write(&self, path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    pub fn read(&self, path: &Path) -> Option<Vec<u8>> {
        fs::read(path).ok()
    }
}

pub fn ready(r: PreviewResult) -> HookPreview {
    match r {
        PreviewResult::Ready { preview } => preview,
        other => panic!("expected a preview, got {other:?}"),
    }
}

/// Preview then install with the previewed fingerprint (the UI's happy path).
pub fn install(ctx: &HookContext, h: HookHarness) -> ApplyResult {
    let preview = ready(ctx.preview_install_result(h.into()).unwrap());
    ctx.install_result(h.into(), &preview.fingerprint).unwrap()
}

pub fn uninstall(ctx: &HookContext, h: HookHarness) -> ApplyResult {
    let preview = ready(ctx.preview_uninstall_result(h.into()).unwrap());
    ctx.uninstall_result(h.into(), &preview.fingerprint)
        .unwrap()
}

pub fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}
