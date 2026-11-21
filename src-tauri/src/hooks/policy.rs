//! Managed (administrator) policy that forbids user-level hooks.
//!
//! WHY writes user-level hooks. If an administrator has said "only managed
//! hooks may run", writing ours would be at best useless, at worst an attempt
//! to route around a rule someone set on purpose. So we READ the documented
//! managed locations (never write them, never read anything else there) and
//! report `disabledByPolicy`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_POLICY_BYTES: u64 = 1024 * 1024;

fn read_small(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut text = String::new();
    file.take(MAX_POLICY_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

/// Claude Code: a managed settings file with `allowManagedHooksOnly` (blocks
/// user/project/local hooks) or `disableAllHooks` set to `true`. `files` are
/// the `managed-settings.json` candidates, `dropin_dirs` their `*.d`
/// directories. Returns the file that says so.
pub fn claude_policy(files: &[PathBuf], dropin_dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = files.to_vec();
    for dir in dropin_dirs {
        let mut found: Vec<PathBuf> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        found.sort();
        candidates.extend(found);
    }
    candidates.into_iter().find(|path| {
        read_small(path)
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .is_some_and(|v| {
                ["allowManagedHooksOnly", "disableAllHooks"]
                    .iter()
                    .any(|k| v.get(k).and_then(|x| x.as_bool()) == Some(true))
            })
    })
}

/// Codex: `requirements.toml` with `allow_managed_hooks_only = true` or
/// `[features] hooks = false`. Invalid files do not count as a policy block.
pub fn codex_policy(requirements: &[PathBuf]) -> Option<PathBuf> {
    requirements
        .iter()
        .find(|path| read_small(path).is_some_and(|t| requirements_forbid_user_hooks(&t)))
        .cloned()
}

pub fn requirements_forbid_user_hooks(text: &str) -> bool {
    let Ok(policy) = text.parse::<toml::Table>() else {
        return false;
    };
    policy
        .get("allow_managed_hooks_only")
        .and_then(toml::Value::as_bool)
        == Some(true)
        || policy
            .get("features")
            .and_then(|features| features.get("hooks"))
            .and_then(toml::Value::as_bool)
            == Some(false)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn claude_managed_settings_are_detected_in_files_and_dropins() {
        let t = TempDir::new().unwrap();
        let main = t.path().join("managed-settings.json");
        let dropin = t.path().join("managed-settings.d");
        fs::create_dir(&dropin).unwrap();
        assert_eq!(
            claude_policy(std::slice::from_ref(&main), std::slice::from_ref(&dropin)),
            None
        );

        fs::write(&main, r#"{"allowManagedHooksOnly": false, "x": 1}"#).unwrap();
        assert_eq!(
            claude_policy(std::slice::from_ref(&main), std::slice::from_ref(&dropin)),
            None
        );

        fs::write(&main, r#"{"allowManagedHooksOnly": true}"#).unwrap();
        assert_eq!(
            claude_policy(std::slice::from_ref(&main), &[]),
            Some(main.clone())
        );

        fs::write(&main, "{}").unwrap();
        let d = dropin.join("10-hooks.json");
        fs::write(&d, r#"{"disableAllHooks": true}"#).unwrap();
        fs::write(dropin.join("README.txt"), "allowManagedHooksOnly").unwrap();
        assert_eq!(claude_policy(&[main], &[dropin]), Some(d));
        // Unreadable / invalid policy files never count as a block.
        assert_eq!(claude_policy(&[t.path().join("nope.json")], &[]), None);
    }

    #[test]
    fn codex_requirements_parse_equivalent_toml_forms() {
        assert!(requirements_forbid_user_hooks(
            "allow_managed_hooks_only = true\n"
        ));
        assert!(requirements_forbid_user_hooks(
            "# c\n[features]\nhooks = false # off\n"
        ));
        assert!(requirements_forbid_user_hooks(
            "features = { hooks = false }"
        ));
        assert!(requirements_forbid_user_hooks(
            r#"features."hooks" = false"#
        ));
        assert!(!requirements_forbid_user_hooks(
            "features = { hooks = 'false' }"
        ));
        assert!(!requirements_forbid_user_hooks(
            "[features]\nhooks = true\n"
        ));
        assert!(!requirements_forbid_user_hooks(
            "allow_managed_hooks_only = false\n"
        ));
        // `hooks = false` outside [features] is some other table's key.
        assert!(!requirements_forbid_user_hooks("[other]\nhooks = false\n"));
        assert!(!requirements_forbid_user_hooks(""));
    }
}
