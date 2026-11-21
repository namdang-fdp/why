//! Installing WHY's hooks into the user's agent config (WHY-51).
//!
//! WHY observes agents through hooks the harnesses run. This module writes (and
//! removes) WHY's entries in the USER-LEVEL config of each harness, never in a
//! repository (decision 21: a file in the worktree would show up in diffs and
//! could be committed):
//!
//! | Harness | File |
//! |---|---|
//! | Claude Code | `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json` |
//! | Codex | `$CODEX_HOME/hooks.json`, else `~/.codex/hooks.json` |
//!
//! The rules, in the order a change flows through them:
//!
//! 1. **Preview, then apply.** `preview_*` computes the exact before/after text
//!    and a fingerprint of the bytes it read, writing nothing. `install` /
//!    `uninstall` take that fingerprint and refuse if the file changed since:
//!    the user approved one specific diff.
//! 2. **Merge, never clobber** ([`json_edit`]): WHY's entries are spliced in
//!    beside the user's; invalid or oddly shaped files are refused.
//! 3. **Safe writes** ([`config_file`]): plain user-owned files only, temp file +
//!    fsync + rename, an advisory lock, original mode kept.
//! 4. **Byte-identical restore** ([`state`]): the original bytes are kept so an
//!    untouched file is restored exactly; a file the user edited since only
//!    loses WHY's entries.
//! 5. **Policy and trust are not ours to override**: managed policy that forbids
//!    user hooks blocks install ([`policy`]); Codex trust is never bypassed.
//!
//! Everything reads the environment through [`HooksEnv`], never `std::env`, so
//! tests run against temp directories and cannot touch a real home.

pub mod claude;
pub mod codex;
pub mod command;
pub mod config_file;
pub mod diff;
pub mod json_edit;
pub mod policy;
pub mod state;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use thiserror::Error;
use why_wire::{InstancePaths, PathEnv};

use crate::collector::install::{
    bundled_hook_path, install_hook_binary_from, is_executable, stable_hook_path,
};
use crate::collector::{CollectorStatus, StatsSnapshot};
use crate::contracts::{
    ApplyResult, BinaryInfo, BinaryStatus, ChangeOutcome, CollectorHealth, CollectorState,
    HarnessHookStatus, HookHarness, HookPreview, HookState, HookStatusReport, PreviewKind,
    PreviewResult, RefusalReason,
};
use config_file::Existing;

/// Fingerprint of a config file that does not exist.
pub const ABSENT: &str = "absent";

/// `sha256:<hex>` of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn fingerprint(existing: &Existing) -> String {
    existing
        .bytes()
        .map_or_else(|| ABSENT.to_owned(), sha256_hex)
}

// ───────────────────────── targets ─────────────────────────

/// The harnesses we install into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    ClaudeCode,
    Codex,
}

impl Target {
    pub const ALL: [Target; 2] = [Target::ClaudeCode, Target::Codex];

    /// The value of `why-hook --harness`.
    pub fn hook_arg(self) -> &'static str {
        match self {
            Target::ClaudeCode => "claude-code",
            Target::Codex => "codex",
        }
    }

    pub fn events(self) -> &'static [(&'static str, u32)] {
        match self {
            Target::ClaudeCode => claude::EVENTS,
            Target::Codex => codex::EVENTS,
        }
    }

    pub fn contract(self) -> HookHarness {
        match self {
            Target::ClaudeCode => HookHarness::ClaudeCode,
            Target::Codex => HookHarness::Codex,
        }
    }
}

impl From<HookHarness> for Target {
    fn from(h: HookHarness) -> Self {
        match h {
            HookHarness::ClaudeCode => Target::ClaudeCode,
            HookHarness::Codex => Target::Codex,
        }
    }
}

// ───────────────────────── errors ─────────────────────────

