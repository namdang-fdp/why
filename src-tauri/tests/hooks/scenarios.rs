//! The hostile and edge scenarios from the WHY-51 brief, one test each.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::process::Command;

use why_lib::contracts::{
    ApplyResult, BinaryStatus, ChangeOutcome, HookState, PreviewResult, RefusalReason,
};
use why_lib::hooks::{Target, config_file::MAX_CONFIG_BYTES, sha256_hex};

use crate::common::*;

fn applied(r: ApplyResult) -> ChangeOutcome {
    match r {
        ApplyResult::Applied { outcome, .. } => outcome,
        other => panic!("expected applied, got {other:?}"),
    }
}

fn refused_install(fx: &Fx, h: why_lib::contracts::HookHarness) -> (RefusalReason, String) {
    match fx.ctx.preview_install_result(h.into()).unwrap() {
        PreviewResult::Refused { reason, message } => (reason, message),
        PreviewResult::Ready { preview } => {
            // The preview may be fine and the apply refused (read-only, lock).
            match fx
                .ctx
                .install_result(h.into(), &preview.fingerprint)
                .unwrap()
            {
                ApplyResult::Refused { reason, message } => (reason, message),
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
    }
}

/// Install the harness and return the written commands, one per event.
fn commands(bytes: &[u8]) -> Vec<String> {
    let v = json(bytes);
    v["hooks"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|groups| groups.as_array().unwrap())
        .flat_map(|g| g["hooks"].as_array().unwrap())
        .filter_map(|h| h["command"].as_str().map(str::to_owned))
        .collect()
}

// ── merging ──

#[test]
fn existing_user_hooks_on_the_same_events_stay_untouched_and_in_order() {
    let fx = Fx::new();
    let original = r#"{
  "model": "x",
  "hooks": {
    "PreToolUse": [
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "mine-1"}, {"type": "command", "command": "mine-2"}]},
      {"hooks": [{"type": "command", "command": "mine-3"}]}
    ],
    "Notification": [{"hooks": [{"type": "command", "command": "ping"}]}]
  }
}
"#;
    fx.write(&fx.claude_file(), original);
    assert_eq!(applied(install(&fx.ctx, CLAUDE)), ChangeOutcome::ModifyFile);
    let v = json(&fx.read(&fx.claude_file()).unwrap());
    let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 3);
    assert_eq!(pre[0]["matcher"], "Bash");
    assert_eq!(pre[0]["hooks"][0]["command"], "mine-1");
    assert_eq!(pre[0]["hooks"][1]["command"], "mine-2");
    assert_eq!(pre[1]["hooks"][0]["command"], "mine-3");
    assert!(
        pre[2]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("why-hook")
    );
    assert_eq!(v["hooks"]["Notification"][0]["hooks"][0]["command"], "ping");
    assert_eq!(v["model"], "x");
    // All ten Claude events are there.
    assert_eq!(
        commands(&fx.read(&fx.claude_file()).unwrap()).len(),
        10 + 3 + 1
    );
}

#[test]
fn odd_formatting_and_unicode_show_up_as_additions_only() {
    let fx = Fx::new();
    let original = "{\n\t\"nom\"  :  \"caf\\u00e9 ☕ 日本\" ,\n\t\"hooks\" : {}\n}";
    fx.write(&fx.claude_file(), original);
    let preview = ready(fx.ctx.preview_install_result(CLAUDE.into()).unwrap());
    // Nothing the user wrote is removed or rewritten: the diff's only `-` lines
    // are the hunk header and the one-line change around the closing brace.
    let after = preview.after.clone().unwrap();
    assert!(
        after.contains("\"nom\"  :  \"caf\\u00e9 ☕ 日本\" ,"),
        "{after}"
    );
    assert!(after.starts_with("{\n\t\"nom\""));
    assert!(!preview.diff.is_empty());
    assert_eq!(preview.before.as_deref(), Some(original));
    // And the preview equals what a real install writes.
    install(&fx.ctx, CLAUDE);
    assert_eq!(
        String::from_utf8(fx.read(&fx.claude_file()).unwrap()).unwrap(),
        after
    );
}

#[test]
fn trailing_newline_or_the_lack_of_it_is_preserved() {
    for original in [
        "{\"a\":1}\n",
        "{\"a\":1}",
        "{\"a\":1}\n\n",
        "{\r\n  \"a\": 1\r\n}\r\n",
    ] {
        let fx = Fx::new();
        fx.write(&fx.claude_file(), original);
        install(&fx.ctx, CLAUDE);
        let after = String::from_utf8(fx.read(&fx.claude_file()).unwrap()).unwrap();
        assert!(
            after.ends_with(&original[original.trim_end().len()..]),
            "{original:?}"
        );
        uninstall(&fx.ctx, CLAUDE);
        assert_eq!(fx.read(&fx.claude_file()).unwrap(), original.as_bytes());
    }
}

// ── preview / fingerprint ──

