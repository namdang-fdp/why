//! The collector's unix-socket server.
//!
//! One task accepts connections; each connection gets a small task that reads
//! newline-delimited envelopes (bounded), decodes them and offers them to the
//! ingest task over a BOUNDED channel. Every limit here exists because the
//! other end is another program: it may be buggy, slow, hostile or just a
//! crowd. Nothing a peer does may panic us, grow our memory without bound or
//! hold a resource forever.

use std::fs::File;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Semaphore, mpsc, watch};
use why_wire::{Envelope, MAX_LINE_BYTES, WireError, decode_line};

use super::stats::{Stats, bump};

/// Limits for the server. `Default` is what the app runs with; tests shrink
/// the durations.
#[derive(Debug, Clone, Copy)]
pub struct ServerConfig {
    /// Most connections served at once. A hook connects, writes one line and
    /// closes within milliseconds, so even a burst of parallel agents stays far
    /// below this; past it new connections are closed unread.
    pub max_connections: usize,
    /// Longest silence between two reads. A client that connects and sends
    /// nothing is dropped after this.
    pub idle_timeout: Duration,
    /// Longest a connection may live in total (a client dribbling one byte at a
    /// time keeps resetting the idle timer; this bounds it anyway).
    pub connection_deadline: Duration,
    /// Most bytes of envelope lines waiting in the ingest queue. The queue is
    /// also bounded by COUNT, but a count alone is no memory bound: 1024
    /// envelopes of 1 MiB (a big `Write`) are a gigabyte. Over budget, new
    /// envelopes are dropped and counted. (Parsed JSON takes a few times its
    /// text size, so the real peak is a multiple of this; the default keeps
    /// that well under a few hundred MiB.)
    pub max_queued_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_connections: 128,
            idle_timeout: Duration::from_secs(2),
            connection_deadline: Duration::from_secs(10),
            max_queued_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Bytes of envelope lines currently queued for ingest.
#[derive(Debug)]
pub struct ByteBudget {
    used: AtomicUsize,
    max: usize,
}

impl ByteBudget {
    pub fn new(max: usize) -> Arc<Self> {
        Arc::new(Self {
            used: AtomicUsize::new(0),
            max,
        })
    }

    /// Claim `n` bytes; `false` (nothing claimed) when that would pass the cap.
    #[allow(deprecated)] // `try_update` is newer than this crate's MSRV (1.90)
    pub fn try_reserve(&self, n: usize) -> bool {
        self.used
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(n).filter(|total| *total <= self.max)
            })
            .is_ok()
    }

    pub fn release(&self, n: usize) {
        self.used.fetch_sub(n, Ordering::SeqCst);
    }

    pub fn used(&self) -> usize {
        self.used.load(Ordering::SeqCst)
    }
}

/// An envelope waiting for ingest, with the bytes it holds against the budget.
#[derive(Debug)]
pub struct Queued {
    pub env: Envelope,
    pub bytes: usize,
}

