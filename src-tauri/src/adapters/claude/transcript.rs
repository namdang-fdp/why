//! Lazy, read-only reader for Claude Code transcripts (Declared rationale).
//!
//! Hooks give WHY what happened; the *words around it* ("I'll fix the off-by-one
//! first") live only in the transcript JSONL at `transcript_path`. This module
//! reads those words on demand. It is **not** called during ingest (WHY-16/17
//! decide when).
//!
//! The transcript is an unstable, private interface of Claude Code (payloads.md
//! §2): record types come and go, so unknown records are skipped, never an
//! error, and the `version` field is reported so a caller can degrade. Thinking
//! blocks are empty in this version and are not read at all.
//!
//! Safety, because a path comes from a hook payload (another program):
//! - Linux 5.6+ opens the file relative to the allowed root directory handle
//!   (`~/.claude/projects` or `$CLAUDE_CONFIG_DIR/projects`) without following
//!   symlinks beneath it;
//! - `O_NONBLOCK` avoids waiting on a FIFO, and the open descriptor is checked
//!   to be a regular file before reading;
//! - reads are bounded in total bytes and per line, so a 50 MB file (or one with
//!   no newline at all) cannot make a call expensive.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::events::excerpt;

/// Transcripts whose `version` starts with this were checked (payloads.md).
pub const SUPPORTED_MAJOR: &str = "2.";

const MAX_METADATA_BYTES: usize = 128;
const MAX_NARRATION_BLOCKS: usize = 1024;

/// Per-call read bounds.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Most bytes read from the file in one call.
    pub max_bytes: u64,
    /// A line longer than this is skipped unparsed (tool results can be huge).
    pub max_line_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 8 * 1024 * 1024,
            max_line_bytes: 1024 * 1024,
        }
    }
}

/// What to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query<'a> {
    /// The assistant text written just before the tool call `tool_use_id`
    /// (the narration of the same assistant step, back to the last user or
    /// tool-result record).
    BeforeToolUse(&'a str),
    /// All assistant text of one user turn (`prompt_id`): from the user record
    /// with that `promptId` to the next user record with another one.
    Turn(&'a str),
}

/// One assistant `text` block, capped with [`excerpt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextBlock {
    pub uuid: Option<String>,
    pub timestamp: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Context {
    /// Text found on this page. Append pages of a turn in scan order.
    pub texts: Vec<TextBlock>,
    /// The query's anchor (tool_use / user prompt record) has been seen.
    pub found: bool,
    /// First valid `version` seen so far in the scan.
    pub version: Option<String>,
    /// `version` is a major WHY has checked. `false` also when none was seen;
    /// the texts are still returned, the caller decides how much to trust them.
    pub version_supported: bool,
    /// Byte position reached by the scan. This can be inside a record.
    pub next_offset: u64,
    /// The byte budget ended with more data available. Call
    /// [`TranscriptScan::read_page`] again on the same scan to continue.
    pub truncated: bool,
    /// A tool's narration exceeded the retained block limit. Only the most
    /// recent blocks before the tool call are returned.
    pub narration_truncated: bool,
    /// Complete lines skipped on this page: invalid JSON or over the line cap.
    pub skipped_lines: u32,
}

#[derive(Debug, Error)]
pub enum TranscriptError {
    #[error("transcript not found")]
    NotFound,
    #[error("transcript is outside the allowed directory")]
    OutsideAllowedRoot,
    #[error("transcript is not a regular file")]
    NotRegularFile,
    #[error("transcript path contains a symlink beneath the allowed directory")]
    Symlink,
    #[error("safe transcript lookup is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("transcript read failed: {0}")]
    Io(io::Error),
}

impl From<io::Error> for TranscriptError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::NotFound {
            TranscriptError::NotFound
        } else {
            TranscriptError::Io(e)
        }
    }
}

/// The directory transcripts may be read from, given the (already read)
/// `CLAUDE_CONFIG_DIR` value and home directory. Pure, so tests need no
/// process-wide environment. A relative or empty config dir is ignored.
pub fn projects_root(config_dir: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    match config_dir.map(Path::new) {
        Some(dir) if dir.is_absolute() => Some(dir.join("projects")),
        _ => home
            .filter(|h| h.is_absolute())
            .map(|h| h.join(".claude").join("projects")),
    }
}

