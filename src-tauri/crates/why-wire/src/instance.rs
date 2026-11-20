//! Where an app instance keeps its collector socket, spool and allowlist.
//!
//! The app supplies its resolved paths in the hook configuration so hooks can
//! find the collector and allowlist even with a different shell environment.
//!
//! Two kinds of instance must never see each other's data (decision 26): the
//! installed app, and dev builds (`pnpm dev`, one per worktree, told apart by
//! their `WHY_DEV_PORT`).
//!
//! | | Installed | Dev { port } |
//! |---|---|---|
//! | data dir | `<data>/<APP_IDENTIFIER>` (Tauri's `app_data_dir`) | `<installed data dir>-dev` (shared by all dev instances) |
//! | socket | `$XDG_RUNTIME_DIR/why/collector.sock` | `$XDG_RUNTIME_DIR/why/collector-dev-<port>.sock` |
//! | socket, no usable `XDG_RUNTIME_DIR` | `<data dir>/run/collector.sock` | `<data dir>/run/collector-dev-<port>.sock` |
//! | spool | `<data dir>/spool/` | `<data dir>/spool-<port>/` |
//! | allowlist | `<data dir>/hook-allowlist.json` | same, in the dev data dir |
//!
//! Every function here is pure: the environment comes in as a [`PathEnv`]
//! value, so tests can use any `HOME`/`XDG_*` without touching the real ones
//! (and without setting process-wide variables, which races across tests).

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use thiserror::Error;

/// Tauri's bundle identifier (`identifier` in `src-tauri/tauri.conf.json`).
/// Tauri names the app data dir after it, and the hook has no Tauri to ask, so
/// it is repeated here; a test in the app crate fails if the two drift.
pub const APP_IDENTIFIER: &str = "io.github.namdang-fdp.why";

/// The environment variable the hook reads to pick an instance when no
/// `--instance` argument is given (`installed` or `dev:<port>`).
pub const INSTANCE_ENV: &str = "WHY_INSTANCE";

/// Which app instance a path belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Instance {
    /// The user's installed WHY (packaged app).
    Installed,
    /// A `pnpm dev` build serving on `port` (`WHY_DEV_PORT`).
    Dev { port: u16 },
}

/// `Instance` is spelled `installed` or `dev:<port>` in `WHY_INSTANCE` and in
/// `why-hook --instance`. Those strings end up in hook configs, so the grammar
/// is pinned by tests.
impl FromStr for Instance {
    type Err = ParseInstanceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "installed" {
            return Ok(Instance::Installed);
        }
        let port = s
            .strip_prefix("dev:")
            .and_then(|p| p.parse::<u16>().ok())
            // Port 0 means "any port" to the OS; no dev server listens there.
            .filter(|&p| p != 0)
            .ok_or_else(|| ParseInstanceError(s.to_owned()))?;
        Ok(Instance::Dev { port })
    }
}

impl fmt::Display for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instance::Installed => f.write_str("installed"),
            Instance::Dev { port } => write!(f, "dev:{port}"),
        }
    }
}

/// The text was neither `installed` nor `dev:<port>` (1..=65535).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0:?} is not an instance (expected `installed` or `dev:<port>`)")]
pub struct ParseInstanceError(pub String);

/// The environment variables that decide the paths, captured as plain values.
///
/// Build it with [`PathEnv::from_process`] in real code and by hand in tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathEnv {
    /// `XDG_RUNTIME_DIR`: a per-user, `0700`, tmpfs directory that the login
    /// session creates and removes. The right place for a socket.
    pub runtime_dir: Option<OsString>,
    /// `XDG_DATA_HOME`.
    pub data_home: Option<OsString>,
    /// `HOME`.
    pub home: Option<OsString>,
}

impl PathEnv {
    /// Read the three variables from this process's environment.
    pub fn from_process() -> Self {
        Self {
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR"),
            data_home: std::env::var_os("XDG_DATA_HOME"),
            home: std::env::var_os("HOME"),
        }
    }
}

/// No data directory could be derived: neither an absolute `XDG_DATA_HOME`
/// nor an absolute `HOME` is set.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("cannot locate the WHY data directory: neither XDG_DATA_HOME nor HOME is an absolute path")]
pub struct PathError;

/// The XDG Base Directory spec says relative paths in `XDG_*` variables are
/// invalid and must be ignored; an empty value counts as unset.
fn absolute(var: Option<&OsStr>) -> Option<&Path> {
    var.map(Path::new).filter(|p| p.is_absolute())
}

/// The installed app's data dir, computed the way Tauri's `app_data_dir` does
/// (it uses the `dirs` crate): `$XDG_DATA_HOME/<id>` if that is absolute, else
/// `$HOME/.local/share/<id>` (on macOS `$HOME/Library/Application Support/<id>`).
///
/// Fallback for manual hook invocations without configured paths.
pub fn installed_data_dir(env: &PathEnv) -> Result<PathBuf, PathError> {
    let base = if cfg!(target_os = "macos") {
        absolute(env.home.as_deref()).map(|h| h.join("Library/Application Support"))
    } else {
        absolute(env.data_home.as_deref())
            .map(Path::to_path_buf)
            .or_else(|| absolute(env.home.as_deref()).map(|h| h.join(".local/share")))
    };
    base.map(|b| b.join(APP_IDENTIFIER)).ok_or(PathError)
}

