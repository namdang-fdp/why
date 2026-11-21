//! Is this Bash command a test run? A small, deliberately conservative guess.
//!
//! The timeline labels a successful `TestRun` as passed. A hook reports the
//! status of the whole shell command, so compound commands stay shell rows.
//!
//! Rules (a miss only costs an icon, a false hit mislabels, so we prefer misses):
//! 1. Accept a single simple command. Leave shell operators, redirections and
//!    substitutions to shell rows. Quoted literal arguments are fine.
//! 2. Skip leading `VAR=value` words and `time`.
//! 3. The command must START with a known runner:
//!    `cargo test`, `cargo nextest run`, `pytest`, `py.test`,
//!    `python[3] -m pytest|unittest`, `go test`, `vitest`, `jest`, `mocha`
//!    (bare, or through `npx`/`bunx`/`pnpm [exec|dlx]`/`yarn`/`npm exec`),
//!    and the package-manager scripts `npm|pnpm|yarn|bun test` (also
//!    `... run test`, `npm t`).
//!
//! Not detected on purpose: wrappers (`bash -c "..."`, `make test`,
//! `uv run pytest`, `nix develop -c ...`), subshells, `$(...)`.

/// `true` when `command` is a single simple invocation of a known test runner.
pub fn is_test_run(command: &str) -> bool {
    simple_words(command).is_some_and(|words| words_are_test_run(&words))
}

/// Split the supported shell syntax into words, removing quotes and escapes.
fn simple_words(line: &str) -> Option<Vec<String>> {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.trim_matches([' ', '\t', '\n']).chars();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if q == '"' && matches!(c, '$' | '`') {
                    return None;
                } else if q == '"' && c == '\\' {
                    match chars.next()? {
                        '\n' => {}
                        next @ ('"' | '\\' | '$' | '`') => word.push(next),
                        next => {
                            word.push('\\');
                            word.push(next);
                        }
                    }
                } else {
                    word.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    in_word = true;
                }
                '\\' => {
                    let next = chars.next()?;
                    if next != '\n' {
                        in_word = true;
                        word.push(next);
                    }
                }
                '&' | '|' | ';' | '\n' | '<' | '>' | '(' | ')' | '$' | '`' => return None,
                '#' if !in_word => return None,
                ' ' | '\t' => {
                    if in_word {
                        words.push(std::mem::take(&mut word));
                        in_word = false;
                    }
                }
                c => {
                    word.push(c);
                    in_word = true;
                }
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

/// `NAME=value` (a shell variable assignment before a command).
fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

fn words_are_test_run(words: &[String]) -> bool {
    let mut words = words
        .iter()
        .map(String::as_str)
        .skip_while(|w| is_assignment(w) || *w == "time");
    let Some(head) = words.next() else {
        return false;
    };
    let args: Vec<_> = words.collect();
    let args = args.as_slice();
    // `/usr/bin/pytest` is still pytest.
    let head = head.rsplit('/').next().unwrap_or(head);

    match head {
        "cargo" => {
            // `cargo +nightly test`: skip the toolchain selector.
            let args: Vec<&str> = args
                .iter()
                .copied()
                .skip_while(|a| a.starts_with('+'))
                .collect();
            matches!(args.as_slice(), ["test", ..] | ["nextest", "run", ..])
        }
        "pytest" | "py.test" | "vitest" | "jest" | "mocha" => true,
        "python" | "python3" => matches!(args, ["-m", "pytest" | "unittest", ..]),
        h if h.starts_with("python3.") => matches!(args, ["-m", "pytest" | "unittest", ..]),
        "go" => matches!(args, ["test", ..]),
        "npx" | "bunx" => first_non_flag(args).is_some_and(is_js_runner),
        "npm" => match args {
            ["test" | "t", ..] => true,
            // `npm run test`, but `npm run test-all` is some other script.
            ["run" | "run-script", "test", ..] => true,
            ["exec", rest @ ..] => first_non_flag(rest).is_some_and(is_js_runner),
            _ => false,
        },
        "pnpm" | "yarn" | "bun" => match args {
            ["test", ..] => true,
            ["run", "test", ..] => true,
            ["exec" | "dlx", rest @ ..] => first_non_flag(rest).is_some_and(is_js_runner),
            [runner, ..] => is_js_runner(runner),
            [] => false,
        },
        _ => false,
    }
}

fn is_js_runner(word: &str) -> bool {
    matches!(word, "vitest" | "jest" | "mocha")
}

fn first_non_flag<'a>(args: &[&'a str]) -> Option<&'a str> {
    args.iter().copied().find(|a| !a.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_common_runners() {
        for cmd in [
            "cargo test",
            "cargo test -p why --lib",
            "cargo +nightly test",
            "cargo nextest run",
            "RUST_LOG=debug cargo test",
            "pytest -q",
            "python3 -m pytest tests/",
            "python -m unittest discover -s tests",
            "python3.12 -m unittest",
            "go test ./...",
            "npx vitest run",
            "npx --yes jest",
            "pnpm test",
            "pnpm run test",
            "pnpm exec vitest",
            "pnpm vitest run",
            "npm test",
            "npm t",
            "npm run test",
            "yarn test",
            "bun test",
            "time cargo test",
            "pytest -k 'a;b'\n",
            "/usr/bin/pytest",
            "mocha",
        ] {
            assert!(is_test_run(cmd), "should be a test run: {cmd}");
        }
    }

    #[test]
    fn rejects_everything_else() {
        for cmd in [
            "",
            "ls -la",
            "echo cargo test",
            "echo \"cargo test\"",
            "git commit -m 'pnpm test'",
            "cargo build",
            "cargo check",
            "cargo nextest list",
            "python3 script.py",
            "python3 -m http.server",
            "go build ./...",
            "go vet ./... && go fmt",
            "npm run test-all",
            "npm install",
            "pnpm install",
            "pnpm lint",
            "npx prettier --write .",
            "cat tests/test_calc.py",
            "grep -r pytest .",
            "make test",
            "bash -c 'cargo test'",
            "exit 3",
            "FOO=1",
        ] {
            assert!(!is_test_run(cmd), "should NOT be a test run: {cmd}");
        }
    }

    #[test]
    fn ambiguous_shell_outcomes_are_not_test_results() {
        for cmd in [
            "cargo test; echo exit=$?",
            "cargo test || true",
            "cargo test && echo done",
            "cd src-tauri && cargo test",
            "pytest\necho done",
            "pytest # comment\necho done",
            "pytest | tail -5",
            "pytest &",
            "pytest 2>&1 | tail -5",
            "pytest $(echo --help)",
            "pytest \"unterminated",
            "pytest \\",
        ] {
            assert!(!is_test_run(cmd), "should be plain shell: {cmd}");
        }
    }
}
