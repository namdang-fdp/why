//! The shell command WHY writes into an agent's hook config, and the rule that
//! recognises it again.
//!
//! Both harnesses run a hook's `command` string through a shell, so every value
//! we put in it (the binary path, the data dir, the socket path) must survive
//! being re-parsed by `sh`: a home directory with a space, a `'`, a `$` or a
//! newline must not split into two arguments or run something.
//!
//! The strategy is the simplest one that is provably safe: wrap each value in
//! single quotes, the one quoting form in which the shell interprets nothing,
//! and spell a literal `'` as `'\''` (close quote, escaped quote, reopen).

use std::path::Path;

use super::Target;
use crate::collector::install::HOOK_BINARY;

/// Quote `s` for POSIX `sh`. Always quotes, even harmless text, so the output
/// is uniform and an auditor can see exactly which parts are data.
pub fn sh_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Split a command into words, understanding ONLY the subset of shell syntax
/// that [`sh_quote`] and plain flags produce: bare words made of harmless
/// characters, `'single quoted'` runs and `\'`. Anything else (`$`, backticks,
/// `;`, `"`, redirections, ...) returns `None`: such a command is not ours, and
/// we must never guess what it means.
pub fn split_words(cmd: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => cur.push(c),
                    }
                }
            }
            '\\' => {
                // Only an escaped single quote is part of our own output.
                if chars.next()? != '\'' {
                    return None;
                }
                in_word = true;
                cur.push('\'');
            }
            c if c.is_ascii_alphanumeric() || "_-./:=+,@%".contains(c) => {
                in_word = true;
                cur.push(c);
            }
            _ => return None,
        }
    }
    if in_word {
        words.push(cur);
    }
    Some(words)
}

/// `'<hook>' --harness <h> --instance '<i>' --data-dir '<d>' --socket-path '<s>'`.
/// The flag names are constants; every value is quoted.
pub fn hook_command(
    hook: &str,
    target: Target,
    instance: &str,
    data_dir: &str,
    socket_path: &str,
) -> String {
    format!(
        "{} --harness {} --instance {} --data-dir {} --socket-path {}",
        sh_quote(hook),
        target.hook_arg(),
        sh_quote(instance),
        sh_quote(data_dir),
        sh_quote(socket_path),
    )
}

/// THE ownership rule. A handler's command is WHY's own entry for `target` and
/// `instance` iff, parsed with [`split_words`]:
///
/// 1. its first word is a path whose file name is `why-hook`,
/// 2. it contains `--harness <target>`, and
/// 3. it contains `--instance <instance>`.
///
/// The path itself is deliberately NOT compared: if the stable binary moves
/// (a different `XDG_DATA_HOME`), the old entry is still ours and shows up as
/// outdated instead of being orphaned forever. Dev and installed instances
/// differ in rule 3, so each touches only its own entries.
pub fn is_own_command(cmd: &str, target: Target, instance: &str) -> bool {
    let Some(words) = split_words(cmd) else {
        return false;
    };
    let named_hook = words
        .first()
        .is_some_and(|w| Path::new(w).file_name().is_some_and(|n| n == HOOK_BINARY));
    let flag = |name: &str, value: &str| {
        words
            .windows(2)
            .any(|pair| pair[0] == name && pair[1] == value)
    };
    named_hook && flag("--harness", target.hook_arg()) && flag("--instance", instance)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strings a hostile (or just unlucky) home directory could contain.
    const HOSTILE: &[&str] = &[
        "plain",
        "with space",
        "it's",
        "'",
        "''",
        "dq\"uote",
        "$HOME/x",
        "`touch /tmp/pwned`",
        "$(touch /tmp/pwned)",
        "a;b|c&d",
        "back\\slash",
        "new\nline",
        "tab\there",
        "ünïcode/日本語/🦀",
        "glob*?[x]",
        "~/tilde",
        "-leading-dash",
        "",
        "# not a comment",
        "!bang",
    ];

    #[test]
    fn quoting_survives_a_real_shell() {
        // printf prints each argument NUL-terminated: the shell's own parse of
        // our quoting is the oracle, not our reading of the POSIX spec.
        for s in HOSTILE {
            let script = format!("printf '%s\\0' {}", sh_quote(s));
            let out = std::process::Command::new("sh")
                .args(["-c", &script])
                .output()
                .unwrap();
            assert!(out.status.success(), "{s:?}");
            assert_eq!(out.stdout, [s.as_bytes(), b"\0"].concat(), "{s:?}");
        }
    }

    #[test]
    fn a_whole_hook_command_round_trips_through_sh_and_our_splitter() {
        for s in HOSTILE {
            let hook = format!("/h/{s}/why-hook");
            let cmd = hook_command(&hook, Target::Codex, "dev:1520", s, &format!("{s}/sock"));
            let script = format!("set -- {cmd}; printf '%s\\0' \"$@\"");
            // `set -- <cmd>` makes the shell split exactly like the harness will.
            let out = std::process::Command::new("sh")
                .args(["-c", &script])
                .output()
                .unwrap();
            let got: Vec<&[u8]> = out.stdout.split(|b| *b == 0).collect();
            // 9 words (program + 4 flags + 4 values), plus the empty tail after
            // the last NUL.
            assert_eq!(got.len(), 9 + 1, "{s:?}: {got:?}");
            assert_eq!(got[0], hook.as_bytes(), "{s:?}");
            assert_eq!(got[6], s.as_bytes(), "{s:?}");
            assert_eq!(got[8], format!("{s}/sock").as_bytes(), "{s:?}");
            // And our own splitter agrees with the shell.
            let words = split_words(&cmd).unwrap();
            assert_eq!(words[0], hook);
            assert_eq!(words[6], *s);
            assert_eq!(words.len(), 9);
        }
    }

    #[test]
    fn the_splitter_refuses_syntax_we_never_write() {
        for cmd in [
            "$HOME/why-hook --harness codex",
            "why-hook --harness \"codex\"",
            "why-hook; rm -rf x",
            "why-hook `x`",
            "why-hook | cat",
            "why-hook > /dev/null",
            "'unterminated",
            "why-hook \\n",
        ] {
            assert_eq!(split_words(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn ownership_rule() {
        let cmd = hook_command(
            "/d/why/bin/why-hook",
            Target::ClaudeCode,
            "installed",
            "/d",
            "/s",
        );
        assert!(is_own_command(&cmd, Target::ClaudeCode, "installed"));
        // Another instance, another harness: not ours.
        assert!(!is_own_command(&cmd, Target::ClaudeCode, "dev:1520"));
        assert!(!is_own_command(&cmd, Target::Codex, "installed"));
        // A moved binary is still ours (reported as outdated, not orphaned).
        let moved = hook_command(
            "/elsewhere/why-hook",
            Target::ClaudeCode,
            "installed",
            "/d",
            "/s",
        );
        assert!(is_own_command(&moved, Target::ClaudeCode, "installed"));
        // A different program that merely mentions the flags is not.
        assert!(!is_own_command(
            "/usr/bin/other --harness claude-code --instance installed",
            Target::ClaudeCode,
            "installed"
        ));
        // Unparseable (the user's own shell magic) is never ours.
        assert!(!is_own_command(
            "echo $(why-hook)",
            Target::ClaudeCode,
            "installed"
        ));
        assert!(!is_own_command("", Target::ClaudeCode, "installed"));
    }
}