/// Every path an instance uses. The app resolves them from Tauri's data dir
/// and passes them to the hook using [`InstancePaths::hook_args`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstancePaths {
    pub instance: Instance,
    /// Where this instance keeps its databases (`why.db`, `events.db`).
    pub data_dir: PathBuf,
    /// The collector's unix socket.
    pub socket_path: PathBuf,
    /// Where the hook appends envelopes it could not deliver (WHY-13).
    pub spool_dir: PathBuf,
    /// The registered-repository allowlist the app writes and the hook reads
    /// (decision 22; file format is WHY-13's).
    pub allowlist_path: PathBuf,
}

impl InstancePaths {
    /// Compute paths for a manual hook invocation without configured paths.
    pub fn resolve(instance: Instance, env: &PathEnv) -> Result<Self, PathError> {
        let installed = installed_data_dir(env)?;
        Ok(Self::from_installed_data_dir(
            instance,
            &installed,
            env.runtime_dir.as_deref(),
        ))
    }

    /// Compute everything from the installed app's data dir (what the app does,
    /// passing Tauri's `app_data_dir`; and what tests do, passing a temp dir).
    /// `runtime_dir` is `XDG_RUNTIME_DIR` as read; unusable values (unset,
    /// empty, relative) select the fallback under the data dir.
    pub fn from_installed_data_dir(
        instance: Instance,
        installed_data_dir: &Path,
        runtime_dir: Option<&OsStr>,
    ) -> Self {
        let data_dir = match instance {
            Instance::Installed => installed_data_dir.to_path_buf(),
            Instance::Dev { .. } => dev_data_dir(installed_data_dir),
        };
        let socket_name = match instance {
            Instance::Installed => "collector.sock".to_owned(),
            Instance::Dev { port } => format!("collector-dev-{port}.sock"),
        };
        // Note for WHY-13: a unix socket path is limited to ~108 bytes on Linux
        // (104 on macOS); a deep `HOME` can exceed that in the fallback case,
        // and `bind` then fails. The collector must report that clearly.
        let socket_dir = match absolute(runtime_dir) {
            Some(rt) => rt.join("why"),
            // No runtime dir (some containers, `su`, cron, macOS): a per-user
            // directory under the data dir. The collector creates it `0700`.
            None => data_dir.join("run"),
        };
        Self::from_resolved_paths(instance, data_dir, socket_dir.join(socket_name))
    }

    /// Arguments to persist in the harness's hook config, alongside
    /// `--harness`. Keep paths as individual arguments when rendering the command.
    pub fn hook_args(&self) -> [OsString; 6] {
        [
            "--instance".into(),
            self.instance.to_string().into(),
            "--data-dir".into(),
            self.data_dir.as_os_str().to_owned(),
            "--socket-path".into(),
            self.socket_path.as_os_str().to_owned(),
        ]
    }

    /// Rebuild the collector paths from arguments supplied by the app.
    /// Configured paths must be absolute so the hook's cwd cannot change them.
    pub fn from_hook_paths(
        instance: Instance,
        data_dir: PathBuf,
        socket_path: PathBuf,
    ) -> Option<Self> {
        if !data_dir.is_absolute() || !socket_path.is_absolute() {
            return None;
        }
        Some(Self::from_resolved_paths(instance, data_dir, socket_path))
    }

    fn from_resolved_paths(instance: Instance, data_dir: PathBuf, socket_path: PathBuf) -> Self {
        let spool_dir = match instance {
            Instance::Installed => data_dir.join("spool"),
            // Dev instances share one data dir but must not drain each other's
            // spool, so each port gets its own.
            Instance::Dev { port } => data_dir.join(format!("spool-{port}")),
        };
        Self {
            instance,
            socket_path,
            spool_dir,
            allowlist_path: data_dir.join("hook-allowlist.json"),
            data_dir,
        }
    }
}

