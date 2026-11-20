//! Putting the `why-hook` binary where agent configs can point at it for good.
//!
//! An AppImage mounts itself at a new path on every run (`/tmp/.mount_WHYabc/`),
//! and the bundled `why-hook` sits inside that mount, so a hook config naming
//! it would break the next launch. The installers (WHY-14/15) therefore point
//! at ONE stable copy: `~/.local/share/why/bin/why-hook`. The app is meant to
//! refresh it at start so an upgrade replaces the hook too; nothing calls
//! [`install_bundled_hook`] yet (the installers, WHY-14/15, will).

use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use why_wire::PathEnv;

/// Name of the binary, as Tauri places a sidecar next to the app executable
/// (the `-<target-triple>` suffix is stripped when bundling).
pub const HOOK_BINARY: &str = "why-hook";

/// `<data home>/why/bin/why-hook`: `$XDG_DATA_HOME` if absolute, else
/// `$HOME/.local/share` (also on macOS, deliberately: one documented path for
/// both, easy for users to find and to audit). `None` without a usable home.
pub fn stable_hook_path(env: &PathEnv) -> Option<PathBuf> {
    let abs = |v: &Option<std::ffi::OsString>| {
        v.as_deref()
            .map(Path::new)
            .filter(|p| p.is_absolute())
            .map(Path::to_path_buf)
    };
    let base = abs(&env.data_home).or_else(|| abs(&env.home).map(|h| h.join(".local/share")))?;
    Some(base.join("why/bin").join(HOOK_BINARY))
}

/// Where the bundled sidecar is, given the app executable's path.
pub fn bundled_hook_path(current_exe: &Path) -> Option<PathBuf> {
    Some(current_exe.parent()?.join(HOOK_BINARY))
}

/// Copy `src` to `dest` atomically with mode `0755`.
///
/// Copy to a temp file in the destination directory, then `rename` over `dest`:
/// an agent running the hook at this very moment either still has the old
/// inode (rename never disturbs a running executable) or starts the complete
/// new one; it can never exec a half-written file ("text file busy" or a
/// truncated ELF). A copy that does not exist at `src` is an error, never a
/// stub.
pub fn install_hook_binary(src: &Path, dest: &Path) -> io::Result<()> {
    let dir = dest
        .parent()
        .ok_or_else(|| io::Error::other("destination has no parent directory"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{HOOK_BINARY}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut from = std::fs::File::open(src)?;
        let mut to = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o755)
            .open(&tmp)?;
        io::copy(&mut from, &mut to)?;
        to.sync_all()?;
        drop(to); // close before rename/exec: an open-for-write file is ETXTBSY
        // `mode` above is filtered by the umask; set the final mode explicitly.
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Install the bundled hook to the stable path; returns where it now lives.
pub fn install_bundled_hook(env: &PathEnv) -> io::Result<PathBuf> {
    let missing = |what: &str| io::Error::new(io::ErrorKind::NotFound, what.to_owned());
    let dest = stable_hook_path(env).ok_or_else(|| missing("no usable HOME or XDG_DATA_HOME"))?;
    let exe = std::env::current_exe()?;
    let src = bundled_hook_path(&exe).ok_or_else(|| missing("no directory for the app binary"))?;
    install_hook_binary(&src, &dest)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use tempfile::TempDir;

    use super::*;

    fn env(data_home: Option<&str>, home: Option<&str>) -> PathEnv {
        PathEnv {
            runtime_dir: None,
            data_home: data_home.map(OsString::from),
            home: home.map(OsString::from),
        }
    }

    #[test]
    fn the_stable_path_follows_xdg_rules() {
        assert_eq!(
            stable_hook_path(&env(None, Some("/home/u"))),
            Some("/home/u/.local/share/why/bin/why-hook".into())
        );
        assert_eq!(
            stable_hook_path(&env(Some("/data"), Some("/home/u"))),
            Some("/data/why/bin/why-hook".into())
        );
        // Relative XDG_DATA_HOME is invalid per the spec: ignored.
        assert_eq!(
            stable_hook_path(&env(Some("rel"), Some("/home/u"))),
            Some("/home/u/.local/share/why/bin/why-hook".into())
        );
        assert_eq!(stable_hook_path(&env(None, None)), None);
        assert_eq!(stable_hook_path(&env(None, Some("relative"))), None);
    }

    #[test]
    fn bundled_path_is_next_to_the_executable() {
        assert_eq!(
            bundled_hook_path(Path::new("/opt/WHY/why")),
            Some("/opt/WHY/why-hook".into())
        );
    }

    #[test]
    fn install_copies_atomically_with_mode_0755_and_replaces() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("bundled");
        std::fs::write(&src, b"v1").unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o600)).unwrap();
        let dest = tmp.path().join("share/why/bin/why-hook");

        install_hook_binary(&src, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"v1");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dest), 0o755);

        // Upgrade: replaced in place, no temp file left behind.
        std::fs::write(&src, b"v2").unwrap();
        install_hook_binary(&src, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"v2");
        let names: Vec<_> = std::fs::read_dir(dest.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, [OsString::from("why-hook")]);
    }

    #[test]
    fn a_missing_source_is_an_error_and_leaves_the_old_copy() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("bin/why-hook");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"old").unwrap();
        assert!(install_hook_binary(&tmp.path().join("nope"), &dest).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"old");
        assert_eq!(
            std::fs::read_dir(dest.parent().unwrap()).unwrap().count(),
            1
        );
    }
}
