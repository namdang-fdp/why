//! The spool: where the hook parks events the app could not take, and the
//! protocol that lets the app drain it while hooks keep appending.
//!
//! Layout: `<spool dir>/<YYYY-MM-DD>.jsonl`, one envelope line per event
//! (the very bytes that would have gone over the socket). Files and directory
//! are private to the user (`0600` / `0700`: events hold prompts and code).
//!
//! # Appending (the hook)
//!
//! Each event is `"\n"` + its line, written as ONE `write(2)` on a file opened
//! with `O_APPEND`. The leading newline is the framing: if an earlier append was
//! cut short (disk full) it left a fragment without its `\n`, and the next
//! complete record would otherwise be glued to it and lost with it; the extra
//! newline ends the fragment, so the drain sees one malformed line and then the
//! good record. The drain skips empty lines.
//!
//! The file is opened `O_NONBLOCK | O_NOFOLLOW` and checked to be a regular
//! file: a FIFO planted at the path would otherwise block the hook (and so the
//! agent) forever in `open`. For a regular file `O_NONBLOCK` changes nothing. With
//! `O_APPEND` the kernel moves to end-of-file and writes as a single step, so
//! two processes appending at once never overwrite each other. The nuance
//! people mix up: the `PIPE_BUF` (4096-byte) atomicity guarantee is for *pipes
//! and FIFOs*; regular files have no such size limit, but POSIX only promises
//! the seek-and-write is atomic, and a *short* write (disk full, signal) could
//! still interleave with another writer's bytes if we retried. So the hook
//! never retries: a short write is a failed append, and the next drain skips
//! the broken (unterminated or non-JSON) line as malformed. On NFS `O_APPEND`
//! is not atomic at all; the data dir is local on every platform we ship.
//!
//! # Draining (the app) and the race it has to avoid
//!
//! The app must read a file, ingest it, then delete it. Deleting a file a hook
//! is about to append to would lose that event: the hook `open`ed the path
//! before the app renamed it, and `write`s after the app finished reading. The
//! fix is `flock(2)` on the file:
//!
//! 1. the hook opens the file, then takes a SHARED lock (non-blocking);
//! 2. the app renames `x.jsonl` to `x.jsonl.draining-N` (new hooks now create a
//!    fresh `x.jsonl`), then takes the EXCLUSIVE lock, which waits for every
//!    hook still holding the shared one (they hold it for microseconds);
//! 3. the app reads to end-of-file, ingests, deletes the file and unlocks.
//!
//! A hook whose shared lock FAILS (the app holds the exclusive one) or whose
//! file turns out to have been unlinked (link count 0: it opened the path just
//! before the delete) simply re-opens the path and gets a new file. Either the
//! hook's line is in the file before the app reads it, or the hook never wrote
//! to that file. Nothing is lost, and nothing waits on a lock for long.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Most the spool directory may hold. Past it new events are dropped: an app
/// that stays down for days must not fill the user's disk with prompts.
pub const MAX_SPOOL_BYTES: u64 = 50 * 1024 * 1024;

/// Spool files older than this are no longer counted against the cap: the app
/// deletes them at its next drain (events older than 30 days are never ingested,
/// decision 22), so they must not block new events in the meantime.
pub const SPOOL_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Suffix of a file the app took for draining: `<name>.jsonl.draining-<n>`.
pub const DRAINING_MARKER: &str = ".jsonl.draining-";

/// How many times an append retries after meeting a drain in progress.
const APPEND_ATTEMPTS: usize = 3;

/// What [`append_line`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appended {
    Written,
    /// The spool already holds [`MAX_SPOOL_BYTES`]; the event was dropped.
    CapReached,
}