#[test]
fn a_file_changed_between_preview_and_install_is_refused_with_a_fresh_preview() {
    let fx = Fx::new();
    fx.write(&fx.claude_file(), "{\"a\":1}\n");
    let preview = ready(fx.ctx.preview_install_result(CLAUDE.into()).unwrap());
    // The user (or the agent) edits the file after the preview.
    fx.write(&fx.claude_file(), "{\"a\":2}\n");
    match fx
        .ctx
        .install_result(CLAUDE.into(), &preview.fingerprint)
        .unwrap()
    {
        ApplyResult::Stale { preview: fresh } => {
            assert_ne!(fresh.fingerprint, preview.fingerprint);
            assert!(fresh.after.unwrap().contains("\"a\":2"));
        }
        other => panic!("expected stale, got {other:?}"),
    }
    assert_eq!(
        fx.read(&fx.claude_file()).unwrap(),
        b"{\"a\":2}\n",
        "nothing written"
    );
    // The same for a file that appeared after an `absent` preview.
    let fx = Fx::new();
    let preview = ready(fx.ctx.preview_install_result(CODEX.into()).unwrap());
    assert_eq!(preview.fingerprint, "absent");
    fx.write(&fx.codex_file(), "{}");
    assert!(matches!(
        fx.ctx.install_result(CODEX.into(), "absent").unwrap(),
        ApplyResult::Stale { .. }
    ));
    assert_eq!(fx.read(&fx.codex_file()).unwrap(), b"{}");
}

#[test]
fn a_preview_writes_nothing_not_even_the_directory_or_the_binary() {
    let fx = Fx::new();
    let before: Vec<_> = fs::read_dir(&fx.home).unwrap().flatten().collect();
    ready(fx.ctx.preview_install_result(CLAUDE.into()).unwrap());
    ready(fx.ctx.preview_uninstall_result(CLAUDE.into()).unwrap());
    assert_eq!(
        fs::read_dir(&fx.home).unwrap().flatten().count(),
        before.len()
    );
    assert!(!fx.home.join(".local").exists());
}

#[test]
fn install_twice_is_idempotent_and_the_second_preview_is_empty() {
    let fx = Fx::new();
    fx.write(&fx.claude_file(), "{\n  \"a\": 1\n}\n");
    install(&fx.ctx, CLAUDE);
    let once = fx.read(&fx.claude_file()).unwrap();
    let second = ready(fx.ctx.preview_install_result(CLAUDE.into()).unwrap());
    assert_eq!(second.outcome, ChangeOutcome::NoChange);
    assert!(second.diff.is_empty());
    assert_eq!(applied(install(&fx.ctx, CLAUDE)), ChangeOutcome::NoChange);
    assert_eq!(fx.read(&fx.claude_file()).unwrap(), once);
    // The idempotent second install must not have lost the original.
    uninstall(&fx.ctx, CLAUDE);
    assert_eq!(fx.read(&fx.claude_file()).unwrap(), b"{\n  \"a\": 1\n}\n");
}

// ── restore ──

#[test]
fn uninstall_restores_the_original_bytes_exactly() {
    let fx = Fx::new();
    // Deliberately ugly: odd spacing, escapes, duplicate-looking keys, no final newline.
    let original = "{ \"b\" :1,\"a\":\"\\u00e9\",   \"hooks\":{\"Stop\":[ {\"hooks\":[{\"type\":\"command\",\"command\":\"mine\"}]} ]} }";
    fx.write(&fx.claude_file(), original);
    fs::set_permissions(fx.claude_file(), fs::Permissions::from_mode(0o640)).unwrap();
    install(&fx.ctx, CLAUDE);
    assert_ne!(fx.read(&fx.claude_file()).unwrap(), original.as_bytes());
    let preview = ready(fx.ctx.preview_uninstall_result(CLAUDE.into()).unwrap());
    assert_eq!(preview.outcome, ChangeOutcome::RestoreOriginal);
    assert_eq!(
        applied(uninstall(&fx.ctx, CLAUDE)),
        ChangeOutcome::RestoreOriginal
    );
    assert_eq!(fx.read(&fx.claude_file()).unwrap(), original.as_bytes());
    assert_eq!(
        fs::metadata(fx.claude_file()).unwrap().mode() & 0o777,
        0o640
    );
}

#[test]
fn a_file_why_created_is_deleted_again_and_its_directory_kept() {
    let fx = Fx::new();
    assert_eq!(applied(install(&fx.ctx, CODEX)), ChangeOutcome::CreateFile);
    let dir = fx.home.join(".codex");
    assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(fx.codex_file()).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        applied(uninstall(&fx.ctx, CODEX)),
        ChangeOutcome::DeleteFile
    );
    assert!(!fx.codex_file().exists());
    assert!(
        dir.is_dir(),
        "the directory may hold other things; it stays"
    );
}