/// Why the socket could not be bound.
#[derive(Debug, Error)]
pub enum BindError {
    /// `sun_path` holds ~108 bytes (104 on macOS); `bind` would fail with a
    /// cryptic `InvalidInput`, so we say what is wrong.
    #[error("socket path is {len} bytes, longer than the {max}-byte unix socket limit: {path}")]
    PathTooLong {
        path: PathBuf,
        len: usize,
        max: usize,
    },
    /// Another process answers on the socket (usually another WHY with the same
    /// instance). We never take over a live socket.
    #[error("another process is listening on {0}")]
    InUse(PathBuf),
    /// The path exists but is not a socket: refuse to delete someone's file.
    #[error("{0} exists and is not a socket")]
    NotASocket(PathBuf),
    #[error("cannot bind {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Longest usable socket path, in bytes (`sun_path` minus the NUL).
fn max_socket_path() -> usize {
    // SAFETY: `sockaddr_un` is plain old data; zeroed is a valid value.
    let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_path.len() - 1
}

/// Remembers which socket file THIS process created and removes only that one.
///
/// Identity is (device, inode), not the path: if another WHY instance replaced
/// the file after we died or were stopped, a plain `remove_file(path)` would
/// delete ITS socket and silently cut it off from every hook.
#[derive(Debug)]
pub struct SocketGuard {
    path: PathBuf,
    identity: (u64, u64),
    released: AtomicBool,
    /// The per-instance lock file, held (flock) for as long as we own the socket.
    lock: Mutex<Option<File>>,
}

impl SocketGuard {
    /// Remove the socket file if it is still the one we bound. Idempotent.
    pub fn release(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(meta) = std::fs::symlink_metadata(&self.path)
            && meta.file_type().is_socket()
            && (meta.dev(), meta.ino()) == self.identity
        {
            let _ = std::fs::remove_file(&self.path);
        }
        // Unlock last: another instance may then take over a clean directory.
        // (The lock FILE stays: deleting it would let two processes lock two
        // different files of the same name.)
        if let Ok(mut lock) = self.lock.lock() {
            lock.take();
        }
    }
}

/// A listening socket plus the guard that cleans it up.
pub struct Bound {
    pub listener: UnixListener,
    pub guard: Arc<SocketGuard>,
}

/// Bind `path`, creating its directory `0700`.
///
/// If the path is taken we PROBE it: connect. An answer means another instance
/// is alive (error, never steal). `ECONNREFUSED`/`ENOENT` means the file is a
/// leftover of a crashed process ("stale socket": the file outlives the process
/// that bound it, and `bind` fails with `EADDRINUSE` on it forever), so we
/// unlink and bind again.
pub async fn bind(path: &Path) -> Result<Bound, BindError> {
    let io_err = |source| BindError::Io {
        path: path.to_owned(),
        source,
    };
    let len = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()).len();
    if len > max_socket_path() {
        return Err(BindError::PathTooLong {
            path: path.to_owned(),
            len,
            max: max_socket_path(),
        });
    }
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(io_err)?;
    }

    // One collector per socket path, decided by a lock, not by racing on the
    // socket file. Without it, two instances starting after a crash can both
    // find the old socket dead, and the second one's `unlink` then deletes the
    // socket the first one just bound.
    let lock = take_instance_lock(path)?;

    let listener = match UnixListener::bind(path) {
        Ok(l) => l,
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            reclaim_stale(path).await?;
            UnixListener::bind(path).map_err(|e| {
                if e.kind() == io::ErrorKind::AddrInUse {
                    BindError::InUse(path.to_owned()) // lost a race with another instance
                } else {
                    io_err(e)
                }
            })?
        }
        Err(e) => return Err(io_err(e)),
    };
    // The directory is already private, but the file should not rely on that.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io_err)?;
    let meta = std::fs::symlink_metadata(path).map_err(io_err)?;
    Ok(Bound {
        listener,
        guard: Arc::new(SocketGuard {
            path: path.to_owned(),
            identity: (meta.dev(), meta.ino()),
            released: AtomicBool::new(false),
            lock: Mutex::new(Some(lock)),
        }),
    })
}

/// `<socket>.lock`, locked with `flock(LOCK_EX | LOCK_NB)`. The lock belongs to
/// the open file description and is released when the file is closed or the
/// process dies, so a crash never leaves a stuck lock (unlike a pid file).
fn take_instance_lock(socket: &Path) -> Result<File, BindError> {
    use std::os::fd::AsRawFd;
    let io_err = |source| BindError::Io {
        path: socket.to_owned(),
        source,
    };
    let mut name = socket.as_os_str().to_owned();
    name.push(".lock");
    let lock_path = PathBuf::from(name);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&lock_path)
        .map_err(io_err)?;
    // SAFETY: `file` owns a valid descriptor for the whole call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(file);
    }
    let err = io::Error::last_os_error();
    if err.kind() == io::ErrorKind::WouldBlock {
        Err(BindError::InUse(socket.to_owned()))
    } else {
        Err(io_err(err))
    }
}