/// `YYYY-MM-DD` (UTC) for a Unix time in milliseconds.
pub fn date_stamp(unix_ms: u64) -> String {
    let (y, m, d) = civil_from_days(i64::try_from(unix_ms / 86_400_000).unwrap_or(0));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
/// Howard Hinnant's `civil_from_days`; written out so the hook needs no
/// date crate (every dependency costs start-up time on every agent event).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era, 0..=146096
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // 0..=399
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365
    let mp = (5 * doy + 2) / 153; // March-based month, 0..=11
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Total size of the live (not yet expired) regular files in `dir`
/// (0 for a missing directory).
fn spool_size(dir: &Path) -> u64 {
    let oldest_live = SystemTime::now().checked_sub(SPOOL_RETENTION);
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .filter(|m| match (oldest_live, m.modified()) {
                    (Some(limit), Ok(modified)) => modified >= limit,
                    _ => true,
                })
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// `flock(fd, op)`, retrying when a signal interrupts it.
fn flock(file: &File, op: libc::c_int) -> io::Result<()> {
    loop {
        // SAFETY: `file` owns a valid open descriptor for the whole call.
        if unsafe { libc::flock(file.as_raw_fd(), op) } == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// Append `line` (one envelope with its `\n`) to today's spool file in `dir`.
///
/// Creates `dir` (`0700`) and the file (`0600`) when missing. Never blocks on
/// another process for longer than a few `open`/`flock` calls and never
/// retries a short write (see the module docs).
pub fn append_line(dir: &Path, line: &[u8], now_ms: u64, cap: u64) -> io::Result<Appended> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    // `"\n" + line`: see the module docs (framing after a torn append).
    let mut record = Vec::with_capacity(line.len() + 1);
    record.push(b'\n');
    record.extend_from_slice(line);
    if spool_size(dir).saturating_add(record.len() as u64) > cap {
        return Ok(Appended::CapReached);
    }
    let path = dir.join(format!("{}.jsonl", date_stamp(now_ms)));

    for _ in 0..APPEND_ATTEMPTS {
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            // Refuse a symlink planted at the spool path (it could redirect
            // the append to a file the user did not mean to write).
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)?;
        // A FIFO, socket or device at the spool path is not ours to write to.
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the spool path is not a regular file",
            ));
        }
        // A held exclusive lock means the app is draining this very file:
        // start over on the path, which now names a fresh file.
        if flock(&file, libc::LOCK_SH | libc::LOCK_NB).is_err() {
            continue;
        }
        // The app may have deleted the file between our `open` and `flock`.
        if file.metadata()?.nlink() == 0 {
            continue;
        }
        // ONE write call, deliberately not `write_all` (module docs).
        let written = file.write(&record)?;
        return if written == record.len() {
            Ok(Appended::Written)
        } else {
            Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short write to the spool",
            ))
        };
        // `file` drops here: the descriptor closes and the shared lock with it.
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "the spool file kept changing under us",
    ))
}

/// A spool file the app has claimed and holds the exclusive lock on.
/// Dropping it (without [`ClaimedFile::delete`]) releases the lock and leaves
/// the file for the next drain.
#[derive(Debug)]
pub struct ClaimedFile {
    path: PathBuf,
    file: File,
}

impl ClaimedFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// When the file was last appended to. A claimed file whose last append is
    /// older than the retention window holds only expired events.
    pub fn modified(&self) -> io::Result<SystemTime> {
        self.file.metadata()?.modified()
    }

    /// Everything in the file. Safe to call once the claim exists: no hook can
    /// append any more (module docs).
    pub fn read_all(&mut self, max_bytes: u64) -> io::Result<Vec<u8>> {
        use io::Read;
        let mut buf = Vec::new();
        (&mut self.file).take(max_bytes + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "spool file too big",
            ));
        }
        Ok(buf)
    }

    /// Delete the file (while still holding the lock) and release it.
    pub fn delete(self) -> io::Result<()> {
        fs::remove_file(&self.path)
    }
}

/// Take a spool file for draining.
///
/// `path` may be a live `<date>.jsonl` (it is first renamed to
/// `<name>.jsonl.draining-<ticket>`, so hooks move on to a fresh file) or an
/// already renamed leftover of a crashed drain. Waits up to `lock_wait` for
/// hooks that are mid-append. `Ok(None)` when the file vanished or could not
/// be locked in time (try again at the next drain).
pub fn claim(path: &Path, ticket: u64, lock_wait: Duration) -> io::Result<Option<ClaimedFile>> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let Some(name) = name else { return Ok(None) };
    let target = if name.contains(DRAINING_MARKER) {
        path.to_path_buf()
    } else if name.ends_with(".jsonl") {
        let renamed = path.with_file_name(format!("{name}.draining-{ticket}"));
        match fs::rename(path, &renamed) {
            Ok(()) => renamed,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        }
    } else {
        return Ok(None);
    };

    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&target)
    {
        Ok(f) if f.metadata()?.is_file() => f,
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    // Non-blocking attempts in a loop instead of a blocking `flock`: a hook
    // stuck holding its shared lock must not wedge the app's drain task.
    let deadline = Instant::now() + lock_wait;
    loop {
        match flock(&file, libc::LOCK_EX | libc::LOCK_NB) {
            Ok(()) => return Ok(Some(ClaimedFile { path: target, file })),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e),
        }
    }
}