#[derive(Debug, Error)]
pub enum HookError {
    #[error("no usable home directory: the agent's config directory cannot be located")]
    NoHome,
    #[error("{0}")]
    InvalidEnv(String),
    #[error("WHY will not edit this config: {0}")]
    ConfigInvalid(String),
    #[error("a managed policy forbids user-level hooks ({0})")]
    DisabledByPolicy(String),
    #[error("refusing to touch {0}")]
    UnsafeTarget(String),
    #[error("the config file is read-only; WHY will not override that")]
    ReadOnly,
    #[error("another process is editing this config directory; try again")]
    Locked,
    #[error("the config file is unexpectedly large")]
    TooLarge,
    #[error("no usable why-hook binary at {0} and no bundled copy to install")]
    HookBinaryMissing(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

impl HookError {
    /// Domain refusals the UI can render; `None` for plain I/O failures.
    pub fn refusal(&self) -> Option<(RefusalReason, String)> {
        let reason = match self {
            HookError::NoHome | HookError::InvalidEnv(_) => RefusalReason::NoHome,
            HookError::ConfigInvalid(_) => RefusalReason::ConfigInvalid,
            HookError::DisabledByPolicy(_) => RefusalReason::DisabledByPolicy,
            HookError::UnsafeTarget(_) => RefusalReason::UnsafeTarget,
            HookError::ReadOnly => RefusalReason::ReadOnly,
            HookError::Locked => RefusalReason::Locked,
            HookError::TooLarge => RefusalReason::TooLarge,
            HookError::HookBinaryMissing(_) => RefusalReason::HookBinaryMissing,
            HookError::Io(_) => return None,
        };
        Some((reason, self.to_string()))
    }
}

// ───────────────────────── environment ─────────────────────────

/// Everything the installer reads from the outside world, as plain values.
#[derive(Debug, Clone)]
pub struct HooksEnv {
    /// `HOME`, `XDG_DATA_HOME`, `XDG_RUNTIME_DIR`.
    pub paths: PathEnv,
    /// `CLAUDE_CONFIG_DIR`.
    pub claude_config_dir: Option<OsString>,
    /// `CODEX_HOME`.
    pub codex_home: Option<OsString>,
    /// Where a bundled `why-hook` may be copied from (next to the app binary).
    pub bundled_hook: Option<PathBuf>,
    /// Claude managed settings files and drop-in directories (read-only).
    pub claude_managed_files: Vec<PathBuf>,
    pub claude_managed_dropins: Vec<PathBuf>,
    /// Codex `requirements.toml` candidates (read-only).
    pub codex_requirements: Vec<PathBuf>,
    /// How long to wait for the config directory lock.
    pub lock_timeout: Duration,
}

impl HooksEnv {
    /// The real process environment.
    pub fn from_process() -> Self {
        let (files, dropins) = claude::default_managed();
        Self {
            paths: PathEnv::from_process(),
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR"),
            codex_home: std::env::var_os("CODEX_HOME"),
            bundled_hook: std::env::current_exe()
                .ok()
                .and_then(|exe| bundled_hook_path(&exe)),
            claude_managed_files: files,
            claude_managed_dropins: dropins,
            codex_requirements: codex::default_requirements(),
            lock_timeout: Duration::from_secs(2),
        }
    }

    /// An environment rooted in `home`, with NO managed policy and no env
    /// overrides: what tests start from.
    pub fn for_home(home: &Path, bundled_hook: Option<PathBuf>) -> Self {
        Self {
            paths: PathEnv {
                runtime_dir: None,
                data_home: None,
                home: Some(home.as_os_str().to_owned()),
            },
            claude_config_dir: None,
            codex_home: None,
            bundled_hook,
            claude_managed_files: Vec::new(),
            claude_managed_dropins: Vec::new(),
            codex_requirements: Vec::new(),
            lock_timeout: Duration::from_millis(200),
        }
    }

    /// `override_dir` if set (must be absolute), else `$HOME/<default_name>`.
    /// An empty override counts as unset. Paths must be UTF-8: they are written
    /// into JSON and a shell command.
    fn dir_from(
        &self,
        var: &str,
        override_dir: Option<&OsStr>,
        default_name: &str,
    ) -> Result<PathBuf, HookError> {
        let dir = match override_dir.filter(|v| !v.is_empty()) {
            Some(v) => {
                let p = Path::new(v);
                if !p.is_absolute() {
                    return Err(HookError::InvalidEnv(format!(
                        "{var} is not an absolute path; WHY will not guess what it is relative to"
                    )));
                }
                p.to_path_buf()
            }
            None => {
                let home = self
                    .paths
                    .home
                    .as_deref()
                    .map(Path::new)
                    .filter(|p| p.is_absolute())
                    .ok_or(HookError::NoHome)?;
                home.join(default_name)
            }
        };
        if dir.to_str().is_none() {
            return Err(HookError::InvalidEnv(format!(
                "{} is not valid UTF-8",
                dir.display()
            )));
        }
        Ok(dir)
    }

    fn config_dir(&self, t: Target) -> Result<PathBuf, HookError> {
        match t {
            Target::ClaudeCode => claude::config_dir(self),
            Target::Codex => codex::config_dir(self),
        }
    }

    fn config_file(&self, t: Target) -> Result<PathBuf, HookError> {
        match t {
            Target::ClaudeCode => claude::config_file(self),
            Target::Codex => codex::config_file(self),
        }
    }

    /// The managed file that forbids user-level hooks, if any.
    fn policy(&self, t: Target) -> Option<PathBuf> {
        match t {
            Target::ClaudeCode => {
                policy::claude_policy(&self.claude_managed_files, &self.claude_managed_dropins)
            }
            Target::Codex => policy::codex_policy(&self.codex_requirements),
        }
    }
}

/// What the running collector reports (taken by the IPC layer, so this module
/// stays testable without a collector).
#[derive(Debug, Clone)]
pub struct CollectorSnapshot {
    pub status: CollectorStatus,
    pub stats: StatsSnapshot,
}

// ───────────────────────── the installer ─────────────────────────

/// A change computed from the current file, not yet applied.
struct Plan {
    before: Option<String>,
    after_bytes: Option<Vec<u8>>,
    outcome: ChangeOutcome,
    notes: Vec<String>,
}

/// What an apply did, or why it did not.
pub enum Applied {
    Done(ChangeOutcome),
    /// The file changed since the preview: here is the fresh one.
    Stale(Box<HookPreview>),
}

/// Environment + this instance's paths: all the installer needs.
#[derive(Debug, Clone)]
pub struct HookContext {
    pub env: HooksEnv,
    pub paths: InstancePaths,
}

fn utf8(bytes: &[u8]) -> Result<&str, HookError> {
    std::str::from_utf8(bytes).map_err(|_| HookError::ConfigInvalid("the file is not UTF-8".into()))
}

fn invalid(e: String) -> HookError {
    HookError::ConfigInvalid(e)
}

impl HookContext {
    pub fn new(env: HooksEnv, paths: InstancePaths) -> Self {
        Self { env, paths }
    }

    fn instance(&self) -> String {
        self.paths.instance.to_string()
    }

    fn store(&self, t: Target, file: &Path) -> state::Store {
        state::Store::new(&self.paths.data_dir, self.paths.instance, t, file)
    }

    fn stable_path(&self) -> Result<PathBuf, HookError> {
        stable_hook_path(&self.env.paths, self.paths.instance).ok_or(HookError::NoHome)
    }

    /// The `command` WHY writes for `t`.
    fn command(&self, t: Target) -> Result<String, HookError> {
        let utf = |p: &Path| {
            p.to_str()
                .map(str::to_owned)
                .ok_or_else(|| HookError::InvalidEnv(format!("{} is not valid UTF-8", p.display())))
        };
        Ok(command::hook_command(
            &utf(&self.stable_path()?)?,
            t,
            &self.instance(),
            &utf(&self.paths.data_dir)?,
            &utf(&self.paths.socket_path)?,
        ))
    }

    fn is_own(&self, t: Target) -> impl Fn(&str) -> bool {
        let instance = self.instance();
        move |cmd| command::is_own_command(cmd, t, &instance)
    }

    pub fn binary_info(&self) -> Result<BinaryInfo, HookError> {
        let stable = self.stable_path()?;
        let bundled = self.env.bundled_hook.clone().filter(|p| p.is_file());
        let read = |p: &Path| fs::read(p).ok();
        let status = if !stable.is_file() {
            BinaryStatus::Missing
        } else if !is_executable(&stable) {
            BinaryStatus::Outdated
        } else {
            match bundled.as_deref().and_then(read) {
                None => BinaryStatus::Unverified,
                Some(b) if read(&stable).is_some_and(|s| s == b) => BinaryStatus::Current,
                Some(_) => BinaryStatus::Outdated,
            }
        };
        Ok(BinaryInfo {
            status,
            stable_path: stable.to_string_lossy().into_owned(),
            bundled_path: bundled.map(|p| p.to_string_lossy().into_owned()),
        })
    }

    /// Make the stable hook binary exist and match the bundled one.
    ///
    /// In a packaged app the bundle sits next to the executable. In a dev run
    /// `cargo build -p why-hook` puts the same file next to the dev binary, so
    /// the same lookup works; when it is absent we do NOT build or guess: we
    /// refuse with a clear "hook binary missing" (a config that points at a
    /// missing binary would make every agent event fail with exit 127).
    fn require_binary_source(&self) -> Result<(), HookError> {
        let info = self.binary_info()?;
        let needs_copy = matches!(info.status, BinaryStatus::Missing | BinaryStatus::Outdated);
        if needs_copy && info.bundled_path.is_none() {
            return Err(HookError::HookBinaryMissing(info.stable_path));
        }
        Ok(())
    }

    fn ensure_binary(&self) -> Result<(), HookError> {
        let info = self.binary_info()?;
        if matches!(
            info.status,
            BinaryStatus::Current | BinaryStatus::Unverified
        ) {
            return Ok(());
        }
        let Some(bundled) = self.env.bundled_hook.as_deref().filter(|p| p.is_file()) else {
            return Err(HookError::HookBinaryMissing(info.stable_path));
        };
        let stable = self.stable_path()?;
        if let Some(dir) = stable.parent() {
            config_file::reject_worktree(dir)?;
        }
        let installed = install_hook_binary_from(&self.env.paths, self.paths.instance, bundled)?;
        if !is_executable(&installed) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} cannot be executed", installed.display()),
            )
            .into());
        }
        Ok(())
    }

    fn check_policy(&self, t: Target) -> Result<(), HookError> {
        match self.env.policy(t) {
            Some(src) => Err(HookError::DisabledByPolicy(src.display().to_string())),
            None => Ok(()),
        }
    }

    // ── install ──

    fn plan_install(&self, t: Target, existing: &Existing) -> Result<Plan, HookError> {
        let command = self.command(t)?;
        let events = t.events();
        let mut notes = vec![format!(
            "Adds one hook per event ({}) that forwards the event to WHY; your own hooks are left alone.",
            events.len()
        )];
        if t == Target::Codex {
            notes.push(codex::REVIEW_HINT.to_owned());
        }
        let (before, after, outcome) = match existing {
            Existing::Absent => (
                None,
                json_edit::fresh(events, &command),
                ChangeOutcome::CreateFile,
            ),
            Existing::File { bytes, .. } => {
                let text = utf8(bytes)?;
                json_edit::validate(text).map_err(invalid)?;
                let after =
                    json_edit::install(text, events, &command, &self.is_own(t)).map_err(invalid)?;
                let outcome = if after == text {
                    ChangeOutcome::NoChange
                } else {
                    ChangeOutcome::ModifyFile
                };
                (Some(text.to_owned()), after, outcome)
            }
        };
        if after.len() as u64 > config_file::MAX_CONFIG_BYTES {
            return Err(HookError::TooLarge);
        }
        if outcome == ChangeOutcome::ModifyFile
            && let Ok(file) = self.env.config_file(t)
        {
            // The restore copy is a byte copy of the user's file, which can hold
            // secrets (Claude `env` values): say it, and where it is.
            let copy = self
                .store(t, &file)
                .original_copy_path(&sha256_hex(after.as_bytes()));
            notes.push(format!(
                "WHY keeps a private copy of the original file at {} (mode 0600) to restore it exactly; it is deleted on uninstall.",
                copy.display()
            ));
        }
        if t == Target::ClaudeCode && before.as_deref().is_some_and(user_disabled_hooks) {
            notes.push(USER_DISABLED_MESSAGE.to_owned());
        }
        Ok(Plan {
            before,
            after_bytes: Some(after.into_bytes()),
            outcome,
            notes,
        })
    }

    fn make_preview(
        &self,
        t: Target,
        kind: PreviewKind,
        file: &Path,
        existing: &Existing,
        plan: Plan,
    ) -> Result<HookPreview, HookError> {
        let after = plan
            .after_bytes
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned());
        let label = file
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let diff = if plan.outcome == ChangeOutcome::NoChange {
            String::new()
        } else {
            diff::unified_diff(&label, plan.before.as_deref(), after.as_deref())
        };
        Ok(HookPreview {
            harness: t.contract(),
            kind,
            config_path: file.to_string_lossy().into_owned(),
            fingerprint: fingerprint(existing),
            outcome: plan.outcome,
            before: plan.before,
            after,
            diff,
            binary: self.binary_info()?,
            notes: plan.notes,
        })
    }

    pub fn preview_install(&self, t: Target) -> Result<HookPreview, HookError> {
        self.check_policy(t)?;
        let file = self.env.config_file(t)?;
        let existing = config_file::read_target(&file)?;
        let plan = self.plan_install(t, &existing)?;
        self.make_preview(t, PreviewKind::Install, &file, &existing, plan)
    }

    pub fn install(&self, t: Target, approved: &str) -> Result<Applied, HookError> {
        self.check_policy(t)?;
        let dir = self.env.config_dir(t)?;
        let file = self.env.config_file(t)?;
        // Fail on a missing hook binary BEFORE creating any directory: a
        // refused install must leave no trace.
        self.require_binary_source()?;

        if !config_file::check_dir(&dir)? {
            // The user approved a preview of a file that did not exist.
            if approved != ABSENT {
                return self.stale(t, PreviewKind::Install, &file);
            }
            config_file::ensure_dir(&dir)?;
        }
        let _lock = config_file::lock_dir(&dir, self.env.lock_timeout)?;
        // Read AFTER the lock: the fingerprint must describe what we edit.
        let existing = config_file::read_target(&file)?;
        if fingerprint(&existing) != approved {
            return self.stale(t, PreviewKind::Install, &file);
        }
        let plan = self.plan_install(t, &existing)?;
        if plan.outcome != ChangeOutcome::NoChange {
            check_writable(&existing)?;
        }
        // After the plan (an invalid config must not cost a binary copy), before
        // the write (a config must never point at a missing binary).
        self.ensure_binary()?;

        let Some(after) = plan
            .after_bytes
            .as_deref()
            .filter(|_| plan.outcome != ChangeOutcome::NoChange)
        else {
            if config_file::read_target(&file)? != existing {
                return self.stale(t, PreviewKind::Install, &file);
            }
            return Ok(Applied::Done(ChangeOutcome::NoChange));
        };
        let store = self.store(t, &file);
        let remembered = self.remember_original(t, &store, &existing, approved);
        let written = sha256_hex(after);
        store.save(&written, remembered.as_deref())?;
        if !config_file::publish(&file, &existing, Some(after))? {
            store.discard(&written);
            return self.stale(t, PreviewKind::Install, &file);
        }
        store.clear_except(Some(&written));
        Ok(Applied::Done(plan.outcome))
    }

    /// The bytes uninstall should restore: the file before this instance's
    /// entries. `None` means the file did not exist.
    ///
    /// * untouched since our last write: the original we saved back then;
    /// * otherwise the current bytes with our (possibly stale) entries cut out,
    ///   so a later restore never resurrects an old WHY entry.
    fn remember_original(
        &self,
        t: Target,
        store: &state::Store,
        existing: &Existing,
        written: &str,
    ) -> Option<Vec<u8>> {
        let bytes = existing.bytes()?;
        match store.load(written) {
            Some(state::Original::Absent) => return None,
            Some(state::Original::File(bytes)) => return Some(bytes),
            None => {}
        }
        let own = self.is_own(t);
        Some(
            std::str::from_utf8(bytes)
                .ok()
                .and_then(|text| json_edit::remove_own(text, &own).ok())
                .map_or_else(|| bytes.to_vec(), String::into_bytes),
        )
    }

    // ── uninstall ──

    fn plan_uninstall(
        &self,
        t: Target,
        file: &Path,
        existing: &Existing,
    ) -> Result<Plan, HookError> {
        let Existing::File { bytes, .. } = existing else {
            return Ok(Plan {
                before: None,
                after_bytes: None,
                outcome: ChangeOutcome::NoChange,
                notes: vec!["The config file does not exist; nothing to remove.".into()],
            });
        };
        let text = utf8(bytes)?;
        match self.store(t, file).load(&sha256_hex(bytes)) {
            Some(state::Original::Absent) => {
                return Ok(Plan {
                    before: Some(text.to_owned()),
                    after_bytes: None,
                    outcome: ChangeOutcome::DeleteFile,
                    notes: vec![
                        "WHY created this file and it is unchanged since, so it is deleted.".into(),
                    ],
                });
            }
            Some(state::Original::File(original)) => {
                return Ok(Plan {
                    before: Some(text.to_owned()),
                    after_bytes: Some(original),
                    outcome: ChangeOutcome::RestoreOriginal,
                    notes: vec![
                        "The file is unchanged since WHY wrote it: your original bytes are restored exactly."
                            .into(),
                    ],
                });
            }
            None => {}
        }
        // The user edited the file since (or WHY has no record): only our
        // entries are cut out; everything else stays as the user left it.
        json_edit::validate(text).map_err(invalid)?;
        let after = json_edit::remove_own(text, &self.is_own(t)).map_err(invalid)?;
        let (outcome, note) = if after == text {
            (
                ChangeOutcome::NoChange,
                "No WHY entries for this instance were found.",
            )
        } else {
            (
                ChangeOutcome::RemoveEntries,
                "The file changed since WHY wrote it, so only WHY's entries are removed.",
            )
        };
        Ok(Plan {
            before: Some(text.to_owned()),
            after_bytes: Some(after.into_bytes()),
            outcome,
            notes: vec![note.to_owned()],
        })
    }

    pub fn preview_uninstall(&self, t: Target) -> Result<HookPreview, HookError> {
        let file = self.env.config_file(t)?;
        let existing = config_file::read_target(&file)?;
        let plan = self.plan_uninstall(t, &file, &existing)?;
        self.make_preview(t, PreviewKind::Uninstall, &file, &existing, plan)
    }

    pub fn uninstall(&self, t: Target, approved: &str) -> Result<Applied, HookError> {
        let dir = self.env.config_dir(t)?;
        let file = self.env.config_file(t)?;
        if !config_file::check_dir(&dir)? {
            if approved != ABSENT {
                return self.stale(t, PreviewKind::Uninstall, &file);
            }
            return Ok(Applied::Done(ChangeOutcome::NoChange));
        }
        let _lock = config_file::lock_dir(&dir, self.env.lock_timeout)?;
        let existing = config_file::read_target(&file)?;
        if fingerprint(&existing) != approved {
            return self.stale(t, PreviewKind::Uninstall, &file);
        }
        let plan = self.plan_uninstall(t, &file, &existing)?;
        if plan.outcome != ChangeOutcome::NoChange {
            check_writable(&existing)?;
            if !config_file::publish(&file, &existing, plan.after_bytes.as_deref())? {
                return self.stale(t, PreviewKind::Uninstall, &file);
            }
        }
        // The record described an install that no longer exists.
        self.store(t, &file).clear_except(None);
        Ok(Applied::Done(plan.outcome))
    }

    /// The approved fingerprint no longer matches: hand back a fresh preview.
    fn stale(&self, t: Target, kind: PreviewKind, file: &Path) -> Result<Applied, HookError> {
        let existing = config_file::read_target(file)?;
        let plan = match kind {
            PreviewKind::Install => self.plan_install(t, &existing)?,
            PreviewKind::Uninstall => self.plan_uninstall(t, file, &existing)?,
        };
        Ok(Applied::Stale(Box::new(
            self.make_preview(t, kind, file, &existing, plan)?,
        )))
    }

    // ── status ──

    /// Health of `t`'s hooks. Never fails: every problem is a state.
    pub fn status(&self, t: Target) -> HarnessHookStatus {
        let events = t.events().iter().map(|(e, _)| (*e).to_owned()).collect();
        let (config_path, state) = match self.env.config_file(t) {
            Err(e) => (
                String::new(),
                HookState::ConfigInvalid {
                    message: e.to_string(),
                },
            ),
            Ok(file) => (file.to_string_lossy().into_owned(), self.state_of(t, &file)),
        };
        HarnessHookStatus {
            harness: t.contract(),
            config_path,
            state,
            events,
        }
    }

    fn state_of(&self, t: Target, file: &Path) -> HookState {
        if let Some(src) = self.env.policy(t) {
            return HookState::DisabledByPolicy {
                source: src.display().to_string(),
            };
        }
        let existing = match config_file::read_target(file) {
            Ok(e) => e,
            Err(e) => {
                return HookState::ConfigInvalid {
                    message: e.to_string(),
                };
            }
        };
        let Existing::File { bytes, .. } = existing else {
            return HookState::NotInstalled;
        };
        let (Ok(text), command) = (utf8(&bytes), self.command(t)) else {
            return HookState::ConfigInvalid {
                message: "the file is not UTF-8".into(),
            };
        };
        let command = match command {
            Ok(c) => c,
            Err(e) => {
                return HookState::ConfigInvalid {
                    message: e.to_string(),
                };
            }
        };
        let own = self.is_own(t);
        let hits = match json_edit::own_handlers(text, &own) {
            Ok(h) => h,
            Err(e) => return HookState::ConfigInvalid { message: e },
        };
        if hits.is_empty() {
            // Not installed, but could we install? A shape we would have to
            // guess about (`"Stop": 5`) is reported now, not at install time.
            return match json_edit::install(text, t.events(), &command, &own) {
                Ok(_) => HookState::NotInstalled,
                Err(message) => HookState::ConfigInvalid { message },
            };
        }
        // Entries exist but the user's own settings switch every hook off: never
        // say "installed" while nothing can run.
        if t == Target::ClaudeCode && user_disabled_hooks(text) {
            return HookState::DisabledByUserSetting {
                message: USER_DISABLED_MESSAGE.to_owned(),
            };
        }
        let mut reasons = Vec::new();
        let events = t.events();
        let missing: Vec<&str> = events
            .iter()
            .map(|(e, _)| *e)
            .filter(|e| !hits.iter().any(|h| h.event == *e))
            .collect();
        if !missing.is_empty() {
            reasons.push(format!("missing events: {}", missing.join(", ")));
        }
        if hits
            .iter()
            .any(|h| !events.iter().any(|(e, _)| *e == h.event))
        {
            reasons.push("entries for events WHY no longer uses".into());
        }
        if hits.iter().any(|h| h.command != command) {
            reasons.push("the hook command (path or arguments) differs".into());
        }
        if hits.iter().any(|h| h.has_matcher) {
            reasons.push("a WHY group has a matcher".into());
        }
        if hits.iter().any(|h| {
            events
                .iter()
                .any(|(e, tm)| *e == h.event && h.timeout != Some(*tm))
        }) {
            reasons.push("a timeout differs".into());
        }
        // A duplicate or a shared group is not "wrong", but `install` would
        // tidy it: ask the editor whether it would change anything.
        if reasons.is_empty()
            && json_edit::install(text, events, &command, &own).is_ok_and(|after| after != text)
        {
            reasons.push("duplicate or shared entries".into());
        }
        match self.binary_info().map(|b| b.status) {
            Ok(BinaryStatus::Missing) => reasons.push("the why-hook binary is missing".into()),
            Ok(BinaryStatus::Outdated) => {
                reasons.push(
                    "the why-hook binary is not executable or differs from the bundled one".into(),
                );
            }
            Ok(_) => {}
            Err(e) => reasons.push(e.to_string()),
        }
        if !reasons.is_empty() {
            return HookState::Outdated { reasons };
        }
        match t {
            Target::ClaudeCode => HookState::Installed,
            Target::Codex => HookState::NeedsReview {
                hint: codex::REVIEW_HINT.to_owned(),
            },
        }
    }

    pub fn hook_status(&self, collector: Option<CollectorSnapshot>) -> HookStatusReport {
        let binary = self.binary_info().unwrap_or_else(|_| BinaryInfo {
            status: BinaryStatus::Missing,
            stable_path: String::new(),
            bundled_path: None,
        });
        HookStatusReport {
            claude_code: self.status(Target::ClaudeCode),
            codex: self.status(Target::Codex),
            binary,
            collector: self.collector_health(collector),
            instance: self.instance(),
        }
    }

    fn collector_health(&self, snapshot: Option<CollectorSnapshot>) -> CollectorHealth {
        let to_u32 = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
        let spool_files = fs::read_dir(&self.paths.spool_dir).ok().map(|d| {
            to_u32(
                d.flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                    .count() as u64,
            )
        });
        let socket_path = self.paths.socket_path.to_string_lossy().into_owned();
        let Some(snap) = snapshot else {
            return CollectorHealth {
                socket_path,
                status: CollectorState::NotRunning,
                events_received: None,
                spool_ingested: None,
                spool_files,
                unavailable: vec!["lastEventAt".into()],
            };
        };
        CollectorHealth {
            socket_path,
            status: match snap.status {
                CollectorStatus::Starting => CollectorState::Starting,
                CollectorStatus::Listening => CollectorState::Listening,
                CollectorStatus::Disabled(reason) => CollectorState::Disabled { reason },
            },
            events_received: Some(to_u32(snap.stats.received)),
            spool_ingested: Some(to_u32(snap.stats.spool_ingested)),
            spool_files,
            // The collector keeps counters but no timestamp of the last event.
            unavailable: vec!["lastEventAt".into()],
        }
    }
}

