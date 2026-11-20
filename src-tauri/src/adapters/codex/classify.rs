//! Is this shell command a test run? A conservative heuristic, on purpose.
//!
//! The hook gives only the command string (no exit code, payloads.md §3), so
//! this decides how to LABEL a call in the timeline, never whether it passed.
//! A false "test run" label is worse than a missed one (it implies a check
//! happened), so the rule is narrow: a command segment must START with a known
//! runner. `echo pytest`, `grep -r "cargo test" docs` and `cat run-tests.sh`
//! are plain shell calls.
//!
//! Same idea as the Claude adapter's heuristic (WHY-14) but written here
//! independently: the two adapters share no code on purpose, so they can be
//! built and changed in parallel.

/// Wrappers that do not change what runs: `sudo pytest`, `time cargo test`.
const WRAPPERS: &[&str] = &["sudo", "time", "env", "nice", "command", "exec"];

/// Runners recognised by program name alone (any arguments).
const BARE_RUNNERS: &[&str] = &[
    "pytest", "py.test", "vitest", "jest", "rspec", "phpunit", "nextest", "tox", "ctest",
];

/// `(program, first argument)` pairs: `cargo test`, `go test`, `pnpm test`...
const PROGRAM_SUBCOMMANDS: &[(&str, &str)] = &[
    ("cargo", "test"),
    ("cargo", "nextest"),
    ("go", "test"),
    ("npm", "test"),
    ("pnpm", "test"),
    ("yarn", "test"),
    ("bun", "test"),
    ("deno", "test"),
    ("dotnet", "test"),
    ("mvn", "test"),
    ("gradle", "test"),
    ("make", "test"),
    ("make", "check"),
    ("swift", "test"),
    ("mix", "test"),
    ("flutter", "test"),
];

/// Runners started as `python -m <module>`.
const PYTHON_MODULES: &[&str] = &["pytest", "unittest", "nose2"];

fn program_name(token: &str) -> &str {
    // `/usr/bin/python3` and `./node_modules/.bin/jest` -> the file name.
    token.rsplit('/').next().unwrap_or(token)
}

fn is_python(program: &str) -> bool {
    program == "python"
        || program
            .strip_prefix("python")
            .is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        let mut chars = name.bytes();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
    })
}

fn python_runs_test_module(args: &[&str]) -> bool {
    let mut args = args.iter().copied();
    while let Some(option) = args.next() {
        match option {
            "-m" => {
                return args
                    .next()
                    .is_some_and(|module| PYTHON_MODULES.contains(&module));
            }
            "-W" | "-X" => {
                // Expansions can change how many arguments the option consumes.
                if args
                    .next()
                    .is_none_or(|arg| arg.contains(['$', '*', '?', '[', '{']))
                {
                    return false;
                }
            }
            "-b" | "-bb" | "-B" | "-E" | "-i" | "-I" | "-O" | "-OO" | "-P" | "-q" | "-R" | "-s"
            | "-S" | "-u" | "-v" | "-vv" => {}
            // A script, `-c`, stdin or an option we do not understand ends the search.
            _ => return false,
        }
    }
    false
}

fn segment_is_test_run(words: &[&str]) -> bool {
    // Skip `FOO=bar` assignments and transparent wrappers in front.
    let first = words
        .iter()
        .position(|word| !is_assignment(word) && !WRAPPERS.contains(word));
    let Some((first, args)) = first.and_then(|i| words[i..].split_first()) else {
        return false;
    };
    let program = program_name(first);
    if BARE_RUNNERS.contains(&program) {
        return true;
    }
    if is_python(program) {
        return python_runs_test_module(args);
    }
    if let Some(sub) = args.first() {
        // `npm run test` is the common spelling of `npm test`.
        if matches!(program, "npm" | "pnpm" | "yarn" | "bun") && *sub == "run" {
            return args.get(1).is_some_and(|s| *s == "test");
        }
        return PROGRAM_SUBCOMMANDS.contains(&(program, *sub));
    }
    false
}