#[test]
fn uninstall_after_the_user_edited_removes_only_our_entries() {
    let fx = Fx::new();
    fx.write(&fx.claude_file(), "{\"a\": 1}\n");
    install(&fx.ctx, CLAUDE);
    // The user adds a setting and a hook of their own afterwards.
    let mut v = json(&fx.read(&fx.claude_file()).unwrap());
    v["b"] = serde_json::json!(2);
    v["hooks"]["Stop"].as_array_mut().unwrap().insert(
        0,
        serde_json::json!({"hooks": [{"type": "command", "command": "later-mine"}]}),
    );
    fs::write(fx.claude_file(), serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let preview = ready(fx.ctx.preview_uninstall_result(CLAUDE.into()).unwrap());
    assert_eq!(preview.outcome, ChangeOutcome::RemoveEntries);
    assert_eq!(
        applied(uninstall(&fx.ctx, CLAUDE)),
        ChangeOutcome::RemoveEntries
    );
    let after = json(&fx.read(&fx.claude_file()).unwrap());
    assert_eq!(after["a"], 1);
    assert_eq!(after["b"], 2);
    assert_eq!(
        after["hooks"]["Stop"][0]["hooks"][0]["command"],
        "later-mine"
    );
    assert_eq!(
        after["hooks"].as_object().unwrap().len(),
        1,
        "only Stop is left"
    );
    assert!(
        commands(&fx.read(&fx.claude_file()).unwrap())
            .iter()
            .all(|c| !c.contains("why-hook"))
    );
}

#[test]
fn uninstall_when_why_never_installed_changes_nothing() {
    let fx = Fx::new();
    // No file at all.
    assert_eq!(applied(uninstall(&fx.ctx, CLAUDE)), ChangeOutcome::NoChange);
    assert!(!fx.claude_file().exists());
    assert!(!fx.home.join(".claude").exists());
    // A file with only the user's hooks.
    let original =
        "{\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"mine\"}]}]}}";
    fx.write(&fx.claude_file(), original);
    assert_eq!(applied(uninstall(&fx.ctx, CLAUDE)), ChangeOutcome::NoChange);
    assert_eq!(fx.read(&fx.claude_file()).unwrap(), original.as_bytes());
}

#[test]
fn dev_and_installed_instances_install_side_by_side_and_remove_independently() {
    let fx = Fx::new();
    let dev = fx.dev(1520);
    let original = "{\n  \"a\": 1\n}\n";
    fx.write(&fx.claude_file(), original);

    install(&fx.ctx, CLAUDE);
    install(&dev, CLAUDE);
    let both = commands(&fx.read(&fx.claude_file()).unwrap());
    assert_eq!(
        both.iter().filter(|c| c.contains("'installed'")).count(),
        10
    );
    assert_eq!(both.iter().filter(|c| c.contains("'dev:1520'")).count(), 10);
    // Each sees itself as installed; neither sees the other's entries as its own.
    for ctx in [&fx.ctx, &dev] {
        assert_eq!(ctx.status(Target::ClaudeCode).state, HookState::Installed);
    }
    // Installing again from either is a no-op.
    assert_eq!(applied(install(&dev, CLAUDE)), ChangeOutcome::NoChange);

    // Dev uninstalls first: the file is not what the dev instance wrote last?
    // It is (dev wrote last), so it gets the file as it was before dev ran.
    uninstall(&dev, CLAUDE);
    let rest = commands(&fx.read(&fx.claude_file()).unwrap());
    assert!(rest.iter().all(|c| c.contains("'installed'")) && rest.len() == 10);
    assert_eq!(
        dev.status(Target::ClaudeCode).state,
        HookState::NotInstalled
    );
    assert_eq!(
        fx.ctx.status(Target::ClaudeCode).state,
        HookState::Installed
    );
    uninstall(&fx.ctx, CLAUDE);
    assert_eq!(fx.read(&fx.claude_file()).unwrap(), original.as_bytes());

    // The other order: installed removes first, while the file holds dev's entries
    // written after it: only its own entries go.
    install(&fx.ctx, CLAUDE);
    install(&dev, CLAUDE);
    uninstall(&fx.ctx, CLAUDE);
    let rest = commands(&fx.read(&fx.claude_file()).unwrap());
    assert!(rest.iter().all(|c| c.contains("'dev:1520'")) && rest.len() == 10);
    uninstall(&dev, CLAUDE);
    let end = json(&fx.read(&fx.claude_file()).unwrap());
    assert_eq!(end, serde_json::json!({"a": 1}));
}

#[test]
fn dev_instances_in_the_same_data_directory_restore_independently() {
    let fx = Fx::new();
    let a = fx.dev(1520);
    let b = fx.dev(1521);
    assert_eq!(a.paths.data_dir, b.paths.data_dir);
    let path = fx.claude_file();
    let original = "{\n  \"name\" : \"test\"\n}\n";
    fx.write(&path, original);

    install(&a, CLAUDE);
    let a_only = fs::read(&path).unwrap();
    install(&b, CLAUDE);
    assert_eq!(
        applied(uninstall(&b, CLAUDE)),
        ChangeOutcome::RestoreOriginal
    );
    assert_eq!(fs::read(&path).unwrap(), a_only);
    assert_eq!(
        applied(uninstall(&a, CLAUDE)),
        ChangeOutcome::RestoreOriginal
    );
    assert_eq!(fs::read(&path).unwrap(), original.as_bytes());

    install(&a, CLAUDE);
    install(&b, CLAUDE);
    assert_eq!(applied(uninstall(&a, CLAUDE)), ChangeOutcome::RemoveEntries);
    assert_eq!(a.status(Target::ClaudeCode).state, HookState::NotInstalled);
    assert_eq!(b.status(Target::ClaudeCode).state, HookState::Installed);
    uninstall(&b, CLAUDE);
    assert_eq!(
        json(&fs::read(&path).unwrap()),
        serde_json::json!({"name": "test"})
    );
}

#[test]
fn a_restore_store_failure_prevents_config_creation_and_replacement() {
    let fx = Fx::new();
    fs::create_dir(&fx.ctx.paths.data_dir).unwrap();
    fs::write(fx.ctx.paths.data_dir.join("hook-installs"), b"blocked").unwrap();
    let target = Target::ClaudeCode;
    let path = fx.claude_file();

    let preview = fx.ctx.preview_install(target).unwrap();
    assert!(fx.ctx.install_result(target, &preview.fingerprint).is_err());
    assert!(!path.exists());

    fx.write(&path, "{ \"model\" : \"keep\" }\n");
    let original = fs::read(&path).unwrap();
    let preview = fx.ctx.preview_install(target).unwrap();
    assert!(fx.ctx.install_result(target, &preview.fingerprint).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
}

// ── refusals ──

#[test]
fn invalid_shapes_are_refused_and_the_file_is_never_repaired() {
    for (text, label) in [
        ("{ not json", "invalid"),
        ("", "empty"),
        ("[1, 2]", "root is an array"),
        ("{\"hooks\": \"nope\"}", "hooks is a string"),
        ("{\"hooks\": {\"Stop\": 5}}", "event is a number"),
        ("// comment\n{}", "comment"),
    ] {
        let fx = Fx::new();
        fx.write(&fx.claude_file(), text);
        let (reason, _) = refused_install(&fx, CLAUDE);
        assert_eq!(reason, RefusalReason::ConfigInvalid, "{label}");
        assert_eq!(
            fx.read(&fx.claude_file()).unwrap(),
            text.as_bytes(),
            "{label}"
        );
        assert!(
            matches!(
                fx.ctx.status(Target::ClaudeCode).state,
                HookState::ConfigInvalid { .. }
            ),
            "{label}"
        );
        assert!(
            !fx.home.join(".local").exists(),
            "{label}: no binary copied for a refused config"
        );
    }
}

#[test]
fn a_symlinked_config_is_refused_and_the_link_target_untouched() {
    let fx = Fx::new();
    let real = fx.tmp.path().join("dotfiles/settings.json");
    fx.write(&real, "{\"a\":1}\n");
    fs::create_dir_all(fx.home.join(".claude")).unwrap();
    symlink(&real, fx.claude_file()).unwrap();
    let (reason, msg) = refused_install(&fx, CLAUDE);
    assert_eq!(reason, RefusalReason::UnsafeTarget);
    assert!(msg.contains("symlink"), "{msg}");
    assert_eq!(fx.read(&real).unwrap(), b"{\"a\":1}\n");
    assert!(
        fs::symlink_metadata(fx.claude_file())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // A symlinked config DIRECTORY is refused too (never follow a link out of the home).
    let fx = Fx::new();
    let elsewhere = fx.tmp.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    symlink(&elsewhere, fx.home.join(".codex")).unwrap();
    assert_eq!(refused_install(&fx, CODEX).0, RefusalReason::UnsafeTarget);
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn a_fifo_or_a_directory_as_target_is_refused_without_hanging() {
    let fx = Fx::new();
    fs::create_dir_all(fx.home.join(".claude")).unwrap();
    let c = std::ffi::CString::new(fx.claude_file().to_str().unwrap()).unwrap();
    // SAFETY: valid NUL-terminated path, plain syscall.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert_eq!(refused_install(&fx, CLAUDE).0, RefusalReason::UnsafeTarget);

    fs::create_dir_all(fx.codex_file()).unwrap(); // hooks.json is a directory
    assert_eq!(refused_install(&fx, CODEX).0, RefusalReason::UnsafeTarget);
}

#[test]
fn a_read_only_file_is_respected() {
    let fx = Fx::new();
    fx.write(&fx.claude_file(), "{}\n");
    fs::set_permissions(fx.claude_file(), fs::Permissions::from_mode(0o444)).unwrap();
    assert_eq!(refused_install(&fx, CLAUDE).0, RefusalReason::ReadOnly);
    assert_eq!(fx.read(&fx.claude_file()).unwrap(), b"{}\n");
}

#[test]
fn a_concurrent_writer_holding_the_directory_lock_blocks_the_write() {
    let fx = Fx::new();
    fx.write(&fx.claude_file(), "{}\n");
    let preview = ready(fx.ctx.preview_install_result(CLAUDE.into()).unwrap());
    // Another process (here: another open file description) holds the lock.
    let holder = fs::File::open(fx.home.join(".claude")).unwrap();
    holder.lock().unwrap();
    match fx
        .ctx
        .install_result(CLAUDE.into(), &preview.fingerprint)
        .unwrap()
    {
        ApplyResult::Refused { reason, .. } => assert_eq!(reason, RefusalReason::Locked),
        other => panic!("expected locked, got {other:?}"),
    }
    assert_eq!(fx.read(&fx.claude_file()).unwrap(), b"{}\n");
    drop(holder);
    assert_eq!(
        applied(
            fx.ctx
                .install_result(CLAUDE.into(), &preview.fingerprint)
                .unwrap()
        ),
        ChangeOutcome::ModifyFile
    );
}

#[test]
fn a_missing_home_is_a_clear_refusal() {
    let mut fx = Fx::new();
    fx.ctx.env.paths.home = Some(fx.tmp.path().join("no-such-home").into_os_string());
    // Preview works on "no file"; the apply cannot create `.claude` without a home.
    let (reason, _) = refused_install(&fx, CLAUDE);
    assert_eq!(reason, RefusalReason::NoHome);
    assert!(
        !fx.tmp.path().join("no-such-home").exists(),
        "a home is never created"
    );
    // No HOME at all.
    fx.ctx.env.paths.home = None;
    assert_eq!(refused_install(&fx, CODEX).0, RefusalReason::NoHome);
    assert!(matches!(
        fx.ctx.status(Target::Codex).state,
        HookState::ConfigInvalid { .. }
    ));
}

// ── where the files are ──

#[test]
fn custom_config_dirs_are_honoured_and_the_default_home_is_left_alone() {
    let mut fx = Fx::new();
    let claude_dir = fx.tmp.path().join("cfg/claude");
    let codex_dir = fx.tmp.path().join("cfg/codex");
    fs::create_dir_all(claude_dir.parent().unwrap()).unwrap();
    fx.ctx.env.claude_config_dir = Some(claude_dir.clone().into_os_string());
    fx.ctx.env.codex_home = Some(codex_dir.clone().into_os_string());
    install(&fx.ctx, CLAUDE);
    install(&fx.ctx, CODEX);
    assert!(claude_dir.join("settings.json").is_file());
    assert!(codex_dir.join("hooks.json").is_file());
    assert!(!fx.home.join(".claude").exists() && !fx.home.join(".codex").exists());
    // A relative override is refused, not resolved against some cwd.
    fx.ctx.env.codex_home = Some("relative/codex".into());
    assert_eq!(refused_install(&fx, CODEX).0, RefusalReason::NoHome);
}

#[test]
fn worktree_configs_are_refused_even_through_an_ancestor_alias() {
    let mut fx = Fx::new();
    let repo = fx.tmp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let claude = repo.join(".claude");
    fx.ctx.env.claude_config_dir = Some(claude.clone().into_os_string());
    assert_eq!(refused_install(&fx, CLAUDE).0, RefusalReason::UnsafeTarget);
    assert!(matches!(
        fx.ctx.install_result(Target::ClaudeCode, "absent").unwrap(),
        ApplyResult::Refused {
            reason: RefusalReason::UnsafeTarget,
            ..
        }
    ));
    assert!(!claude.exists());

    let linked = fx.tmp.path().join("linked");
    let codex = linked.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    let alias = fx.tmp.path().join("alias");
    symlink(&linked, &alias).unwrap();
    fx.ctx.env.codex_home = Some(alias.join(".codex").into_os_string());
    assert_eq!(applied(install(&fx.ctx, CODEX)), ChangeOutcome::CreateFile);
    let path = codex.join("hooks.json");
    let installed = fs::read(&path).unwrap();
    let preview = fx.ctx.preview_uninstall(Target::Codex).unwrap();

    // A linked worktree is identified by a .git file instead of a directory.
    fs::write(
        linked.join(".git"),
        "gitdir: ../repo/.git/worktrees/linked\n",
    )
    .unwrap();
    assert_eq!(refused_install(&fx, CODEX).0, RefusalReason::UnsafeTarget);
    assert!(matches!(
        fx.ctx
            .uninstall_result(Target::Codex, &preview.fingerprint)
            .unwrap(),
        ApplyResult::Refused {
            reason: RefusalReason::UnsafeTarget,
            ..
        }
    ));
    assert_eq!(fs::read(path).unwrap(), installed);
}

#[test]
fn an_install_that_would_exceed_the_read_limit_is_refused() {
    let fx = Fx::new();
    let config = format!(
        "{{\"model\":\"{}\"}}",
        "x".repeat(MAX_CONFIG_BYTES as usize - 32)
    );
    let path = fx.claude_file();
    fx.write(&path, &config);
    assert_eq!(refused_install(&fx, CLAUDE).0, RefusalReason::TooLarge);
    assert!(matches!(
        fx.ctx
            .install_result(Target::ClaudeCode, &sha256_hex(config.as_bytes()))
            .unwrap(),
        ApplyResult::Refused {
            reason: RefusalReason::TooLarge,
            ..
        }
    ));
    assert_eq!(fs::read(path).unwrap(), config.as_bytes());
    assert!(!fx.home.join(".local").exists());
}

#[test]
fn codex_hooks_go_to_hooks_json_and_config_toml_is_never_touched() {
    let fx = Fx::new();
    let toml = fx.home.join(".codex/config.toml");
    let sentinel = "[hooks.state]\n# WHY must not read or change this\n";
    fx.write(&toml, sentinel);
    let before = fs::metadata(&toml).unwrap().modified().unwrap();
    let preview = ready(fx.ctx.preview_install_result(CODEX.into()).unwrap());
    assert!(
        preview.notes.iter().any(|n| n.contains("/hooks")),
        "{:?}",
        preview.notes
    );
    install(&fx.ctx, CODEX);
    assert_eq!(fs::read_to_string(&toml).unwrap(), sentinel);
    assert_eq!(fs::metadata(&toml).unwrap().modified().unwrap(), before);
    let written = fx.read(&fx.codex_file()).unwrap();
    let all = String::from_utf8(written.clone()).unwrap();
    assert!(!all.contains("bypass") && !all.contains("trust"));
    // Eight events, no model-request ones.
    assert_eq!(commands(&written).len(), 8);
    assert!(!all.contains("ModelRequest"));
    // Matching entries are `needsReview`, never plain `installed`.
    assert!(matches!(
        fx.ctx.status(Target::Codex).state,
        HookState::NeedsReview { ref hint } if hint.contains("/hooks")
    ));
}

// ── policy ──

#[test]
fn managed_policy_blocks_install_but_not_uninstall() {
    let mut fx = Fx::new();
    let managed = fx.tmp.path().join("managed-settings.json");
    fs::write(&managed, r#"{"allowManagedHooksOnly": true}"#).unwrap();
    let reqs = fx.tmp.path().join("requirements.toml");
    fs::write(&reqs, "features = { hooks = false }\n").unwrap();

    // Installed earlier, then the admin turned the policy on.
    install(&fx.ctx, CLAUDE);
    install(&fx.ctx, CODEX);
    fx.ctx.env.claude_managed_files = vec![managed.clone()];
    fx.ctx.env.codex_requirements = vec![reqs.clone()];

    for (h, target) in [(CLAUDE, Target::ClaudeCode), (CODEX, Target::Codex)] {
        assert_eq!(refused_install(&fx, h).0, RefusalReason::DisabledByPolicy);
        assert!(matches!(
            fx.ctx.status(target).state,
            HookState::DisabledByPolicy { .. }
        ));
        // Removing our own entries is always allowed.
        assert_eq!(applied(uninstall(&fx.ctx, h)), ChangeOutcome::DeleteFile);
    }
    // Policy present from the start: nothing is written at all.
    let fx2 = {
        let mut f = Fx::new();
        f.ctx.env.claude_managed_files = vec![managed];
        f
    };
    assert_eq!(
        refused_install(&fx2, CLAUDE).0,
        RefusalReason::DisabledByPolicy
    );
    assert!(!fx2.home.join(".claude").exists());
    assert!(!fx2.home.join(".local").exists());
}

// ── the hook binary ──

#[test]
fn a_missing_hook_binary_is_reported_and_blocks_install() {
    let mut fx = Fx::new();
    fx.ctx.env.bundled_hook = None; // a dev run before `cargo build -p why-hook`
    let preview = ready(fx.ctx.preview_install_result(CLAUDE.into()).unwrap());
    assert_eq!(preview.binary.status, BinaryStatus::Missing);
    assert_eq!(preview.binary.bundled_path, None);
    assert_eq!(
        refused_install(&fx, CLAUDE).0,
        RefusalReason::HookBinaryMissing
    );
    assert!(
        !fx.claude_file().exists(),
        "no config may point at a missing binary"
    );
    // With a bundled copy the install puts the stable binary in place, mode 0755.
    let fx = Fx::new();
    let stable = fx.home.join(".local/share/why/bin/why-hook");
    assert!(!stable.exists());
    install(&fx.ctx, CLAUDE);
    assert_eq!(fs::read(&stable).unwrap(), FAKE_HOOK.as_bytes());
    assert_eq!(fs::metadata(&stable).unwrap().mode() & 0o777, 0o755);
}

#[test]
fn a_non_executable_stable_binary_needs_a_usable_replacement() {
    let mut fx = Fx::new();
    install(&fx.ctx, CLAUDE);
    let stable = fx.home.join(".local/share/why/bin/why-hook");
    fs::set_permissions(&stable, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(fx.ctx.binary_info().unwrap().status, BinaryStatus::Outdated);
    assert!(matches!(
        fx.ctx.status(Target::ClaudeCode).state,
        HookState::Outdated { .. }
    ));

    fx.ctx.env.bundled_hook = None;
    assert_eq!(fx.ctx.binary_info().unwrap().status, BinaryStatus::Outdated);
    assert_eq!(
        refused_install(&fx, CLAUDE).0,
        RefusalReason::HookBinaryMissing
    );

    fx.ctx.env.bundled_hook = Some(fx.bundled.clone());
    assert_eq!(applied(install(&fx.ctx, CLAUDE)), ChangeOutcome::NoChange);
    assert_eq!(fs::metadata(stable).unwrap().mode() & 0o777, 0o755);
    assert_eq!(
        fx.ctx.status(Target::ClaudeCode).state,
        HookState::Installed
    );
}

#[test]
fn reinstall_removes_a_matcher_from_whys_group_and_preserves_the_users_matcher() {
    let fx = Fx::new();
    let path = fx.claude_file();
    let original = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"mine"}]}]}}"#;
    fx.write(&path, original);
    install(&fx.ctx, CLAUDE);
    let mut edited = json(&fs::read(&path).unwrap());
    edited["hooks"]["PreToolUse"][1]["matcher"] = serde_json::json!("Read");
    fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();

    let HookState::Outdated { reasons } = fx.ctx.status(Target::ClaudeCode).state else {
        panic!("expected an outdated hook");
    };
    assert!(reasons.iter().any(|reason| reason.contains("matcher")));
    assert_eq!(applied(install(&fx.ctx, CLAUDE)), ChangeOutcome::ModifyFile);
    let repaired = json(&fs::read(&path).unwrap());
    assert_eq!(
        repaired["hooks"]["PreToolUse"][0],
        edited["hooks"]["PreToolUse"][0]
    );
    assert!(repaired["hooks"]["PreToolUse"][1].get("matcher").is_none());
    assert_eq!(
        fx.ctx.status(Target::ClaudeCode).state,
        HookState::Installed
    );
}