/// [`projects_root`] for the running process (reads the environment).
pub fn default_projects_root() -> Option<PathBuf> {
    let config = std::env::var("CLAUDE_CONFIG_DIR").ok();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    projects_root(config.as_deref(), home.as_deref())
}

/// Open `path` for reading if it is a regular file inside `allowed_root`.
#[cfg(target_os = "linux")]
fn open_checked(allowed_root: &Path, path: &Path) -> Result<File, TranscriptError> {
    use std::ffi::CString;
    use std::fs::OpenOptions;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
    use std::path::Component;

    let relative = path
        .strip_prefix(allowed_root)
        .map_err(|_| TranscriptError::OutsideAllowedRoot)?;
    if relative.as_os_str().is_empty() {
        return Err(TranscriptError::NotRegularFile);
    }
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(TranscriptError::OutsideAllowedRoot);
    }
    let relative = CString::new(relative.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_PATH)
        .open(allowed_root)?;

    // SAFETY: open_how consists of unsigned integers, and the kernel requires
    // unused fields (including mode for a read-only open) to be zeroed.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY) as u64;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS;
    // SAFETY: root stays open and the path and open_how pointers are valid
    // for the call. The kernel enforces containment in the same lookup.
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
        let error = io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(libc::EXDEV) => TranscriptError::OutsideAllowedRoot,
            Some(libc::ELOOP) => TranscriptError::Symlink,
            Some(libc::ENOSYS) => TranscriptError::UnsupportedPlatform,
            _ => error.into(),
        });
    }
    // SAFETY: a successful openat2 returns a new fd that we own.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) });

    // Check the opened descriptor so it is the same file we will read.
    if !file.metadata()?.is_file() {
        return Err(TranscriptError::NotRegularFile);
    }
    Ok(file)
}

#[cfg(not(target_os = "linux"))]
fn open_checked(_allowed_root: &Path, _path: &Path) -> Result<File, TranscriptError> {
    Err(TranscriptError::UnsupportedPlatform)
}

/// Read one page of assistant text matching `query`, starting at a record
/// boundary (`0` to start at the beginning). Use [`TranscriptScan`] for reads
/// that can span pages or follow a growing file.
pub fn read_context(
    allowed_root: &Path,
    path: &Path,
    query: Query<'_>,
    start_offset: u64,
    limits: Limits,
) -> Result<Context, TranscriptError> {
    TranscriptScan::open(allowed_root, path, query, start_offset, limits)?.read_page()
}

/// A bounded scan of one append-only transcript for one query. The same file
/// descriptor, incomplete record and matcher state are kept between pages.
pub struct TranscriptScan<'a> {
    file: File,
    limits: Limits,
    matcher: Matcher<'a>,
    version: Option<String>,
    offset: u64,
    line: Vec<u8>,
    oversize: bool,
    done: bool,
}

impl<'a> TranscriptScan<'a> {
    /// Open a scan at a record boundary (`0` to start at the beginning).
    pub fn open(
        allowed_root: &Path,
        path: &Path,
        query: Query<'a>,
        start_offset: u64,
        limits: Limits,
    ) -> Result<Self, TranscriptError> {
        let mut file = open_checked(allowed_root, path)?;
        file.seek(SeekFrom::Start(start_offset))?;
        Ok(Self {
            file,
            limits,
            matcher: Matcher::new(query),
            version: None,
            offset: start_offset,
            line: Vec::new(),
            oversize: false,
            done: false,
        })
    }

    /// Read up to one page. An unfinished record is only parsed once its newline
    /// arrives, even if it takes several pages or the writer is still appending.
    pub fn read_page(&mut self) -> Result<Context, TranscriptError> {
        let mut ctx = Context::default();
        let mut budget = self.limits.max_bytes;
        let mut buf = vec![0u8; 64 * 1024];
        'read: while !self.done && budget > 0 {
            let want = buf.len().min(usize::try_from(budget).unwrap_or(usize::MAX));
            let n = self.file.read(&mut buf[..want])?;
            if n == 0 {
                break;
            }
            budget -= n as u64;
            for &byte in &buf[..n] {
                self.offset += 1;
                if byte == b'\n' {
                    self.finish_line(&mut ctx);
                    if self.done {
                        break 'read;
                    }
                } else if !self.oversize {
                    if self.line.len() == self.limits.max_line_bytes {
                        // Skip to the next newline, even across page boundaries.
                        self.oversize = true;
                        self.line.clear();
                    } else {
                        self.line.push(byte);
                    }
                }
            }
        }