/// True when a simple command starts with a known test runner. Keep quoted or
/// escaped text within its word. Leave complex shell syntax labeled Shell.
pub fn is_test_command(command: &str) -> bool {
    let mut chars = command.char_indices().peekable();
    let mut words = Vec::new();
    let mut word_start = None;
    let mut quote = None;
    let mut found = false;
    while let Some((i, c)) = chars.next() {
        if quote.is_some() {
            if quote == Some(c) {
                quote = None;
            } else if quote == Some('"') {
                match c {
                    '\\' => {
                        chars.next();
                    }
                    '`' => return false,
                    '$' if chars.peek().is_some_and(|&(_, c)| matches!(c, '(' | '{')) => {
                        return false;
                    }
                    _ => {}
                }
            }
            continue;
        }
        match c {
            ' ' | '\t' | '\n' | ';' | '|' | '&' => {
                if let Some(start) = word_start.take() {
                    words.push(&command[start..i]);
                }
                if matches!(c, '\n' | ';' | '|' | '&') {
                    found |= segment_is_test_run(&words);
                    words.clear();
                }
            }
            '#' if word_start.is_none() => while chars.next_if(|&(_, c)| c != '\n').is_some() {},
            '\\' => match chars.next() {
                Some((_, '\n')) => {}
                Some(_) => {
                    word_start.get_or_insert(i);
                }
                None => return false,
            },
            // Redirections (including heredocs), substitutions and groups
            // have their own grammar for separators. Do not guess at it.
            '<' | '>' | '(' | ')' | '`' => return false,
            '$' if chars
                .peek()
                .is_some_and(|&(_, c)| matches!(c, '\'' | '"' | '(' | '{')) =>
            {
                return false;
            }
            '\'' | '"' => {
                word_start.get_or_insert(i);
                quote = Some(c);
            }
            _ => {
                word_start.get_or_insert(i);
            }
        }
    }
    if quote.is_some() {
        return false;
    }
    if let Some(start) = word_start {
        words.push(&command[start..]);
    }
    found || segment_is_test_run(&words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_common_runners() {
        for cmd in [
            "pytest",
            "pytest -q tests/",
            "python3 -m unittest discover -s tests",
            "python -m pytest",
            "python -I -m pytest",
            "python -W 'ignore:some warning' -m unittest",
            "/usr/bin/python3.12 -m pytest -x",
            "cargo test --lib",
            "cargo nextest run",
            "go test ./...",
            "pnpm test",
            "npm run test",
            "yarn test --watch=false",
            "FOO=1 vitest run",
            "sudo make test",
            "cd src && cargo test",
            "cargo fmt; cargo test",
            "printf '%s' 'setup; pytest -q' && cargo test",
            "./node_modules/.bin/jest",
        ] {
            assert!(is_test_command(cmd), "{cmd}");
        }
    }

    #[test]
    fn plain_shell_is_not_a_test_run() {
        for cmd in [
            "",
            "ls -la",
            "echo pytest",
            "grep -r \"cargo test\" docs",
            "cat run-tests.sh",
            "git log --oneline",
            "python3 script.py",
            "python tools/report.py -m pytest",
            "python -c 'print(0)' -m pytest",
            "python - -m pytest",
            "python -- -m pytest",
            "python -m pip -m pytest",
            "python -W -m pytest",
            "python -W $WARNINGS -m pytest",
            "printf '%s\\n' 'setup; pytest -q'",
            "printf \"setup && cargo test\"",
            "printf setup\\; pytest -q",
            "printf '%s' 'setup\npytest -q'",
            "FOO='value pytest -q' echo setup",
            "echo setup # ; pytest -q",
            "echo setup \\\n# ; pytest -q",
            "cat <<EOF\npytest -q\nEOF",
            "printf '%s' 'setup; pytest -q",
            "python3 -m http.server",
            "cargo build",
            "npm install",
            "sh -c \"exit 3\" .",
            "cd tests",
        ] {
            assert!(!is_test_command(cmd), "{cmd}");
        }
    }
}