#[test]
fn status_follows_the_install_lifecycle() {
    let fx = Fx::new();
    let state = |c: &why_lib::hooks::HookContext| c.status(Target::ClaudeCode).state;
    assert_eq!(state(&fx.ctx), HookState::NotInstalled);
    install(&fx.ctx, CLAUDE);
    assert_eq!(state(&fx.ctx), HookState::Installed);

    // A new build bundles a different hook binary: installed but outdated.
    fs::write(&fx.bundled, "#!/bin/sh\n# v2\n").unwrap();
    match state(&fx.ctx) {
        HookState::Outdated { reasons } => assert!(reasons.iter().any(|r| r.contains("binary"))),
        other => panic!("{other:?}"),
    }
    install(&fx.ctx, CLAUDE); // refreshes the binary; config unchanged
    assert_eq!(state(&fx.ctx), HookState::Installed);

    // The entries are edited by hand (timeout changed): outdated, and install repairs it.
    let text = String::from_utf8(fx.read(&fx.claude_file()).unwrap()).unwrap();
    fs::write(
        fx.claude_file(),
        text.replacen("\"timeout\": 5", "\"timeout\": 600", 1),
    )
    .unwrap();
    assert!(matches!(state(&fx.ctx), HookState::Outdated { .. }));
    assert_eq!(applied(install(&fx.ctx, CLAUDE)), ChangeOutcome::ModifyFile);
    assert_eq!(state(&fx.ctx), HookState::Installed);

    // The stable binary vanishes.
    fs::remove_file(fx.home.join(".local/share/why/bin/why-hook")).unwrap();
    assert!(matches!(state(&fx.ctx), HookState::Outdated { .. }));
}

