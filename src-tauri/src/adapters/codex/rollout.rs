//! Lazy, read-only reader for Codex "rollout" files (`transcript_path`).
//!
//! Hooks cannot say two things WHY wants: **how a shell command ended** (no
//! exit code in the hook, no failure event) and **what the agent said around a
//! call**. The rollout JSONL has both. This module reads them on demand; nothing
//! in the ingest path calls it yet (a later task upgrades `unknown` shell
//! outcomes with it).
//!
//! # Where the evidence comes from
//!
//! The rollout wraps tool calls in a JavaScript `exec` call, which would mean
//! parsing JS. We do not: Codex also writes a *structured* `event_msg` record
//! per finished item, and its `item.id` equals the hook's `tool_use_id`
//! (verified on every fixture: `exec-<uuid>`):
//!
//! ```text
//! {"type":"event_msg","payload":{"type":"item_completed","turn_id":"…",
//!   "item":{"type":"CommandExecution","id":"exec-…","exit_code":3,…}}}
//! {"type":"event_msg","payload":{"type":"item_completed","turn_id":"…",
//!   "item":{"type":"AgentMessage","content":[{"type":"Text","text":"…"}]}}}
//! ```
//!
//! If the matching record is missing or ambiguous the answer is `None`, never
//! a guess.
//!
//! # Safety and privacy
//!
//! * The path must resolve to a **regular file inside the sessions root**
//!   (`$CODEX_HOME/sessions` or `~/.codex/sessions`). Linux `openat2` enforces
//!   that boundary relative to an open directory handle. `O_NONBLOCK` keeps
//!   a FIFO from hanging the open. Reading requires kernel support for `openat2`.
//! * Reads are bounded: at most [`MAX_SCAN_BYTES`] of the file (its tail when
//!   it is larger), one byte at the tail boundary, and a small head for
//!   `session_meta`. Lines longer than [`MAX_LINE_BYTES`] are skipped.
//! * Only three things ever leave this module: `cli_version`, agent message
//!   text and an exit code. `base_instructions`, developer messages, the
//!   AGENTS.md message, `world_state` and `rate_limits` (owner-specific or
//!   private) are parsed past and dropped: their fields are never read.
//! * Encrypted content stays opaque: a message that is an encrypted blob is
//!   returned as `(encrypted)`.
//! * `auth.json` and other credential files are never opened: the root check
//!   confines this reader to `sessions/`.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{ENCRYPTED, looks_encrypted};
use crate::events::excerpt;

/// At most this much of a rollout is scanned (its tail if the file is bigger):
/// a long session's rollout can reach hundreds of MiB.
pub const MAX_SCAN_BYTES: u64 = 16 * 1024 * 1024;
/// Where to look for `session_meta` (the first record) in a file too big to
/// scan whole.
const HEAD_BYTES: u64 = 2 * 1024 * 1024;
/// A line longer than this is skipped unparsed (a pasted-in file, a huge tool
/// result): bounded memory per line.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// Messages kept in [`RolloutContext::turn_messages`].
const MAX_TURN_MESSAGES: usize = 32;
/// The Codex CLI version this reader was written against. Other versions are
/// still read, but exit codes are withheld (the record shape is not verified).
pub const TESTED_CLI_PREFIX: &str = "0.170.";

#[derive(Debug, thiserror::Error)]
pub enum RolloutError {
    /// The path is not inside the sessions root (or the root does not exist).
    #[error("rollout path is outside the Codex sessions directory")]
    OutsideRoot,
    /// A directory, FIFO, socket, device or dangling symlink.
    #[error("rollout path is not a regular file")]
    NotRegularFile,
    #[error("rollout read failed: {0}")]
    Io(#[from] io::Error),
}

/// What the rollout says about one turn / tool call. Every field is `None` or
/// empty when the rollout has nothing clear to say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RolloutContext {
    /// `session_meta.cli_version`.
    pub cli_version: Option<String>,
    /// The version is one the record shapes were verified against.
    pub version_trusted: bool,
    /// The agent's last message before the tool call, in the same turn
    /// (*Declared*: its own words).
    pub narration_before: Option<String>,
    /// Its first message after the call.
    pub narration_after: Option<String>,
    /// The command's exit code from the structured `CommandExecution` record
    /// whose id equals `tool_use_id`. Only for a trusted version.
    pub exit_code: Option<i32>,
    /// All the turn's agent messages (capped), oldest first.
    pub turn_messages: Vec<String>,
    /// Upper bound on the bytes read, for tests and diagnostics.
    pub bytes_scanned: u64,
    /// The file was bigger than the scan window.
    pub truncated: bool,
}