/// `<installed>-dev`: a sibling of the installed data dir, so a dev build can
/// never open (or migrate, or corrupt) the installed user's databases.
fn dev_data_dir(installed: &Path) -> PathBuf {
    match installed.file_name() {
        Some(name) => {
            let mut dev = name.to_owned();
            dev.push("-dev");
            installed.with_file_name(dev)
        }
        // `/` or `..`: no final name to suffix; nest instead (never the same dir).
        None => installed.join("dev"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(rt: Option<&str>, data_home: Option<&str>, home: Option<&str>) -> PathEnv {
        PathEnv {
            runtime_dir: rt.map(OsString::from),
            data_home: data_home.map(OsString::from),
            home: home.map(OsString::from),
        }
    }

    fn all_paths(p: &InstancePaths) -> [&Path; 4] {
        [&p.data_dir, &p.socket_path, &p.spool_dir, &p.allowlist_path]
    }

    #[test]
    fn instance_grammar_is_pinned() {
        assert_eq!("installed".parse(), Ok(Instance::Installed));
        assert_eq!("dev:1460".parse(), Ok(Instance::Dev { port: 1460 }));
        assert_eq!("dev:65535".parse(), Ok(Instance::Dev { port: 65535 }));
        for bad in [
            "",
            "Installed",
            "dev",
            "dev:",
            "dev:0",
            "dev:abc",
            "dev:70000",
            "dev: 1",
            "prod",
        ] {
            assert!(bad.parse::<Instance>().is_err(), "{bad:?} must be rejected");
        }
        for i in [Instance::Installed, Instance::Dev { port: 1420 }] {
            assert_eq!(i.to_string().parse(), Ok(i));
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn installed_paths_with_a_runtime_dir() {
        let p = InstancePaths::resolve(
            Instance::Installed,
            &env(Some("/run/user/1000"), None, Some("/home/u")),
        )
        .unwrap();
        let data = Path::new("/home/u/.local/share/io.github.namdang-fdp.why");
        assert_eq!(p.data_dir, data);
        assert_eq!(
            p.socket_path,
            Path::new("/run/user/1000/why/collector.sock")
        );
        assert_eq!(p.spool_dir, data.join("spool"));
        assert_eq!(p.allowlist_path, data.join("hook-allowlist.json"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn xdg_data_home_wins_over_home_only_when_absolute() {
        let abs = installed_data_dir(&env(None, Some("/data"), Some("/home/u"))).unwrap();
        assert_eq!(abs, Path::new("/data/io.github.namdang-fdp.why"));
        let rel = installed_data_dir(&env(None, Some("rel/data"), Some("/home/u"))).unwrap();
        assert_eq!(
            rel,
            Path::new("/home/u/.local/share/io.github.namdang-fdp.why")
        );
        let empty = installed_data_dir(&env(None, Some(""), Some("/home/u"))).unwrap();
        assert_eq!(empty, rel);
    }

    #[test]
    fn no_home_at_all_is_an_error_not_a_relative_path() {
        assert_eq!(
            InstancePaths::resolve(Instance::Installed, &env(None, None, None)),
            Err(PathError)
        );
        assert_eq!(
            installed_data_dir(&env(None, None, Some("relative/home"))),
            Err(PathError)
        );
    }

    #[test]
    fn unusable_runtime_dir_falls_back_under_the_data_dir() {
        let data = Path::new("/d/why");
        // Unset, empty, relative: all three are "no runtime dir" per the XDG spec.
        for rt in [None, Some(""), Some("run/user/1000")] {
            let p = InstancePaths::from_installed_data_dir(
                Instance::Installed,
                data,
                rt.map(OsStr::new),
            );
            assert_eq!(
                p.socket_path,
                Path::new("/d/why/run/collector.sock"),
                "rt = {rt:?}"
            );
            let dev = InstancePaths::from_installed_data_dir(
                Instance::Dev { port: 1460 },
                data,
                rt.map(OsStr::new),
            );
            assert_eq!(
                dev.socket_path,
                Path::new("/d/why-dev/run/collector-dev-1460.sock")
            );
        }
    }

    #[test]
    fn paths_with_spaces_are_kept_verbatim() {
        let p = InstancePaths::from_installed_data_dir(
            Instance::Dev { port: 1460 },
            Path::new("/home/a user/My Data/why"),
            Some(OsStr::new("/run/user/my runtime")),
        );
        assert_eq!(p.data_dir, Path::new("/home/a user/My Data/why-dev"));
        assert_eq!(
            p.socket_path,
            Path::new("/run/user/my runtime/why/collector-dev-1460.sock")
        );
        assert_eq!(
            p.spool_dir,
            Path::new("/home/a user/My Data/why-dev/spool-1460")
        );
    }

    #[test]
    fn dev_and_installed_never_collide() {
        let data = Path::new("/d/why");
        for rt in [None, Some(OsStr::new("/run/user/1000"))] {
            let instances = [
                Instance::Installed,
                Instance::Dev { port: 1420 },
                Instance::Dev { port: 1460 },
            ];
            let paths: Vec<InstancePaths> = instances
                .iter()
                .map(|&i| InstancePaths::from_installed_data_dir(i, data, rt))
                .collect();
            let installed = &paths[0];
            for dev in &paths[1..] {
                // A dev instance shares NOTHING with the installed one.
                for a in all_paths(installed) {
                    for b in all_paths(dev) {
                        assert_ne!(a, b);
                        assert!(
                            !b.starts_with(&installed.data_dir),
                            "{b:?} is inside the installed data dir"
                        );
                    }
                }
            }
            // Two dev instances share the data dir and the allowlist (they share
            // the repository registry), but never a socket or a spool.
            assert_eq!(paths[1].data_dir, paths[2].data_dir);
            assert_ne!(paths[1].socket_path, paths[2].socket_path);
            assert_ne!(paths[1].spool_dir, paths[2].spool_dir);
        }
    }

    #[test]
    fn dev_data_dir_is_a_sibling_even_with_a_trailing_slash() {
        assert_eq!(dev_data_dir(Path::new("/d/why/")), Path::new("/d/why-dev"));
        assert_ne!(dev_data_dir(Path::new("/")), Path::new("/"));
    }
}