/// The path is taken: decide between "live" (error) and "stale" (remove).
async fn reclaim_stale(path: &Path) -> Result<(), BindError> {
    let meta = std::fs::symlink_metadata(path).map_err(|source| BindError::Io {
        path: path.to_owned(),
        source,
    })?;
    if !meta.file_type().is_socket() {
        return Err(BindError::NotASocket(path.to_owned()));
    }
    let probe = tokio::time::timeout(Duration::from_millis(500), UnixStream::connect(path)).await;
    match probe {
        // Someone answered, or is too busy to (timeout): not ours to take.
        Ok(Ok(_)) | Err(_) => Err(BindError::InUse(path.to_owned())),
        Ok(Err(e))
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            tracing::info!(socket = %path.display(), "removing a stale collector socket");
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(BindError::Io {
                    path: path.to_owned(),
                    source,
                }),
            }
        }
        Ok(Err(source)) => Err(BindError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

/// One thing a connection produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// A complete line, `\n` included, at most [`MAX_LINE_BYTES`] long.
    Line(Vec<u8>),
    /// The line exceeded [`MAX_LINE_BYTES`] before its newline; its bytes were
    /// discarded and the reader should be abandoned.
    Oversized,
    /// The peer closed in the middle of a line. A partial line is NEVER
    /// decoded: the hook that wrote it gave up and spooled the whole event.
    Unterminated,
}

/// Reads newline-terminated lines without ever buffering more than
/// [`MAX_LINE_BYTES`] of one (`BufRead::read_until` would grow without limit).
pub struct BoundedLines<R> {
    reader: BufReader<R>,
    line: Vec<u8>,
}

impl<R: AsyncRead + Unpin> BoundedLines<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            line: Vec::new(),
        }
    }

    /// The next frame, or `None` at a clean end of stream (EOF at a line
    /// boundary). Cancel-safe: dropping the future loses no bytes.
    pub async fn next_frame(&mut self) -> io::Result<Option<Frame>> {
        loop {
            let buf = self.reader.fill_buf().await?;
            if buf.is_empty() {
                let partial = !self.line.is_empty();
                self.line.clear();
                return Ok(partial.then_some(Frame::Unterminated));
            }
            match buf.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    let take = i + 1;
                    if self.line.len() + take > MAX_LINE_BYTES {
                        return Ok(Some(Frame::Oversized));
                    }
                    self.line.extend_from_slice(&buf[..take]);
                    self.reader.consume(take);
                    return Ok(Some(Frame::Line(std::mem::take(&mut self.line))));
                }
                None => {
                    let n = buf.len();
                    if self.line.len() + n > MAX_LINE_BYTES {
                        return Ok(Some(Frame::Oversized));
                    }
                    self.line.extend_from_slice(buf);
                    self.reader.consume(n);
                }
            }
        }
    }
}

/// Is a peer with `peer_uid` allowed to talk to a collector run by `my_uid`?
/// Only the same user: the data (prompts, code) is that user's alone.
pub fn peer_allowed(peer_uid: u32, my_uid: u32) -> bool {
    peer_uid == my_uid
}

/// Read one connection to its end (or to a limit) and queue its envelopes.
pub async fn handle_connection<R: AsyncRead + Unpin>(
    stream: R,
    tx: mpsc::Sender<Queued>,
    budget: Arc<ByteBudget>,
    stats: Arc<Stats>,
    cfg: ServerConfig,
) {
    let mut lines = BoundedLines::new(stream);
    let deadline = tokio::time::Instant::now() + cfg.connection_deadline;
    loop {
        // Each read waits at most `idle_timeout`, and the whole connection at
        // most `connection_deadline`.
        let wait = cfg
            .idle_timeout
            .min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        let frame = match tokio::time::timeout(wait, lines.next_frame()).await {
            Err(_elapsed) => {
                bump(&stats.timeouts);
                return;
            }
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "collector connection read failed");
                return;
            }
            Ok(Ok(None)) => return,
            Ok(Ok(Some(frame))) => frame,
        };
        match frame {
            Frame::Line(line) => match decode_line(&line) {
                Ok(env) => {
                    // Over the memory budget: drop. The hook has already exited
                    // and nobody can be told; blocking here would only pile up
                    // connections.
                    let bytes = line.len();
                    if !budget.try_reserve(bytes) {
                        bump(&stats.dropped_memory);
                        continue;
                    }
                    match tx.try_send(Queued { env, bytes }) {
                        Ok(()) => bump(&stats.received),
                        // Queue full (by count): same.
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            budget.release(bytes);
                            bump(&stats.dropped_backpressure);
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            budget.release(bytes);
                            return;
                        }
                    }
                }
                Err(WireError::UnsupportedVersion { v }) => {
                    tracing::debug!(v, "dropping an envelope of an unknown version");
                    bump(&stats.unsupported_version);
                }
                Err(WireError::TooLarge { .. }) => bump(&stats.oversized),
                Err(e) => {
                    tracing::debug!(error = %e, "dropping a malformed line");
                    bump(&stats.malformed);
                }
            },
            Frame::Oversized => {
                tracing::debug!("dropping an oversized line and its connection");
                bump(&stats.oversized);
                return;
            }
            Frame::Unterminated => {
                bump(&stats.unterminated);
                return;
            }
        }
    }
}