// ── the command really survives a shell ──

#[test]
fn the_written_command_runs_under_sh_with_hostile_paths() {
    // Data dir name with spaces, quotes, `$`, backticks and unicode.
    let fx = Fx::with_data_dir_name("da ta's $HOME `x` ünï");
    install(&fx.ctx, CLAUDE);
    let written = fx.read(&fx.claude_file()).unwrap();
    let cmd = commands(&written).into_iter().next().unwrap();
    let out = fx.tmp.path().join("args.txt");
    let status = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .env("WHY_TEST_OUT", &out)
        .status()
        .unwrap();
    assert!(status.success());
    let got: Vec<String> = fs::read_to_string(&out)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let paths = &fx.ctx.paths;
    assert_eq!(
        got,
        [
            "--harness",
            "claude-code",
            "--instance",
            "installed",
            "--data-dir",
            paths.data_dir.to_str().unwrap(),
            "--socket-path",
            paths.socket_path.to_str().unwrap(),
        ]
    );
    // No side effects of the shell metacharacters in the path.
    assert!(!fx.tmp.path().join("x").exists());
}

// ── preview samples (printed with --nocapture, pasted into the report) ──

#[test]
fn sample_previews() {
    for (h, file, existing) in [
        (
            CLAUDE,
            "claude",
            Some(
                "{\n  \"model\": \"opus\",\n  \"hooks\": {\n    \"PreToolUse\": [\n      {\n        \"matcher\": \"Bash\",\n        \"hooks\": [{ \"type\": \"command\", \"command\": \"~/bin/audit.sh\" }]\n      }\n    ]\n  }\n}\n",
            ),
        ),
        (CODEX, "codex", None),
    ] {
        let fx = Fx::new();
        if let Some(text) = existing {
            fx.write(&fx.claude_file(), text);
        }
        let p = ready(fx.ctx.preview_install_result(h.into()).unwrap());
        let diff = p.diff.replace(fx.tmp.path().to_str().unwrap(), "<TMP>");
        println!("=== {file} install preview ({:?}) ===\n{diff}", p.outcome);
        assert!(!p.diff.is_empty());
    }
}