const USER_DISABLED_MESSAGE: &str = "Your own Claude settings set `disableAllHooks` to true, so no hook runs, WHY's included. WHY does not change that; set it to false or remove it to receive events.";

/// Claude's `disableAllHooks: true` in the user's own settings (top level).
fn user_disabled_hooks(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .is_some_and(|v| v.get("disableAllHooks").and_then(|x| x.as_bool()) == Some(true))
}

/// WHY respects a read-only file as the user's "do not change this".
fn check_writable(existing: &Existing) -> Result<(), HookError> {
    match existing {
        Existing::File { mode, .. } if mode & 0o200 == 0 => Err(HookError::ReadOnly),
        _ => Ok(()),
    }
}

// ───────────────────────── contract conversions ─────────────────────────

impl HookContext {
    pub fn preview_install_result(&self, t: Target) -> Result<PreviewResult, HookError> {
        preview_result(self.preview_install(t))
    }

    pub fn preview_uninstall_result(&self, t: Target) -> Result<PreviewResult, HookError> {
        preview_result(self.preview_uninstall(t))
    }

    pub fn install_result(&self, t: Target, fingerprint: &str) -> Result<ApplyResult, HookError> {
        self.apply_result(t, self.install(t, fingerprint))
    }

    pub fn uninstall_result(&self, t: Target, fingerprint: &str) -> Result<ApplyResult, HookError> {
        self.apply_result(t, self.uninstall(t, fingerprint))
    }

    fn apply_result(
        &self,
        t: Target,
        applied: Result<Applied, HookError>,
    ) -> Result<ApplyResult, HookError> {
        match applied {
            Ok(Applied::Done(outcome)) => Ok(ApplyResult::Applied {
                outcome,
                status: self.status(t),
            }),
            Ok(Applied::Stale(preview)) => Ok(ApplyResult::Stale { preview: *preview }),
            Err(e) => match e.refusal() {
                Some((reason, message)) => Ok(ApplyResult::Refused { reason, message }),
                None => Err(e),
            },
        }
    }
}

fn preview_result(r: Result<HookPreview, HookError>) -> Result<PreviewResult, HookError> {
    match r {
        Ok(preview) => Ok(PreviewResult::Ready { preview }),
        Err(e) => match e.refusal() {
            Some((reason, message)) => Ok(PreviewResult::Refused { reason, message }),
            None => Err(e),
        },
    }
}