/// `$CODEX_HOME/sessions`, else `~/.codex/sessions`. `None` when neither
/// variable is set.
pub fn sessions_root() -> Option<PathBuf> {
    sessions_root_from(
        std::env::var_os("CODEX_HOME"),
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
    )
}

/// The pure part of [`sessions_root`] (tests pass values instead of touching
/// the process environment).
pub fn sessions_root_from(codex_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    match codex_home.filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir).join("sessions")),
        None => home
            .filter(|v| !v.is_empty())
            .map(|h| PathBuf::from(h).join(".codex").join("sessions")),
    }
}

/// Open `path` for reading, after proving it is a regular file inside `root`.
#[cfg(target_os = "linux")]
fn open_confined(path: &Path, root: &Path) -> Result<File, RolloutError> {
    use std::os::unix::fs::OpenOptionsExt;

    let sessions = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(|_| RolloutError::OutsideRoot)?;

    // Resolve aliases and `..` to obtain a candidate relative path. Only the
    // open beneath the directory handle decides what file can be read.
    let canonical_root = root.canonicalize().map_err(|_| RolloutError::OutsideRoot)?;
    let real = path.canonicalize().map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => RolloutError::Io(e),
        _ => RolloutError::NotRegularFile,
    })?;
    let relative = real
        .strip_prefix(canonical_root)
        .map_err(|_| RolloutError::OutsideRoot)?;
    if relative.as_os_str().is_empty() {
        return Err(RolloutError::NotRegularFile);
    }
    let file = open_beneath(&sessions, relative).map_err(|e| match e.raw_os_error() {
        Some(libc::ELOOP | libc::EXDEV) => RolloutError::OutsideRoot,
        _ => RolloutError::Io(e),
    })?;
    if !file.metadata()?.is_file() {
        return Err(RolloutError::NotRegularFile);
    }
    Ok(file)
}

#[cfg(target_os = "linux")]
fn open_beneath(root: &File, relative: &Path) -> io::Result<File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let relative = CString::new(relative.as_os_str().as_bytes())?;
    // SAFETY: `open_how` contains integer fields, all of which accept zero.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) as u64;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS;
    // SAFETY: the path and open_how pointers remain valid through the call,
    // and `root` keeps the directory descriptor open.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root.as_raw_fd(),
            relative.as_ptr(),
            &how as *const libc::open_how,
            std::mem::size_of_val(&how),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat2 returned a new file descriptor owned by this call.
    Ok(unsafe { File::from_raw_fd(fd as std::os::fd::RawFd) })
}

#[cfg(not(target_os = "linux"))]
fn open_confined(_path: &Path, _root: &Path) -> Result<File, RolloutError> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "confined rollout open requires Linux openat2",
    )
    .into())
}

enum Line {
    Eof,
    Ok,
    TooLong,
}

/// Read one line into `buf` without ever holding more than `max` bytes of it.
fn next_line<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>, max: usize) -> io::Result<Line> {
    buf.clear();
    let n = reader
        .by_ref()
        .take(max as u64 + 1)
        .read_until(b'\n', buf)?;
    if n == 0 {
        return Ok(Line::Eof);
    }
    if buf.len() > max && buf.last() != Some(&b'\n') {
        // Over the limit: drop the rest of this line in small chunks.
        loop {
            buf.clear();
            let n = reader.by_ref().take(64 * 1024).read_until(b'\n', buf)?;
            if n == 0 || buf.last() == Some(&b'\n') {
                break;
            }
        }
        buf.clear();
        return Ok(Line::TooLong);
    }
    Ok(Line::Ok)
}