#[test]
fn a_dev_install_never_touches_the_installed_instances_binary() {
    // Decision 25: each instance has its own stable copy, so a worker's
    // in-progress build cannot replace what real agents run for the installed
    // instance (Codex would also need a re-approval after a hash change).
    let fx = Fx::new();
    let dev_bundle = fx.tmp.path().join("dev-bundle-why-hook");
    fs::write(&dev_bundle, "#!/bin/sh\n# dev build\n").unwrap();
    fs::set_permissions(&dev_bundle, fs::Permissions::from_mode(0o755)).unwrap();
    let mut dev = fx.dev(1520);
    dev.env.bundled_hook = Some(dev_bundle);

    install(&fx.ctx, CLAUDE);
    let installed_bin = fx.home.join(".local/share/why/bin/why-hook");
    let installed_bytes = fs::read(&installed_bin).unwrap();
    let installed_mtime = fs::metadata(&installed_bin).unwrap().modified().unwrap();

    install(&dev, CLAUDE);
    let dev_bin = fx.home.join(".local/share/why/bin/dev-1520/why-hook");
    assert_eq!(
        fs::read_to_string(&dev_bin).unwrap(),
        "#!/bin/sh\n# dev build\n"
    );
    // The installed copy is byte-for-byte what it was (and was not rewritten).
    assert_eq!(fs::read(&installed_bin).unwrap(), installed_bytes);
    assert_eq!(
        fs::metadata(&installed_bin).unwrap().modified().unwrap(),
        installed_mtime
    );
    // Each instance's command names its own binary and both are still "own".
    let cmds = commands(&fx.read(&fx.claude_file()).unwrap());
    let installed_cmd = cmds.iter().find(|c| c.contains("'installed'")).unwrap();
    let dev_cmd = cmds.iter().find(|c| c.contains("'dev:1520'")).unwrap();
    assert!(installed_cmd.starts_with(&format!("'{}'", installed_bin.display())));
    assert!(dev_cmd.starts_with(&format!("'{}'", dev_bin.display())));
    // And neither instance reports the other's binary as its problem.
    assert_eq!(
        fx.ctx.status(Target::ClaudeCode).state,
        HookState::Installed
    );
    assert_eq!(dev.status(Target::ClaudeCode).state, HookState::Installed);
}

