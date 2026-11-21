//! Touching a user's config file as carefully as a program can.
//!
//! The file belongs to an agent that may be running right now and to a user who
//! may keep it in a dotfiles repo. So:
//!
//! * we only ever touch a plain file the user owns, in a plain directory the
//!   user owns; symlinks, FIFOs (reading one blocks forever), directories,
//!   devices and other people's files are refused, not followed;
//! * a write goes to a temp file in the same directory, is `fsync`ed, and is
//!   `rename`d over the target: readers see the whole old file or the whole new
//!   file, never a half-written one, even if we crash mid-write;
//! * read-modify-write holds an advisory `flock` on the directory, then
//!   rechecks the config after staging the new bytes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use tempfile::{NamedTempFile, TempPath};

use super::HookError;

/// Configs are tiny; a megabyte-sized one is not something to rewrite blindly.
pub const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

/// What is on disk at the target path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Existing {
    Absent,
    File { bytes: Vec<u8>, mode: u32 },
}

impl Existing {
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Existing::Absent => None,
            Existing::File { bytes, .. } => Some(bytes),
        }
    }

    fn mode(&self) -> u32 {
        match self {
            Existing::Absent => 0o600,
            Existing::File { mode, .. } => *mode,
        }
    }
}

fn euid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, touches no memory and cannot fail.
    unsafe { libc::geteuid() }
}

fn unsafe_target(path: &Path, why: &str) -> HookError {
    HookError::UnsafeTarget(format!("{}: {why}", path.display()))
}

/// Resolve the closest existing directory so aliases and missing config
/// directories cannot hide a worktree. Linked worktrees have a .git file.
pub(super) fn reject_worktree(dir: &Path) -> Result<(), HookError> {
    let mut ancestor = dir;
    let resolved = loop {
        match ancestor.canonicalize() {
            Ok(path) => break path,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| unsafe_target(dir, "cannot resolve its directory"))?;
            }
            Err(e) => return Err(e.into()),
        }
    };
    for ancestor in resolved.ancestors() {
        match fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => return Err(unsafe_target(dir, "is inside a Git worktree")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Check the directory that holds (or will hold) the config. `Ok(false)` = it
/// does not exist yet.
pub fn check_dir(dir: &Path) -> Result<bool, HookError> {
    let meta = match fs::symlink_metadata(dir) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            reject_worktree(dir)?;
            return Ok(false);
        }
        Err(e) => return Err(e.into()),
    };
    // `symlink_metadata` does not follow: a symlinked config directory could
    // point anywhere (out of the home, into a repository).
    if meta.file_type().is_symlink() {
        return Err(unsafe_target(dir, "is a symlink"));
    }
    if !meta.is_dir() {
        return Err(unsafe_target(dir, "is not a directory"));
    }
    if meta.uid() != euid() {
        return Err(unsafe_target(dir, "is not owned by you"));
    }
    reject_worktree(dir)?;
    Ok(true)
}

/// Create the config directory (mode 0700) if it is missing. Only the last
/// path component is created: a missing parent (a missing home) is an error.
pub fn ensure_dir(dir: &Path) -> Result<(), HookError> {
    if check_dir(dir)? {
        return Ok(());
    }
    let parent_ok = dir
        .parent()
        .is_some_and(|p| fs::metadata(p).is_ok_and(|m| m.is_dir()));
    if !parent_ok {
        return Err(HookError::NoHome);
    }
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => Ok(()),
        // Lost a race with another creator: fine, as long as it is a safe dir.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => check_dir(dir).map(|_| ()),
        Err(e) => Err(e.into()),
    }
}

