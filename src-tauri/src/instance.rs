//! Which app instance this process is, and where its data lives.
//!
//! Decision 26: a Tauri dev run (`pnpm dev`, including `--release`) uses a
//! separate data dir and collector socket.
//!
//! - Installed apps are [`Instance::Installed`] and keep Tauri's
//!   `app_data_dir` exactly as v0.1 did (the installed user's `why.db` does
//!   not move);
//! - Tauri dev runs are [`Instance::Dev`] with the port from `WHY_DEV_PORT`
//!   (what `scripts/dev.mjs` serves on), and use `<app_data_dir>-dev`, one dir
//!   shared by every dev instance so registered repositories follow the
//!   developer across worktrees.
//!
//! The path rules themselves live in `why_wire::instance`, shared with the hook.

use std::path::Path;

use why_wire::{Instance, InstancePaths, PathEnv};

/// The port `scripts/dev.mjs` (and `ui/dev-port.ts`) default to.
pub const DEFAULT_DEV_PORT: u16 = 1420;

/// Pick the instance. Pure, so both branches are testable from either build:
/// `tauri_dev` is `tauri::is_dev()` and `dev_port` is `WHY_DEV_PORT`
/// in real use. An unusable port falls back to [`DEFAULT_DEV_PORT`].
pub fn choose_instance(tauri_dev: bool, dev_port: Option<&str>) -> Instance {
    if !tauri_dev {
        return Instance::Installed;
    }
    let port = dev_port
        .and_then(|p| p.trim().parse::<u16>().ok())
        .filter(|&p| p != 0)
        .unwrap_or(DEFAULT_DEV_PORT);
    Instance::Dev { port }
}

/// All paths for `instance`, given Tauri's `app_data_dir` (the installed
/// instance's data dir) and this process's environment.
pub fn instance_paths(
    instance: Instance,
    tauri_app_data_dir: &Path,
    env: &PathEnv,
) -> InstancePaths {
    InstancePaths::from_installed_data_dir(instance, tauri_app_data_dir, env.runtime_dir.as_deref())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn packaged_app_ignores_the_dev_port() {
        assert_eq!(choose_instance(false, None), Instance::Installed);
        // Even with WHY_DEV_PORT set (say, a developer's shell running the
        // installed app), Tauri's mode still selects the installed instance.
        assert_eq!(choose_instance(false, Some("1460")), Instance::Installed);
    }

    #[test]
    fn tauri_dev_uses_its_port_independent_of_cargo_profile() {
        assert_eq!(
            choose_instance(true, Some("1460")),
            Instance::Dev { port: 1460 }
        );
        assert_eq!(
            choose_instance(true, Some(" 1430\n")),
            Instance::Dev { port: 1430 }
        );
        for bad in [None, Some(""), Some("abc"), Some("0"), Some("99999")] {
            assert_eq!(
                choose_instance(true, bad),
                Instance::Dev {
                    port: DEFAULT_DEV_PORT
                },
                "{bad:?}"
            );
        }
    }

    #[test]
    fn installed_instance_keeps_the_v01_data_dir_and_dev_uses_a_sibling() {
        let tauri_dir = PathBuf::from("/home/u/.local/share/io.github.namdang-fdp.why");
        let env = PathEnv::default();
        let installed = instance_paths(Instance::Installed, &tauri_dir, &env);
        assert_eq!(
            installed.data_dir, tauri_dir,
            "the installed user's why.db must not move"
        );
        let dev = instance_paths(Instance::Dev { port: 1460 }, &tauri_dir, &env);
        assert_eq!(
            dev.data_dir,
            PathBuf::from("/home/u/.local/share/io.github.namdang-fdp.why-dev")
        );
    }

    #[test]
    fn identifier_matches_tauri_conf() {
        // `why_wire::APP_IDENTIFIER` is how the hook finds the data dir; it must
        // be the identifier Tauri names `app_data_dir` after.
        let conf =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"))
                .unwrap();
        let conf: serde_json::Value = serde_json::from_str(&conf).unwrap();
        assert_eq!(conf["identifier"].as_str(), Some(why_wire::APP_IDENTIFIER));
    }
}
