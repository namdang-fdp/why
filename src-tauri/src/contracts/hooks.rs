//! Contracts for the hook installer (WHY-51): which agent configs WHY writes
//! its hooks into, what a change would look like, and whether the hooks are
//! healthy. The UI (WHY-18) renders these directly: every state is a tagged
//! variant, never a pile of booleans.

use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

/// The agents WHY can install hooks for. (Not [`Harness`](super::Harness): that
/// one also has `Unknown` for events from agents this build cannot talk to.)
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum HookHarness {
    ClaudeCode,
    Codex,
}

/// Health of one harness's hook entries.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HookState {
    /// No WHY entry for this instance in the user's config.
    NotInstalled,
    /// Our entries are there and match what this build would write.
    Installed,
    /// Our entries are there but differ from what this build would write
    /// (events, command path or arguments, timeout, or the hook binary).
    Outdated { reasons: Vec<String> },
    /// Entries match, but the harness must also trust them before they run and
    /// WHY cannot tell whether that happened (Codex keeps the decision in
    /// `config.toml`, which WHY does not read). `hint` says what to do.
    NeedsReview { hint: String },
    /// The user's OWN settings turn hooks off (Claude `disableAllHooks: true`),
    /// so WHY's entries are installed but nothing runs. WHY does not change the
    /// user's setting; `message` says what to do.
    DisabledByUserSetting { message: String },
    /// A managed (administrator) setting forbids user-level hooks. WHY does
    /// not install and does not work around it.
    DisabledByPolicy { source: String },
    /// The config file is not something WHY will edit (invalid JSON, wrong
    /// shape, unsafe target). Nothing was changed.
    ConfigInvalid { message: String },
}

/// The shared `why-hook` copy in `~/.local/share/why/bin`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum BinaryStatus {
    /// Not at the stable path (and an install has nothing to copy it from
    /// when no bundled copy is available).
    Missing,
    /// Identical to the copy bundled with this build.
    Current,
    /// Present but differs from the bundled copy: an install refreshes it.
    Outdated,
    /// Present, but there is no bundled copy here to compare with (typical of
    /// a dev run before `cargo build -p why-hook`).
    Unverified,
}

/// Whether the hook binary can be installed by this build.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct BinaryInfo {
    pub status: BinaryStatus,
    /// Where hook configs point.
    pub stable_path: String,
    /// The copy bundled next to the app executable, if there is one.
    pub bundled_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct HarnessHookStatus {
    pub harness: HookHarness,
    /// The user-level config file WHY would edit.
    pub config_path: String,
    pub state: HookState,
    /// The events WHY registers for this harness.
    pub events: Vec<String>,
}

/// What the running collector can tell us. Fields the collector does not
/// expose are named in `unavailable` instead of being guessed.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct CollectorHealth {
    pub socket_path: String,
    pub status: CollectorState,
    /// Envelopes decoded and queued, since the app started (saturates at u32).
    pub events_received: Option<u32>,
    /// Envelopes read back from the spool since the app started.
    pub spool_ingested: Option<u32>,
    /// Files waiting in the spool directory (events hooks could not deliver).
    pub spool_files: Option<u32>,
    /// Health facts the collector does not expose (so the UI shows "unknown",
    /// not zero): currently `lastEventAt`.
    pub unavailable: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CollectorState {
    Starting,
    Listening,
    /// Not receiving; `reason` is the collector's own explanation.
    Disabled {
        reason: String,
    },
    /// No collector is running in this process (its paths could not be resolved).
    NotRunning,
}

/// Result of `hook_status`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct HookStatusReport {
    pub claude_code: HarnessHookStatus,
    pub codex: HarnessHookStatus,
    pub binary: BinaryInfo,
    pub collector: CollectorHealth,
    /// `installed` or `dev:<port>`: which instance's entries this report is about.
    pub instance: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum PreviewKind {
    Install,
    Uninstall,
}

/// What applying the previewed change does to the config file.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum ChangeOutcome {
    /// Nothing to do (already installed / nothing of ours to remove).
    NoChange,
    /// The file does not exist yet; it will be created.
    CreateFile,
    /// WHY's entries are added or updated; everything else stays.
    ModifyFile,
    /// Uninstall: the file is untouched since WHY wrote it, so the original
    /// bytes are put back exactly.
    RestoreOriginal,
    /// Uninstall: WHY created the file and it is untouched since, so it is deleted.
    DeleteFile,
    /// Uninstall: the file changed since WHY wrote it, so only WHY's entries
    /// are cut out and the rest stays as the user left it.
    RemoveEntries,
}

/// The exact change an install or uninstall would make, computed without
/// writing anything. `fingerprint` is what the user approves: pass it back to
/// `install` / `uninstall`, which refuse if the file changed since.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct HookPreview {
    pub harness: HookHarness,
    pub kind: PreviewKind,
    pub config_path: String,
    /// `sha256:<hex>` of the bytes read, or `absent` when the file does not exist.
    pub fingerprint: String,
    pub outcome: ChangeOutcome,
    /// The file as it is now (`null`: it does not exist).
    pub before: Option<String>,
    /// The file as it would be (`null`: it would not exist).
    pub after: Option<String>,
    /// Unified diff of `before` to `after`; empty when nothing changes.
    pub diff: String,
    pub binary: BinaryInfo,
    /// Plain-language facts the user should see (Codex trust, shared file, ...).
    pub notes: Vec<String>,
}

/// Why WHY declined to do something. Nothing was changed in any of these.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum RefusalReason {
    ConfigInvalid,
    DisabledByPolicy,
    UnsafeTarget,
    ReadOnly,
    /// Another process holds the lock on the config directory.
    Locked,
    HookBinaryMissing,
    /// No usable home / config directory.
    NoHome,
    TooLarge,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum PreviewResult {
    Ready {
        preview: HookPreview,
    },
    Refused {
        reason: RefusalReason,
        message: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum ApplyResult {
    Applied {
        outcome: ChangeOutcome,
        status: HarnessHookStatus,
    },
    /// The file changed after the preview the user approved; nothing was
    /// written. `preview` is the fresh one to show.
    Stale { preview: HookPreview },
    Refused {
        reason: RefusalReason,
        message: String,
    },
}