/// Read the config file, refusing anything but a plain, user-owned file.
pub fn read_target(path: &Path) -> Result<Existing, HookError> {
    if let Some(dir) = path.parent()
        && !check_dir(dir)?
    {
        return Ok(Existing::Absent);
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Existing::Absent),
        Err(e) => return Err(e.into()),
    };
    let ft = meta.file_type();
    if ft.is_symlink() {
        // A symlinked settings.json is typical of dotfile managers. A rename
        // would replace the link with a regular file and silently break that
        // setup, and writing through it would edit a file elsewhere; refuse.
        return Err(unsafe_target(path, "is a symlink"));
    }
    if !ft.is_file() {
        return Err(unsafe_target(
            path,
            "is not a regular file (directory, FIFO or device)",
        ));
    }
    if meta.uid() != euid() {
        return Err(unsafe_target(path, "is not owned by you"));
    }
    // O_NOFOLLOW + O_NONBLOCK close the window between the check above and
    // the open: if the path was swapped for a link or FIFO meanwhile, the open
    // fails or the fstat below catches it, instead of hanging.
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Existing::Absent),
        Err(e) => return Err(e.into()),
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != euid() {
        return Err(unsafe_target(path, "changed while it was being opened"));
    }
    if meta.len() > MAX_CONFIG_BYTES {
        return Err(HookError::TooLarge);
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    (&file).take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(HookError::TooLarge);
    }
    Ok(Existing::File {
        bytes,
        mode: meta.mode() & 0o7777,
    })
}