/// What one scan looks for.
struct Scanner<'a> {
    turn_id: &'a str,
    tool_use_id: Option<&'a str>,
    cli_version: Option<String>,
    /// `(sequence number, text)` of the turn's agent messages.
    messages: Vec<(u64, String)>,
    /// Sequence number of the anchor item (the tool call), if seen.
    anchor: Option<u64>,
    exit_codes: Vec<i32>,
    seq: u64,
}

impl Scanner<'_> {
    fn feed(&mut self, record: &Value) {
        self.seq += 1;
        match record.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                // Only the version. Everything else in this record (base
                // instructions, account ids) is deliberately not looked at.
                if self.cli_version.is_none() {
                    self.cli_version = record
                        .pointer("/payload/cli_version")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
            }
            Some("event_msg") => self.feed_event(record.get("payload")),
            // response_item, turn_context, world_state, token_usage_record and
            // anything newer: not needed (and several hold private text).
            _ => {}
        }
    }

    fn feed_event(&mut self, payload: Option<&Value>) {
        let Some(payload) = payload else { return };
        if payload.get("type").and_then(Value::as_str) != Some("item_completed") {
            return;
        }
        if payload.get("turn_id").and_then(Value::as_str) != Some(self.turn_id) {
            return;
        }
        let Some(item) = payload.get("item") else {
            return;
        };
        let is_anchor = self.tool_use_id.is_some()
            && item.get("id").and_then(Value::as_str) == self.tool_use_id;
        if is_anchor {
            self.anchor.get_or_insert(self.seq);
            if item.get("type").and_then(Value::as_str) == Some("CommandExecution")
                && let Some(code) = item
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .and_then(|c| i32::try_from(c).ok())
            {
                self.exit_codes.push(code);
            }
        }
        if item.get("type").and_then(Value::as_str) == Some("AgentMessage")
            && let Some(text) = agent_message_text(item)
        {
            self.messages.push((self.seq, text));
        }
    }
}

/// The text of an `AgentMessage` item: its `Text` content parts, joined.
/// Encrypted blobs become `(encrypted)`; other part types are ignored.
fn agent_message_text(item: &Value) -> Option<String> {
    let parts = item.get("content")?.as_array()?;
    let text: Vec<&str> = parts
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("Text"))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    if text.is_empty() {
        return None;
    }
    let joined = text.join("\n");
    if looks_encrypted(joined.trim()) {
        return Some(ENCRYPTED.to_owned());
    }
    Some(excerpt(&joined))
}

/// Feed every parseable line of `reader` to `scanner`. Truncated, oversized,
/// non-UTF-8 and non-JSON lines are skipped, never fatal.
fn scan<R: BufRead>(
    reader: &mut R,
    scanner: &mut Scanner<'_>,
    stop_at_meta: bool,
) -> io::Result<()> {
    let mut buf = Vec::new();
    loop {
        match next_line(reader, &mut buf, MAX_LINE_BYTES)? {
            Line::Eof => return Ok(()),
            Line::TooLong => continue,
            Line::Ok => {}
        }
        let Ok(record) = serde_json::from_slice::<Value>(&buf) else {
            continue;
        };
        scanner.feed(&record);
        if stop_at_meta && scanner.cli_version.is_some() {
            return Ok(());
        }
    }
}