/// Accept connections until `shutdown` flips to `true`.
pub async fn serve(
    listener: UnixListener,
    tx: mpsc::Sender<Queued>,
    budget: Arc<ByteBudget>,
    stats: Arc<Stats>,
    cfg: ServerConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let my_uid = unsafe { libc::geteuid() };
    // One permit per live connection. `try_acquire` (never `acquire`): when
    // full we refuse instead of queueing, so a flood cannot build a backlog.
    let slots = Arc::new(Semaphore::new(cfg.max_connections));
    loop {
        let accepted = tokio::select! {
            r = listener.accept() => r,
            _ = shutdown.changed() => return,
        };
        let stream = match accepted {
            Ok((stream, _addr)) => stream,
            Err(e) => {
                // EMFILE and friends: back off a little instead of spinning.
                tracing::warn!(error = %e, "collector accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        // `SO_PEERCRED`: the kernel tells us the uid of the process that
        // connected (it cannot be forged by the peer). The socket is `0600` in
        // a `0700` directory already; this is the second lock on the same door.
        match stream.peer_cred() {
            Ok(cred) if peer_allowed(cred.uid(), my_uid) => {}
            _ => {
                bump(&stats.rejected_peer);
                continue;
            }
        }
        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
            bump(&stats.rejected_busy);
            continue;
        };
        bump(&stats.connections);
        let (tx, budget, stats) = (tx.clone(), Arc::clone(&budget), Arc::clone(&stats));
        tauri::async_runtime::spawn(async move {
            handle_connection(stream, tx, budget, stats, cfg).await;
            drop(permit);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn frames(bytes: &[u8]) -> Vec<Frame> {
        let mut r = BoundedLines::new(bytes);
        let mut out = Vec::new();
        while let Some(f) = r.next_frame().await.unwrap() {
            let stop = f == Frame::Oversized;
            out.push(f);
            if stop {
                break;
            }
        }
        out
    }

    #[tokio::test]
    async fn splits_lines_and_flags_a_partial_tail() {
        assert_eq!(
            frames(b"a\nbb\nccc").await,
            [
                Frame::Line(b"a\n".to_vec()),
                Frame::Line(b"bb\n".to_vec()),
                Frame::Unterminated
            ]
        );
        assert_eq!(frames(b"").await, []);
        assert_eq!(frames(b"\n").await, [Frame::Line(b"\n".to_vec())]);
    }

    #[tokio::test]
    async fn the_limit_is_exact_and_never_buffers_past_it() {
        let mut ok = vec![b'x'; MAX_LINE_BYTES - 1];
        ok.push(b'\n');
        assert_eq!(frames(&ok).await, [Frame::Line(ok.clone())]);

        let mut big = vec![b'x'; MAX_LINE_BYTES];
        big.push(b'\n');
        assert_eq!(frames(&big).await, [Frame::Oversized]);
        // No newline at all, far past the limit: still bounded.
        assert_eq!(
            frames(&vec![b'x'; MAX_LINE_BYTES * 3]).await,
            [Frame::Oversized]
        );
    }

    #[test]
    fn only_our_own_uid_may_connect() {
        assert!(peer_allowed(1000, 1000));
        assert!(!peer_allowed(0, 1000));
        assert!(!peer_allowed(1001, 1000));
    }
}