/// Advisory lock on the config directory, released when dropped.
///
/// `flock` is cooperative: it only stops other processes that also take it
/// (another WHY instance, say). An agent writing its own settings does not, so
/// we also check the file immediately before publication.
/// The lock is on the directory itself so no `.lock` file litters `~/.claude`.
#[derive(Debug)]
pub struct DirLock(#[allow(dead_code)] File);

pub fn lock_dir(dir: &Path, timeout: Duration) -> Result<DirLock, HookError> {
    let file = File::open(dir)?;
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(DirLock(file)),
            Err(fs::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(HookError::Locked);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

fn parent(path: &Path) -> Result<&Path, HookError> {
    path.parent()
        .ok_or_else(|| unsafe_target(path, "has no parent directory"))
}

pub(super) fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()?;
    Ok(())
}

fn stage(dir: &Path, bytes: &[u8], mode: u32) -> io::Result<TempPath> {
    let mut staged = NamedTempFile::new_in(dir)?;
    staged.write_all(bytes)?;
    staged
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    staged.as_file().sync_all()?;
    Ok(staged.into_temp_path())
}

/// Store private restore data atomically, including the directory entry.
pub(super) fn write_backup(path: &Path, bytes: &[u8]) -> Result<(), HookError> {
    let dir = parent(path)?;
    stage(dir, bytes, 0o600)?
        .persist(path)
        .map_err(io::Error::from)?;
    sync_dir(dir)?;
    Ok(())
}

/// Publish a config edit or deletion. A stale snapshot leaves the file alone.
pub(super) fn publish(
    path: &Path,
    expected: &Existing,
    bytes: Option<&[u8]>,
) -> Result<bool, HookError> {
    let dir = parent(path)?;
    if !check_dir(dir)? {
        return Ok(false);
    }
    let staged = bytes
        .map(|bytes| stage(dir, bytes, expected.mode()))
        .transpose()?;
    publish_staged(path, dir, expected, staged)
}

fn publish_staged(
    path: &Path,
    dir: &Path,
    expected: &Existing,
    staged: Option<TempPath>,
) -> Result<bool, HookError> {
    if read_target(path)? != *expected {
        return Ok(false);
    }
    // The advisory lock only serializes WHY. The final read narrows the race
    // with other writers for replacement/deletion; creation can be exclusive.
    match staged {
        Some(staged) if matches!(expected, Existing::Absent) => {
            match staged.persist_noclobber(path) {
                Ok(()) => {}
                Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
                Err(e) => return Err(e.error.into()),
            }
        }
        Some(staged) => staged.persist(path).map_err(io::Error::from)?,
        None => match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        },
    }
    sync_dir(dir)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn reads_plain_files_and_reports_absence() {
        let t = TempDir::new().unwrap();
        let p = t.path().join("s.json");
        assert_eq!(read_target(&p).unwrap(), Existing::Absent);
        assert_eq!(
            read_target(&t.path().join("no/dir/s.json")).unwrap(),
            Existing::Absent
        );
        fs::write(&p, b"{}").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(
            read_target(&p).unwrap(),
            Existing::File {
                bytes: b"{}".to_vec(),
                mode: 0o640
            }
        );
    }

    #[test]
    fn refuses_symlinks_directories_and_fifos() {
        let t = TempDir::new().unwrap();
        let real = t.path().join("real.json");
        fs::write(&real, b"{}").unwrap();
        let link = t.path().join("link.json");
        symlink(&real, &link).unwrap();
        assert!(
            matches!(read_target(&link), Err(HookError::UnsafeTarget(m)) if m.contains("symlink"))
        );

        let d = t.path().join("dir.json");
        fs::create_dir(&d).unwrap();
        assert!(matches!(read_target(&d), Err(HookError::UnsafeTarget(_))));

        let fifo = t.path().join("fifo.json");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: valid NUL-terminated path, plain syscall.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // Must return an error promptly, not block on the FIFO.
        assert!(matches!(
            read_target(&fifo),
            Err(HookError::UnsafeTarget(_))
        ));

        // A symlinked directory is refused as well.
        let linked_dir = t.path().join("linked");
        symlink(t.path(), &linked_dir).unwrap();
        assert!(matches!(
            read_target(&linked_dir.join("real.json")),
            Err(HookError::UnsafeTarget(_))
        ));
    }

    #[test]
    fn oversized_files_are_refused() {
        let t = TempDir::new().unwrap();
        let p = t.path().join("big.json");
        fs::write(&p, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
        assert!(matches!(read_target(&p), Err(HookError::TooLarge)));
    }

    #[test]
    fn atomic_write_keeps_mode_leaves_no_temp_and_replaces() {
        let t = TempDir::new().unwrap();
        let p = t.path().join("s.json");
        assert!(publish(&p, &Existing::Absent, Some(b"one")).unwrap());
        assert_eq!(fs::metadata(&p).unwrap().mode() & 0o777, 0o600);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        let existing = read_target(&p).unwrap();
        assert!(publish(&p, &existing, Some(b"two")).unwrap());
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(fs::metadata(&p).unwrap().mode() & 0o777, 0o644);
        let names: Vec<_> = fs::read_dir(t.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 1, "no temp file left behind");
    }

    #[test]
    fn late_edits_block_creation_replacement_and_deletion() {
        let t = TempDir::new().unwrap();
        let dir = t.path();
        let p = dir.join("s.json");
        let new = stage(dir, b"initial WHY config", 0o600).unwrap();
        fs::write(&p, b"user created config").unwrap();
        assert!(!publish_staged(&p, dir, &Existing::Absent, Some(new)).unwrap());
        assert_eq!(fs::read(&p).unwrap(), b"user created config");

        let existing = read_target(&p).unwrap();
        let staged = stage(dir, b"WHY edit", existing.mode()).unwrap();
        fs::write(&p, b"user saved another edit").unwrap();
        assert!(!publish_staged(&p, dir, &existing, Some(staged)).unwrap());
        assert!(!publish(&p, &existing, None).unwrap());
        assert_eq!(fs::read(&p).unwrap(), b"user saved another edit");
        assert_eq!(fs::read_dir(dir).unwrap().count(), 1);
    }

    #[test]
    fn ensure_dir_creates_one_level_with_mode_0700_and_never_a_home() {
        let t = TempDir::new().unwrap();
        let d = t.path().join(".claude");
        ensure_dir(&d).unwrap();
        assert_eq!(fs::metadata(&d).unwrap().mode() & 0o777, 0o700);
        ensure_dir(&d).unwrap(); // idempotent
        assert!(matches!(
            ensure_dir(&t.path().join("missing-home/.claude")),
            Err(HookError::NoHome)
        ));
    }

    #[test]
    fn the_directory_lock_excludes_a_second_taker() {
        let t = TempDir::new().unwrap();
        let first = lock_dir(t.path(), Duration::from_millis(50)).unwrap();
        assert!(matches!(
            lock_dir(t.path(), Duration::from_millis(50)),
            Err(HookError::Locked)
        ));
        drop(first);
        lock_dir(t.path(), Duration::from_millis(50)).unwrap();
    }
}