/// The spool files to drain, oldest name first: leftovers of a crashed drain
/// (`*.jsonl.draining-*`) and live `*.jsonl` files. Names only; [`claim`]
/// does the rest. Missing directory: empty list.
pub fn pending_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.metadata().map(|m| m.is_file()).unwrap_or(false))
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name().is_some_and(|n| {
                        let n = n.to_string_lossy();
                        n.ends_with(".jsonl") || n.contains(DRAINING_MARKER)
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::thread;

    use tempfile::TempDir;

    use super::*;

    const DAY1: u64 = 1_795_163_330_579; // 2026-11-20T...
    const CAP: u64 = MAX_SPOOL_BYTES;

    #[test]
    fn dates_are_utc_calendar_dates() {
        assert_eq!(date_stamp(0), "1970-01-01");
        assert_eq!(date_stamp(86_399_999), "1970-01-01");
        assert_eq!(date_stamp(86_400_000), "1970-01-02");
        assert_eq!(date_stamp(951_782_400_000), "2000-02-29"); // leap day
        assert_eq!(date_stamp(1_709_164_800_000), "2024-02-29");
        assert_eq!(date_stamp(4_102_444_799_999), "2099-12-31");
        assert_eq!(date_stamp(DAY1), "2026-11-20");
    }

    #[test]
    fn append_creates_private_files_and_dirs() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("a/spool");
        assert_eq!(
            append_line(&dir, b"one\n", DAY1, CAP).unwrap(),
            Appended::Written
        );
        assert_eq!(
            append_line(&dir, b"two\n", DAY1, CAP).unwrap(),
            Appended::Written
        );
        let file = dir.join("2026-11-20.jsonl");
        assert_eq!(fs::read(&file).unwrap(), b"\none\n\ntwo\n");
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn the_cap_drops_new_events() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(
            append_line(tmp.path(), b"12345\n", DAY1, 10).unwrap(),
            Appended::Written
        );
        assert_eq!(
            append_line(tmp.path(), b"12345\n", DAY1, 10).unwrap(),
            Appended::CapReached
        );
        assert_eq!(
            fs::read(tmp.path().join("2026-11-20.jsonl")).unwrap(),
            b"\n12345\n"
        );
        // Files being drained count: they still occupy the disk.
        fs::write(tmp.path().join("old.jsonl.draining-1"), vec![b'x'; 100]).unwrap();
        assert_eq!(
            append_line(tmp.path(), b"1\n", DAY1, 50).unwrap(),
            Appended::CapReached
        );
    }

    #[test]
    fn a_symlink_at_the_spool_path_is_refused() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("victim");
        fs::write(&target, b"keep").unwrap();
        let dir = tmp.path().join("spool");
        fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&target, dir.join("2026-11-20.jsonl")).unwrap();
        assert!(append_line(&dir, b"x\n", DAY1, CAP).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn a_torn_append_cannot_swallow_the_next_record() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("2026-11-20.jsonl");
        // An append that was cut short: a fragment with no newline.
        fs::write(&file, b"\n{\"v\":1,\"tor").unwrap();
        append_line(tmp.path(), b"{\"good\":1}\n", DAY1, CAP).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(
            lines,
            ["{\"v\":1,\"tor", "{\"good\":1}"],
            "the good record must stay on its own line"
        );
    }

    #[test]
    fn a_fifo_at_the_spool_path_is_refused_without_blocking() {
        let tmp = TempDir::new().unwrap();
        let fifo = tmp.path().join("2026-11-20.jsonl");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // Keep a reader open so an `open(O_WRONLY)` would SUCCEED if it were
        // attempted without the regular-file check.
        let _reader = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap();
        let t = Instant::now();
        assert!(append_line(tmp.path(), b"x\n", DAY1, CAP).is_err());
        assert!(t.elapsed() < Duration::from_secs(2));
        // And with NO reader (where a blocking open would hang): also an error.
        drop(_reader);
        let t = Instant::now();
        assert!(append_line(tmp.path(), b"x\n", DAY1, CAP).is_err());
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn expired_files_do_not_count_against_the_cap() {
        let tmp = TempDir::new().unwrap();
        let old = tmp.path().join("2026-01-01.jsonl");
        fs::write(&old, vec![b'x'; 100]).unwrap();
        let long_ago = SystemTime::now() - SPOOL_RETENTION - Duration::from_secs(3600);
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        assert_eq!(
            append_line(tmp.path(), b"1\n", DAY1, 50).unwrap(),
            Appended::Written
        );
    }

    #[test]
    fn an_unwritable_spool_dir_is_an_error_not_a_panic() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("ro");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let result = append_line(&dir.join("spool"), b"x\n", DAY1, CAP);
        // Root ignores directory permissions; only assert when they apply.
        if unsafe { libc::geteuid() } != 0 {
            assert!(result.is_err());
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn claim_renames_locks_reads_and_deletes() {
        let tmp = TempDir::new().unwrap();
        append_line(tmp.path(), b"a\n", DAY1, CAP).unwrap();
        let live = tmp.path().join("2026-11-20.jsonl");
        let mut claimed = claim(&live, 7, Duration::from_secs(1)).unwrap().unwrap();
        assert!(!live.exists(), "the live name is free for new hooks");
        assert!(
            claimed
                .path()
                .to_string_lossy()
                .ends_with(".jsonl.draining-7")
        );
        // A hook appending now lands in a NEW file.
        append_line(tmp.path(), b"b\n", DAY1, CAP).unwrap();
        assert_eq!(fs::read(&live).unwrap(), b"\nb\n");
        assert_eq!(claimed.read_all(1024).unwrap(), b"\na\n");
        claimed.delete().unwrap();
        assert_eq!(pending_files(tmp.path()), [live]);
    }

    #[test]
    fn a_leftover_from_a_crashed_drain_is_claimable_and_pending() {
        let tmp = TempDir::new().unwrap();
        let left = tmp.path().join("2026-11-18.jsonl.draining-3");
        fs::write(&left, b"x\n").unwrap();
        assert_eq!(pending_files(tmp.path()), std::slice::from_ref(&left));
        let c = claim(&left, 99, Duration::from_millis(10))
            .unwrap()
            .unwrap();
        assert_eq!(c.path(), left, "a leftover keeps its name");
        assert!(
            claim(&tmp.path().join("gone.jsonl"), 1, Duration::ZERO)
                .unwrap()
                .is_none()
        );
        assert!(
            claim(&tmp.path().join("notes.txt"), 1, Duration::ZERO)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn claim_waits_for_a_hook_that_is_mid_append_then_times_out() {
        let tmp = TempDir::new().unwrap();
        let live = tmp.path().join("2026-11-20.jsonl");
        fs::write(&live, b"a\n").unwrap();
        // A hook holding its shared lock.
        let hook = File::open(&live).unwrap();
        flock(&hook, libc::LOCK_SH).unwrap();
        let t = Instant::now();
        assert!(
            claim(&live, 1, Duration::from_millis(60))
                .unwrap()
                .is_none()
        );
        assert!(t.elapsed() >= Duration::from_millis(60));
        // The renamed file stays pending: the next drain picks it up.
        assert_eq!(pending_files(tmp.path()).len(), 1);
        drop(hook);
        let leftover = pending_files(tmp.path()).remove(0);
        assert!(
            claim(&leftover, 2, Duration::from_millis(60))
                .unwrap()
                .is_some()
        );
    }

    /// Hooks append while the app drains, over and over: every line must end up
    /// in exactly one drained file (or still be pending), none lost, none cut.
    #[test]
    fn nothing_is_lost_while_draining() {
        const WRITERS: usize = 4;
        const PER_WRITER: usize = 400;
        let tmp = Arc::new(TempDir::new().unwrap());
        let writers: Vec<_> = (0..WRITERS)
            .map(|w| {
                let tmp = Arc::clone(&tmp);
                thread::spawn(move || {
                    for i in 0..PER_WRITER {
                        let line = format!("{w}-{i}\n");
                        // A drain in progress may make an attempt fail; a real
                        // hook would then spool nothing, so count outcomes.
                        let mut tries = 0;
                        while append_line(tmp.path(), line.as_bytes(), DAY1, CAP).is_err() {
                            tries += 1;
                            assert!(tries < 100, "append kept failing");
                        }
                    }
                })
            })
            .collect();

        let mut seen: Vec<String> = Vec::new();
        let mut ticket = 0;
        let mut drain = |seen: &mut Vec<String>| {
            for p in pending_files(tmp.path()) {
                ticket += 1;
                if let Some(mut c) = claim(&p, ticket, Duration::from_secs(2)).unwrap() {
                    let bytes = c.read_all(CAP).unwrap();
                    seen.extend(
                        String::from_utf8(bytes)
                            .unwrap()
                            .lines()
                            .filter(|l| !l.is_empty())
                            .map(str::to_owned),
                    );
                    c.delete().unwrap();
                }
            }
        };
        while writers.iter().any(|w| !w.is_finished()) {
            drain(&mut seen);
        }
        for w in writers {
            w.join().unwrap();
        }
        drain(&mut seen);

        assert_eq!(
            seen.len(),
            WRITERS * PER_WRITER,
            "a line was lost or duplicated"
        );
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), WRITERS * PER_WRITER, "duplicates");
        assert!(pending_files(tmp.path()).is_empty());
    }
}
