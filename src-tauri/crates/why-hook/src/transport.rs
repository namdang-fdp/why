//! Sending one line to the collector's unix socket within a hard time budget.
//!
//! Why not `std::os::unix::net::UnixStream::connect`: it has no timeout, and a
//! unix socket whose listener is not accepting makes `connect` (and later
//! `write`) block once the kernel's queues are full. A hook that blocks stalls
//! the agent's tool call until the harness kills it (up to its 600 s default
//! timeout), so every step here runs on a NON-BLOCKING socket and waits with
//! `poll(2)` against ONE deadline shared by connect and write.
//!
//! The hook never waits for a reply: it writes the line and closes. The
//! collector acknowledges nothing, so a stuck or slow app costs the agent at
//! most [`crate::SEND_BUDGET`].

use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::time::{Duration, Instant};

/// Retry spacing while the listener's backlog is full (Linux answers `EAGAIN`
/// to a non-blocking `connect` then, and does not tell us when to retry).
const BACKLOG_RETRY: Duration = Duration::from_millis(2);

fn os_err() -> io::Error {
    io::Error::last_os_error()
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "socket send budget exhausted")
}

/// Wait until `fd` is ready for `events` or the deadline passes.
fn wait(fd: &OwnedFd, events: libc::c_short, deadline: Instant) -> io::Result<()> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(timed_out());
        }
        let mut pfd = libc::pollfd {
            fd: fd.as_raw_fd(),
            events,
            revents: 0,
        };
        // Round up so a sub-millisecond remainder is not rounded to "no wait".
        let ms = left.as_millis().min(i32::MAX as u128) as libc::c_int + 1;
        // SAFETY: one valid pollfd, count 1.
        let n = unsafe { libc::poll(&mut pfd, 1, ms) };
        if n > 0 {
            return Ok(());
        }
        if n < 0 && os_err().kind() != io::ErrorKind::Interrupted {
            return Err(os_err());
        }
        // n == 0 (timeout; the loop top decides) or EINTR: go round again.
    }
}

fn new_socket() -> io::Result<OwnedFd> {
    // SAFETY: plain syscall; the result is checked before use.
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw < 0 {
        return Err(os_err());
    }
    // SAFETY: `raw` is a fresh descriptor nobody else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // Close-on-exec (we never exec, but a descriptor must not leak into any
    // process we might spawn later) and non-blocking (the whole point).
    // SAFETY: fcntl on a valid descriptor with integer arguments.
    unsafe {
        if libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) < 0
            || libc::fcntl(raw, libc::F_SETFL, libc::O_NONBLOCK) < 0
        {
            return Err(os_err());
        }
    }
    Ok(fd)
}

/// `sockaddr_un` for `path`. A unix socket path must fit `sun_path` (108 bytes
/// on Linux, 104 on macOS, NUL included); longer paths cannot be used at all.
fn sockaddr(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: `sockaddr_un` is plain old data; all-zero is a valid value.
    let mut addr: libc::sockaddr_un = unsafe { mem::zeroed() };
    if bytes.is_empty() || bytes.len() >= addr.sun_path.len() || bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path is empty, contains NUL or is too long for sun_path",
        ));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    Ok((addr, mem::size_of::<libc::sockaddr_un>() as libc::socklen_t))
}

fn connect(fd: &OwnedFd, path: &Path, deadline: Instant) -> io::Result<()> {
    let (addr, len) = sockaddr(path)?;
    loop {
        // SAFETY: `addr` outlives the call and `len` is its size.
        let rc = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&addr as *const libc::sockaddr_un).cast::<libc::sockaddr>(),
                len,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let err = os_err();
        match err.raw_os_error() {
            Some(libc::EINTR) => {}
            // Backlog full: nobody is accepting. Retry until the deadline.
            Some(libc::EAGAIN) => {
                if Instant::now() + BACKLOG_RETRY >= deadline {
                    return Err(timed_out());
                }
                std::thread::sleep(BACKLOG_RETRY);
            }
            // BSD-style "connecting": wait for writability, then read the result.
            Some(libc::EINPROGRESS) => {
                wait(fd, libc::POLLOUT, deadline)?;
                let mut so_err: libc::c_int = 0;
                let mut n = mem::size_of::<libc::c_int>() as libc::socklen_t;
                // SAFETY: `so_err` and `n` are valid for the call.
                let rc = unsafe {
                    libc::getsockopt(
                        fd.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_ERROR,
                        (&mut so_err as *mut libc::c_int).cast(),
                        &mut n,
                    )
                };
                if rc < 0 {
                    return Err(os_err());
                }
                return if so_err == 0 {
                    Ok(())
                } else {
                    Err(io::Error::from_raw_os_error(so_err))
                };
            }
            // ENOENT (no socket file), ECONNREFUSED (file but no listener), ...
            _ => return Err(err),
        }
    }
}