#[test]
fn the_preview_says_where_the_private_copy_of_the_original_lives_and_uninstall_deletes_it() {
    let fx = Fx::new();
    let original = "{\"env\": {\"API_TOKEN\": \"secret\"}}\n";
    fx.write(&fx.claude_file(), original);
    let preview = ready(fx.ctx.preview_install_result(CLAUDE.into()).unwrap());
    let note = preview
        .notes
        .iter()
        .find(|n| n.contains("private copy"))
        .unwrap_or_else(|| panic!("no note about the copy: {:?}", preview.notes));
    assert!(note.contains("deleted on uninstall"), "{note}");
    let hook_installs = fx.ctx.paths.data_dir.join("hook-installs");
    assert!(note.contains(hook_installs.to_str().unwrap()), "{note}");

    install(&fx.ctx, CLAUDE);
    let copies = |dir: &std::path::Path| -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    found.push(p);
                }
            }
        }
        found
    };
    let files = copies(&hook_installs);
    let orig: Vec<_> = files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "orig"))
        .collect();
    assert_eq!(orig.len(), 1, "{files:?}");
    // The note names the real file, and it holds the original bytes, mode 0600.
    assert!(note.contains(orig[0].to_str().unwrap()), "{note}");
    assert_eq!(fs::read(orig[0]).unwrap(), original.as_bytes());
    assert_eq!(fs::metadata(orig[0]).unwrap().mode() & 0o777, 0o600);

    // Uninstall (restore path) leaves no copy and no record behind.
    uninstall(&fx.ctx, CLAUDE);
    assert!(
        copies(&hook_installs).is_empty(),
        "{:?}",
        copies(&hook_installs)
    );

    // The edited-since path (RemoveEntries) cleans up as well.
    install(&fx.ctx, CLAUDE);
    let text = String::from_utf8(fx.read(&fx.claude_file()).unwrap()).unwrap();
    fs::write(fx.claude_file(), format!("{text} ")).unwrap();
    assert_eq!(
        applied(uninstall(&fx.ctx, CLAUDE)),
        ChangeOutcome::RemoveEntries
    );
    assert!(
        copies(&hook_installs).is_empty(),
        "{:?}",
        copies(&hook_installs)
    );
    // A file WHY created has no copy to announce.
    let fx = Fx::new();
    let preview = ready(fx.ctx.preview_install_result(CODEX.into()).unwrap());
    assert!(!preview.notes.iter().any(|n| n.contains("private copy")));
}