/// Read what the rollout at `path` says about `turn_id` and, optionally, the
/// tool call `tool_use_id` (the hook's id).
///
/// `root` is the sessions directory the file must live under
/// ([`sessions_root`]). A rollout this reader does not understand (another
/// Codex version, unknown record types) is not an error: the result is simply
/// empty where nothing could be established.
pub fn read_context(
    path: &Path,
    root: &Path,
    turn_id: &str,
    tool_use_id: Option<&str>,
) -> Result<RolloutContext, RolloutError> {
    let mut file = open_confined(path, root)?;
    let len = file.metadata()?.len();
    let mut scanner = Scanner {
        turn_id,
        tool_use_id,
        cli_version: None,
        messages: Vec::new(),
        anchor: None,
        exit_codes: Vec::new(),
        seq: 0,
    };

    let mut bytes_scanned = 0;
    let truncated = len > MAX_SCAN_BYTES;
    if truncated {
        // Big file: the version from the head, the evidence from the tail
        // (the recent turns, the ones a live session is asking about).
        let mut head = BufReader::new((&file).take(HEAD_BYTES));
        scan(&mut head, &mut scanner, true)?;
        bytes_scanned += HEAD_BYTES.min(len);
        file.seek(SeekFrom::Start(len - MAX_SCAN_BYTES - 1))?;
        let mut previous = [0];
        file.read_exact(&mut previous)?;
        let mut tail = BufReader::new((&file).take(MAX_SCAN_BYTES));
        if previous[0] != b'\n' {
            tail.skip_until(b'\n')?;
        }
        scan(&mut tail, &mut scanner, false)?;
        bytes_scanned += MAX_SCAN_BYTES + 1;
    } else {
        let mut all = BufReader::new((&file).take(MAX_SCAN_BYTES));
        scan(&mut all, &mut scanner, false)?;
        bytes_scanned += len;
    }

    let version_trusted = scanner
        .cli_version
        .as_deref()
        .is_some_and(|v| v.starts_with(TESTED_CLI_PREFIX));
    // One consistent exit code or nothing: two different codes for one id means
    // the rollout is not what we think it is.
    let exit_code = match scanner.exit_codes.as_slice() {
        [first, rest @ ..] if rest.iter().all(|c| c == first) && version_trusted => Some(*first),
        _ => None,
    };
    let (narration_before, narration_after) = match scanner.anchor {
        Some(anchor) => (
            scanner
                .messages
                .iter()
                .rev()
                .find(|(seq, _)| *seq < anchor)
                .map(|(_, t)| t.clone()),
            scanner
                .messages
                .iter()
                .find(|(seq, _)| *seq > anchor)
                .map(|(_, t)| t.clone()),
        ),
        None => (None, None),
    };
    let turn_messages = scanner
        .messages
        .into_iter()
        .take(MAX_TURN_MESSAGES)
        .map(|(_, t)| t)
        .collect();

    Ok(RolloutContext {
        cli_version: scanner.cli_version,
        version_trusted,
        narration_before,
        narration_after,
        exit_code,
        turn_messages,
        bytes_scanned,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn confined_open_stays_with_the_directory_handle() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        std::fs::create_dir(&root).unwrap();
        let relative = Path::new("2026/rollout.jsonl");
        std::fs::create_dir(root.join("2026")).unwrap();
        std::fs::write(root.join(relative), "original").unwrap();
        let handle = File::open(&root).unwrap();

        std::fs::rename(&root, dir.path().join("old-sessions")).unwrap();
        std::fs::create_dir_all(root.join("2026")).unwrap();
        std::fs::write(root.join(relative), "replacement").unwrap();

        let mut text = String::new();
        open_beneath(&handle, relative)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "original");

        // Every component of the candidate is checked, including directories.
        std::os::unix::fs::symlink("2026", dir.path().join("old-sessions/alias")).unwrap();
        let err = open_beneath(&handle, Path::new("alias/rollout.jsonl")).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ELOOP));
    }

    #[test]
    fn root_prefers_codex_home_and_never_guesses() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(
            sessions_root_from(os("/c"), os("/h")),
            Some(PathBuf::from("/c/sessions"))
        );
        assert_eq!(
            sessions_root_from(None, os("/h")),
            Some(PathBuf::from("/h/.codex/sessions"))
        );
        assert_eq!(
            sessions_root_from(os(""), os("/h")),
            Some(PathBuf::from("/h/.codex/sessions"))
        );
        assert_eq!(sessions_root_from(None, None), None);
    }
}