fn send_all(fd: &OwnedFd, mut buf: &[u8], deadline: Instant) -> io::Result<()> {
    // `MSG_NOSIGNAL`: a peer that closed early must give us `EPIPE`, not a
    // SIGPIPE. (macOS has no such flag; Rust's runtime already ignores SIGPIPE.)
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const FLAGS: libc::c_int = libc::MSG_NOSIGNAL;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const FLAGS: libc::c_int = 0;

    while !buf.is_empty() {
        // SAFETY: `buf` is valid for `buf.len()` bytes.
        let n = unsafe { libc::send(fd.as_raw_fd(), buf.as_ptr().cast(), buf.len(), FLAGS) };
        if n >= 0 {
            buf = &buf[n as usize..];
            continue;
        }
        let err = os_err();
        match err.kind() {
            io::ErrorKind::Interrupted => {}
            // The socket buffer is full (a slow or stuck reader).
            io::ErrorKind::WouldBlock => wait(fd, libc::POLLOUT, deadline)?,
            _ => return Err(err),
        }
    }
    Ok(())
}

/// Connect to `socket` and write `line`, all within `budget`.
///
/// `Ok` means every byte is in the kernel's socket buffer, NOT that the app
/// processed it (no reply is awaited). On `Err` the caller spools the line.
/// A line only partly written when the budget ran out is discarded by the
/// collector (it never decodes a line without its `\n`), and the spooled
/// copy is the one that counts, so no event is delivered twice.
pub fn send_line(socket: &Path, line: &[u8], budget: Duration) -> io::Result<()> {
    let deadline = Instant::now() + budget;
    let fd = new_socket()?;
    connect(&fd, socket, deadline)?;
    send_all(&fd, line, deadline)
    // `fd` closes here: the collector sees our line, then EOF.
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::os::unix::net::UnixListener;

    use tempfile::TempDir;

    use super::*;
    use crate::SEND_BUDGET;

    #[test]
    fn a_line_reaches_a_listener() {
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("s.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        send_line(&sock, b"hello\n", SEND_BUDGET).unwrap();
        let (mut conn, _) = listener.accept().unwrap();
        let mut got = String::new();
        conn.read_to_string(&mut got).unwrap();
        assert_eq!(got, "hello\n");
    }

    #[test]
    fn missing_socket_and_dead_listener_fail_fast() {
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("s.sock");
        let e = send_line(&sock, b"x\n", SEND_BUDGET).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        // A socket file with nobody behind it (the listener died).
        drop(UnixListener::bind(&sock).unwrap());
        // Another test thread's fork may briefly hold a copy of the listener
        // descriptor; wait until the socket is really dead.
        while std::os::unix::net::UnixStream::connect(&sock).is_ok() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let t = Instant::now();
        let e = send_line(&sock, b"x\n", SEND_BUDGET).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
        assert!(t.elapsed() < SEND_BUDGET + Duration::from_millis(400));
    }

    #[test]
    fn too_long_paths_are_errors() {
        let long = Path::new("/").join("x".repeat(200));
        assert!(send_line(&long, b"x\n", SEND_BUDGET).is_err());
        assert!(send_line(Path::new(""), b"x\n", SEND_BUDGET).is_err());
    }

    #[test]
    fn a_stuck_reader_costs_the_budget_not_more() {
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("s.sock");
        // Listener that never `accept`s and never reads. The connection sits in
        // the backlog; a line bigger than the socket buffers cannot be written.
        let _listener = UnixListener::bind(&sock).unwrap();
        let big = vec![b'x'; 8 * 1024 * 1024];
        let t = Instant::now();
        let e = send_line(&sock, &big, SEND_BUDGET).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
        assert!(
            // Generous slack: a loaded CI machine can delay a wake-up. The point
            // is "about the budget, not forever" (8 MiB never fits the buffer).
            t.elapsed() < SEND_BUDGET + Duration::from_millis(400),
            "took {:?}",
            t.elapsed()
        );
    }
}