#[test]
fn disable_all_hooks_in_the_users_own_settings_is_reported_not_refused() {
    let fx = Fx::new();
    fx.write(&fx.claude_file(), "{\n  \"disableAllHooks\": true\n}\n");
    // Not installed yet: the preview warns, install is still allowed.
    let preview = ready(fx.ctx.preview_install_result(CLAUDE.into()).unwrap());
    assert!(
        preview.notes.iter().any(|n| n.contains("disableAllHooks")),
        "{:?}",
        preview.notes
    );
    assert_eq!(
        fx.ctx.status(Target::ClaudeCode).state,
        HookState::NotInstalled
    );
    assert_eq!(applied(install(&fx.ctx, CLAUDE)), ChangeOutcome::ModifyFile);

    // Installed, but nothing will run: status must not say "installed".
    match fx.ctx.status(Target::ClaudeCode).state {
        HookState::DisabledByUserSetting { message } => {
            assert!(message.contains("disableAllHooks"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    // Turning it off again (by the user) restores the normal state.
    let text = String::from_utf8(fx.read(&fx.claude_file()).unwrap()).unwrap();
    fs::write(fx.claude_file(), text.replace("true", "false")).unwrap();
    assert_eq!(
        fx.ctx.status(Target::ClaudeCode).state,
        HookState::Installed
    );
    // `false` or absent never triggers it.
}