        ctx.next_offset = self.offset;
        ctx.truncated = !self.done && budget == 0 && self.offset < self.file.metadata()?.len();
        ctx.found = self.matcher.found;
        ctx.version.clone_from(&self.version);
        ctx.version_supported = self
            .version
            .as_deref()
            .is_some_and(|v| v.starts_with(SUPPORTED_MAJOR));
        Ok(ctx)
    }

    fn finish_line(&mut self, ctx: &mut Context) {
        if self.oversize {
            ctx.skipped_lines += 1;
        } else if !self.line.is_empty() {
            match serde_json::from_slice::<Value>(&self.line) {
                Ok(record) => {
                    if self.version.is_none() {
                        self.version = metadata_field(&record, "version").map(str::to_owned);
                    }
                    self.done = self.matcher.feed(&record, ctx);
                }
                Err(_) => ctx.skipped_lines += 1,
            }
        }
        self.line.clear();
        self.oversize = false;
    }
}

/// Streaming state of one query over the records, in file order.
struct Matcher<'a> {
    query: Query<'a>,
    found: bool,
    /// `BeforeToolUse`: assistant text since the last user record.
    run: VecDeque<TextBlock>,
    run_truncated: bool,
}

impl<'a> Matcher<'a> {
    fn new(query: Query<'a>) -> Self {
        Self {
            query,
            found: false,
            run: VecDeque::new(),
            run_truncated: false,
        }
    }

    /// Feed one record; `true` when the answer is complete.
    fn feed(&mut self, record: &Value, ctx: &mut Context) -> bool {
        // Unknown record types fall through both arms: ignored.
        match record.get("type").and_then(Value::as_str) {
            Some("user") => match self.query {
                Query::BeforeToolUse(_) => {
                    self.run.clear();
                    self.run_truncated = false;
                    false
                }
                Query::Turn(wanted) => {
                    let id = record.get("promptId").and_then(Value::as_str);
                    if id == Some(wanted) {
                        self.found = true;
                        false
                    } else {
                        // A user record of another turn ends ours.
                        self.found
                    }
                }
            },
            Some("assistant") => match self.query {
                Query::BeforeToolUse(id) => {
                    for block in text_blocks(record) {
                        // Unmatched narration can accumulate over many pages.
                        if self.run.len() == MAX_NARRATION_BLOCKS {
                            self.run.pop_front();
                            self.run_truncated = true;
                        }
                        self.run.push_back(block);
                    }
                    if has_tool_use(record, id) {
                        self.found = true;
                        ctx.texts.extend(self.run.drain(..));
                        ctx.narration_truncated = self.run_truncated;
                        return true;
                    }
                    false
                }
                Query::Turn(_) => {
                    if self.found {
                        ctx.texts.extend(text_blocks(record));
                    }
                    false
                }
            },
            _ => false,
        }
    }
}

fn content_blocks(record: &Value) -> &[Value] {
    record
        .pointer("/message/content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn metadata_field<'a>(record: &'a Value, field: &str) -> Option<&'a str> {
    record
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| s.len() <= MAX_METADATA_BYTES)
}

fn text_blocks(record: &Value) -> impl Iterator<Item = TextBlock> {
    let uuid = metadata_field(record, "uuid");
    let timestamp = metadata_field(record, "timestamp");
    content_blocks(record)
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .filter(|t| !t.trim().is_empty())
        .map(move |t| TextBlock {
            uuid: uuid.map(str::to_owned),
            timestamp: timestamp.map(str::to_owned),
            text: excerpt(t),
        })
}

fn has_tool_use(record: &Value, id: &str) -> bool {
    content_blocks(record).iter().any(|b| {
        b.get("type").and_then(Value::as_str) == Some("tool_use")
            && b.get("id").and_then(Value::as_str) == Some(id)
    })
}
