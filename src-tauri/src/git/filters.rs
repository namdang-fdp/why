//! Git *filter drivers*: the one way a repository's own configuration can make
//! a plain `git status` run a program, and WHY's guard against it (WHY-39).
//!
//! # The threat
//!
//! A filter driver is a pair of config entries plus an attribute:
//!
//! ```text
//! .git/config:     [filter "x"]  clean = /path/to/program
//! .gitattributes:  * filter=x
//! ```
//!
//! Whenever git needs to know whether a tracked file changed, it may run
//! `clean` (or the long-running `process` variant) on the file *as a side
//! effect of reading it*. Measured on git 2.56 (see `SECURITY.md`): `status`,
//! `diff` against the working tree, `diff-files`, `diff-index`, `ls-files -m`
//! and `hash-object` all do it; `diff --cached`, `cat-file`, `rev-parse` and
//! `ls-files -c -i` do not. Unlike `core.fsmonitor` there is no `-c` switch
//! that turns every filter off, so the runner cannot neutralise this class.
//!
//! WHY watches repositories that coding agents (and whatever the agents
//! installed or were prompt-injected into running) write to. Anything that can
//! write `.git/config` could therefore make WHY execute a program just because
//! WHY *looked* at the repository. This module is the answer:
//!
//! 1. [`scan_filter_drivers`] asks git which drivers apply, and **from which
//!    config scope** each setting comes.
//! 2. Settings from the *user's own* config files (`system`, `global`) are as
//!    trusted as the user running `git status` themselves. Typical: git-lfs.
//!    Settings from the repository's scopes (`local`, `worktree`), from WHY's
//!    process environment (`command`: `GIT_CONFIG_COUNT`, ...) or from any scope
//!    we do not recognise (fail closed) are **untrusted** until the user says
//!    otherwise.
//! 3. [`authorize`] turns "this worktree, these drivers, this trusted set" into
//!    a [`ReadPermit`] or a [`GitError::UntrustedFilters`].
//! 4. The git functions that read the working tree (`read_status`,
//!    `read_changes`) *require* a `ReadPermit`. A `ReadPermit` can only be made
//!    here, so the compiler enforces that no working-tree read skips the guard.
//!
//! # What this does not do
//!
//! * It does not stop a race: a driver written between the scan and the git
//!   command that follows it is not seen (see `SECURITY.md`).
//! * Trust covers the *configured command string*. If that string is a path
//!   inside the repository, the user is trusting a file the repository (an
//!   agent) can later rewrite.
//! * A `global`/`system` driver is trusted by construction.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::{GitError, ParseError};
use super::runner::GitRunner;

/// Where a git config entry came from, as reported by `--show-scope`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigScope {
    System,
    Global,
    /// The repository's own `.git/config`, and any file it `include`s.
    Local,
    /// `config.worktree` of a linked worktree (`extensions.worktreeConfig`).
    Worktree,
    /// `-c`, `GIT_CONFIG_COUNT` and friends: the environment WHY itself runs in.
    Command,
    /// A scope name this version of WHY does not know. Treated as untrusted.
    Other(String),
}

impl ConfigScope {
    fn parse(s: &str) -> Self {
        match s {
            "system" => Self::System,
            "global" => Self::Global,
            "local" => Self::Local,
            "worktree" => Self::Worktree,
            "command" => Self::Command,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::System => "system",
            Self::Global => "global",
            Self::Local => "local",
            Self::Worktree => "worktree",
            Self::Command => "command",
            Self::Other(s) => s,
        }
    }

    /// Written by the user, not by the repository. Only these are trusted
    /// without asking.
    ///
    /// Deliberately NOT here:
    /// * `Other`: an unknown scope may be a future repository-controlled one.
    /// * `Command`: WHY's process environment is not provably the user's. A
    ///   desktop app is launched from a shell, a launcher or a hook, and a
    ///   repository can influence a shell (`direnv`'s `.envrc`, a build script
    ///   that starts WHY) into exporting `GIT_CONFIG_COUNT` and friends. A real
    ///   user who exports a filter driver this way gets one prompt, which is
    ///   cheaper than silently running whatever the environment names.
    pub fn is_user_owned(&self) -> bool {
        matches!(self, Self::System | Self::Global)
    }
}

/// One effective value of a `filter.<name>.<variable>` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterSetting {
    pub value: String,
    pub scope: ConfigScope,
    /// The file git read it from (without git's `file:` prefix), for display.
    pub origin: String,
}

/// A configured filter driver, after git's own "last definition wins" rule.
///
/// "Effective" matters: a repository's `.git/config` can *override* the
/// `clean` command of a driver the user defined globally (`filter.lfs.clean`),
/// and then the program that runs is the repository's. Looking only at driver
/// names would miss that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterDriver {
    pub name: String,
    pub clean: Option<FilterSetting>,
    pub smudge: Option<FilterSetting>,
    /// Long-running variant; git prefers it over `clean` when both are set.
    pub process: Option<FilterSetting>,
    pub required: Option<FilterSetting>,
}

impl FilterDriver {
    /// The settings a *read* can execute: `clean` and `process`. (`smudge`
    /// only runs when git writes files into the working tree, which WHY never
    /// does.) An empty value means "no program" to git, so it is skipped.
    fn read_programs(&self) -> impl Iterator<Item = &FilterSetting> {
        [&self.clean, &self.process]
            .into_iter()
            .flatten()
            // Exactly git's rule: only the EMPTY string means "no program".
            // Not `trim()`: Rust's trim also strips Unicode whitespace (an NBSP),
            // which git would happily look up as a command name on PATH.
            .filter(|s| !s.value.is_empty())
    }

    /// WHY must not run its reads while this driver is untrusted: one of the
    /// programs a read would execute is controlled by the repository.
    pub fn needs_trust(&self) -> bool {
        self.read_programs().any(|s| !s.scope.is_user_owned())
    }

    /// The setting that makes this driver repository-controlled, for display:
    /// the first of `clean`/`process` from a non-user scope.
    pub fn untrusted_setting(&self) -> Option<&FilterSetting> {
        self.read_programs().find(|s| !s.scope.is_user_owned())
    }

    /// Stable identity of this driver *as configured*: changes when any of its
    /// four settings or the scope they come from changes. Persisted as the
    /// trust decision, so a new or edited driver is asked about again.
    /// Excludes the origin file path (it is relative or absolute depending on
    /// where git was run) so every worktree computes the same value.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"why:filter-driver:v1\0");
        feed(&mut hasher, &self.name);
        for (label, setting) in [
            ("clean", &self.clean),
            ("smudge", &self.smudge),
            ("process", &self.process),
            ("required", &self.required),
        ] {
            feed(&mut hasher, label);
            match setting {
                Some(s) => {
                    feed(&mut hasher, "some");
                    feed(&mut hasher, &s.value);
                    feed(&mut hasher, s.scope.as_str());
                }
                None => feed(&mut hasher, "none"),
            }
        }
        hex(&hasher.finalize())
    }
}

/// Length-prefixed, so `("ab", "c")` and `("a", "bc")` hash differently.
fn feed(hasher: &mut Sha256, field: &str) {
    hasher.update((field.len() as u32).to_le_bytes());
    hasher.update(field.as_bytes());
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Fingerprint of a whole set of drivers (the "trust this repository's
/// filters" prompt): what the UI shows the user and echoes back when they
/// confirm, so a driver that changed in between is not trusted blind.
/// Order-independent; empty set -> empty string.
pub fn set_fingerprint<'a>(drivers: impl IntoIterator<Item = &'a FilterDriver>) -> String {
    let mut each: Vec<String> = drivers.into_iter().map(FilterDriver::fingerprint).collect();
    if each.is_empty() {
        return String::new();
    }
    each.sort();
    each.dedup();
    let mut hasher = Sha256::new();
    hasher.update(b"why:filter-set:v1\0");
    for f in &each {
        feed(&mut hasher, f);
    }
    hex(&hasher.finalize())
}

/// Parse `git config -z --show-origin --show-scope --get-regexp '^filter\.'`.
///
/// Pure function. `-z` output is a flat list of NUL-terminated fields, three
/// per entry: `scope`, `origin`, then `key\nvalue` (the value is absent for a
/// key without `=`). NUL framing is what makes this safe for names and values
/// containing spaces or newlines.
///
/// Entries arrive in precedence order (system, global, local, worktree,
/// command), so a later entry for the same key replaces an earlier one, which
/// is also what git itself does. Drivers are returned sorted by name.
pub fn parse_filter_config(out: &[u8]) -> Result<Vec<FilterDriver>, ParseError> {
    // An empty input is "no entries", not one empty entry.
    if out.is_empty() {
        return Ok(Vec::new());
    }
    let mut fields = out.strip_suffix(&[0]).unwrap_or(out).split(|&b| b == 0);
    let mut drivers: BTreeMap<String, FilterDriver> = BTreeMap::new();
    while let Some(scope) = fields.next() {
        let (Some(origin), Some(entry)) = (fields.next(), fields.next()) else {
            return Err(ParseError::new("truncated `git config` output"));
        };
        // Strict UTF-8: the fingerprint must be over exactly what git said, and
        // a lossy conversion could make two different commands look equal.
        // Failing here blocks the repository (fail closed).
        let text = |bytes: &[u8]| {
            std::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|_| ParseError::new("filter configuration is not valid UTF-8"))
        };
        let scope = ConfigScope::parse(&text(scope)?);
        let origin = text(origin)?;
        let origin = origin.strip_prefix("file:").unwrap_or(&origin).to_string();
        let entry = text(entry)?;
        let (key, value) = entry.split_once('\n').unwrap_or((&entry, ""));

        // `filter.<name>.<variable>`: git lowercases the section and the
        // variable but keeps the name as written, and the name may itself
        // contain dots, so split the variable off the END.
        let Some(rest) = key.strip_prefix("filter.") else {
            continue;
        };
        let Some((name, variable)) = rest.rsplit_once('.') else {
            continue; // `[filter] clean = x` has no driver name
        };
        let driver = drivers
            .entry(name.to_string())
            .or_insert_with(|| FilterDriver {
                name: name.to_string(),
                clean: None,
                smudge: None,
                process: None,
                required: None,
            });
        let setting = Some(FilterSetting {
            value: value.to_string(),
            scope,
            origin,
        });
        match variable {
            "clean" => driver.clean = setting,
            "smudge" => driver.smudge = setting,
            "process" => driver.process = setting,
            "required" => driver.required = setting,
            _ => {}
        }
    }
    Ok(drivers.into_values().collect())
}

/// The filter drivers that apply to `worktree`, with their config scopes.
///
/// Runs through the [`GitRunner`] like everything else. Read-only. Run it per
/// *worktree*, not per repository: `config.worktree` (scope `worktree`) is
/// private to a linked worktree.
pub async fn scan_filter_drivers(
    runner: &GitRunner,
    worktree: &Path,
) -> Result<Vec<FilterDriver>, GitError> {
    match runner
        .run(
            worktree,
            [
                "config",
                "-z",
                "--show-origin",
                "--show-scope",
                "--get-regexp",
                r"^filter\.",
            ],
        )
        .await
    {
        Ok(out) => Ok(parse_filter_config(&out)?),
        // `--get-regexp` exits 1 with no output when nothing matches. That is
        // the ONLY failure that means "no drivers": an exit 1 that also wrote
        // to stderr is a real problem, and so is everything else (timeout,
        // not a repository...). Those propagate, and callers must treat an
        // error as "blocked".
        Err(GitError::NonZeroExit {
            code: Some(1),
            stderr,
            ..
        }) if stderr.is_empty() => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Proof that the filter guard approved reading `worktree`'s files.
///
/// The fields are private and the only constructor is [`authorize`], so holding
/// one means the check happened. `read_status` and `read_changes` take it by
/// reference and run git in [`ReadPermit::worktree`]: the directory that was
/// checked is the directory that is read.
#[derive(Debug, Clone)]
pub struct ReadPermit {
    worktree: PathBuf,
}

impl ReadPermit {
    pub fn worktree(&self) -> &Path {
        &self.worktree
    }
}

/// Decide whether `worktree` may be read, given its scanned `drivers` and the
/// fingerprints the user has `trusted`. Pure function.
///
/// Private on purpose: a [`ReadPermit`] is only meaningful if the drivers were
/// really scanned, so the one public way to get one is [`guard_working_tree`].
/// (Were this public, `authorize(path, &[], ..)` would mint a permit without
/// any scan and the "the compiler enforces the guard" claim would be false.)
fn authorize(
    worktree: &Path,
    drivers: &[FilterDriver],
    trusted: &HashSet<String>,
) -> Result<ReadPermit, GitError> {
    let blocked: Vec<FilterDriver> = drivers
        .iter()
        .filter(|d| d.needs_trust() && !trusted.contains(&d.fingerprint()))
        .cloned()
        .collect();
    if blocked.is_empty() {
        Ok(ReadPermit {
            worktree: worktree.to_path_buf(),
        })
    } else {
        Err(GitError::UntrustedFilters { drivers: blocked })
    }
}

/// Scan, then authorize: the one call every working-tree read starts with.
///
/// Call it as late as possible, immediately before the git command that reads
/// the files, to keep the window for a race as small as it can be.
pub async fn guard_working_tree(
    runner: &GitRunner,
    worktree: &Path,
    trusted: &HashSet<String>,
) -> Result<ReadPermit, GitError> {
    let drivers = scan_filter_drivers(runner, worktree).await?;
    authorize(worktree, &drivers, trusted)
}

/// Does `.gitattributes`-style text select the filter `name`? Best effort, for
/// display only (the guard never relies on it): the repository decides what
/// applies, and attribute files can also sit in any subdirectory.
pub fn attributes_reference(text: &str, name: &str) -> bool {
    let wanted = format!("filter={name}");
    text.lines()
        .map(|line| line.split('#').next().unwrap_or(""))
        .any(|line| line.split_whitespace().skip(1).any(|t| t == wanted))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build `-z` output from `(scope, origin, key, Some(value))` entries.
    fn z(entries: &[(&str, &str, &str, Option<&str>)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (scope, origin, key, value) in entries {
            out.extend_from_slice(format!("{scope}\0file:{origin}\0{key}").as_bytes());
            if let Some(v) = value {
                out.push(b'\n');
                out.extend_from_slice(v.as_bytes());
            }
            out.push(0);
        }
        out
    }

    #[test]
    fn empty_output_is_no_drivers() {
        assert_eq!(parse_filter_config(b"").unwrap(), vec![]);
    }

    #[test]
    fn parses_scopes_and_keeps_the_last_definition() {
        let out = z(&[
            (
                "global",
                "/home/u/.gitconfig",
                "filter.lfs.clean",
                Some("git-lfs clean"),
            ),
            (
                "global",
                "/home/u/.gitconfig",
                "filter.lfs.required",
                Some("true"),
            ),
            // The repository overrides the user's clean command.
            ("local", ".git/config", "filter.lfs.clean", Some("./evil")),
            ("local", ".git/config", "filter.a.b.clean", Some("dotted")),
        ]);
        let drivers = parse_filter_config(&out).unwrap();
        let names: Vec<_> = drivers.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["a.b", "lfs"], "sorted; dots in a name survive");

        let lfs = &drivers[1];
        let clean = lfs.clean.as_ref().unwrap();
        assert_eq!(clean.value, "./evil");
        assert_eq!(clean.scope, ConfigScope::Local);
        assert_eq!(clean.origin, ".git/config");
        assert_eq!(lfs.required.as_ref().unwrap().scope, ConfigScope::Global);
        assert!(
            lfs.needs_trust(),
            "a local override of a global driver is untrusted"
        );
    }

    #[test]
    fn user_scopes_are_trusted_repository_scopes_and_unknown_ones_are_not() {
        let driver = |scope: &str| {
            parse_filter_config(&z(&[(scope, "f", "filter.d.clean", Some("prog"))]))
                .unwrap()
                .remove(0)
        };
        assert!(!driver("system").needs_trust());
        assert!(!driver("global").needs_trust());
        // The process environment can be influenced by a repository (direnv, ...).
        assert!(driver("command").needs_trust());
        assert!(driver("local").needs_trust());
        assert!(driver("worktree").needs_trust());
        assert!(driver("some-future-scope").needs_trust(), "fail closed");
    }

    #[test]
    fn only_clean_and_process_decide_and_empty_values_are_no_program() {
        let only_smudge =
            parse_filter_config(&z(&[("local", "f", "filter.d.smudge", Some("prog"))]))
                .unwrap()
                .remove(0);
        assert!(!only_smudge.needs_trust(), "smudge never runs on a read");

        let process = parse_filter_config(&z(&[("local", "f", "filter.d.process", Some("p"))]))
            .unwrap()
            .remove(0);
        assert!(
            process.needs_trust(),
            "a long-running process driver counts"
        );

        // `clean =` (empty) switches a driver OFF; nothing would run.
        let disabled = parse_filter_config(&z(&[("local", "f", "filter.d.clean", Some(""))]))
            .unwrap()
            .remove(0);
        assert!(!disabled.needs_trust());

        // A value that is only whitespace is NOT "no program" to git: it is a
        // command name (here a no-break space, which `trim()` would strip).
        let blank = parse_filter_config(&z(&[("local", "f", "filter.d.clean", Some("\u{a0}"))]))
            .unwrap()
            .remove(0);
        assert!(blank.needs_trust());

        // A key without a value at all is not a program either.
        let valueless = parse_filter_config(&z(&[("local", "f", "filter.d.clean", None)]))
            .unwrap()
            .remove(0);
        assert!(!valueless.needs_trust());
    }

    #[test]
    fn values_and_names_with_newlines_and_spaces_survive() {
        let out = z(&[(
            "local",
            ".git/config",
            "filter.Mixed Case.clean",
            Some("line1\nline2 x"),
        )]);
        let d = parse_filter_config(&out).unwrap().remove(0);
        assert_eq!(d.name, "Mixed Case");
        assert_eq!(d.clean.unwrap().value, "line1\nline2 x");
    }

    #[test]
    fn unrelated_and_nameless_keys_are_ignored() {
        let out = z(&[
            ("local", "f", "filter.clean", Some("x")), // no driver name
            ("local", "f", "filter.d.unknownvar", Some("x")),
        ]);
        let drivers = parse_filter_config(&out).unwrap();
        // The second creates the (empty) driver entry but sets nothing.
        assert!(drivers.iter().all(|d| !d.needs_trust()));
    }

    #[test]
    fn truncated_and_non_utf8_output_is_an_error() {
        assert!(parse_filter_config(b"local\0file:.git/config\0").is_err());
        assert!(parse_filter_config(b"local\0file:f\0filter.d.clean\n\xff\0").is_err());
    }

    #[test]
    fn fingerprint_tracks_command_scope_and_other_settings() {
        let base = |value: &str, scope: &str| {
            parse_filter_config(&z(&[(scope, "f", "filter.d.clean", Some(value))]))
                .unwrap()
                .remove(0)
        };
        let a = base("prog", "local");
        assert_eq!(a.fingerprint(), base("prog", "local").fingerprint());
        assert_ne!(a.fingerprint(), base("prog2", "local").fingerprint());
        assert_ne!(a.fingerprint(), base("prog", "worktree").fingerprint());
        // Origin path is not part of it.
        let moved = parse_filter_config(&z(&[(
            "local",
            "/abs/.git/config",
            "filter.d.clean",
            Some("prog"),
        )]))
        .unwrap()
        .remove(0);
        assert_eq!(a.fingerprint(), moved.fingerprint());
        // Another variable changing also changes it.
        let with_process = parse_filter_config(&z(&[
            ("local", "f", "filter.d.clean", Some("prog")),
            ("local", "f", "filter.d.process", Some("p")),
        ]))
        .unwrap()
        .remove(0);
        assert_ne!(a.fingerprint(), with_process.fingerprint());
    }

    #[test]
    fn set_fingerprint_ignores_order_and_duplicates() {
        let out = z(&[
            ("local", "f", "filter.a.clean", Some("1")),
            ("local", "f", "filter.b.clean", Some("2")),
        ]);
        let ds = parse_filter_config(&out).unwrap();
        let reversed = [ds[1].clone(), ds[0].clone(), ds[0].clone()];
        assert_eq!(set_fingerprint(&ds), set_fingerprint(&reversed));
        assert_ne!(set_fingerprint(&ds[..1]), set_fingerprint(&ds));
        assert_eq!(set_fingerprint(&[]), "");
    }

    #[test]
    fn authorize_blocks_untrusted_and_lets_trusted_and_global_through() {
        let out = z(&[
            (
                "global",
                "/h/.gitconfig",
                "filter.lfs.clean",
                Some("git-lfs clean"),
            ),
            ("local", ".git/config", "filter.evil.clean", Some("./x")),
        ]);
        let ds = parse_filter_config(&out).unwrap();
        let wt = Path::new("/r");

        let err = authorize(wt, &ds, &HashSet::new()).unwrap_err();
        let GitError::UntrustedFilters { drivers } = err else {
            panic!("wrong error");
        };
        assert_eq!(drivers.len(), 1);
        assert_eq!(drivers[0].name, "evil");

        let evil = ds.iter().find(|d| d.name == "evil").unwrap();
        let trusted: HashSet<String> = [evil.fingerprint()].into();
        let permit = authorize(wt, &ds, &trusted).unwrap();
        assert_eq!(permit.worktree(), wt);

        // A changed command is no longer the trusted one.
        let changed = parse_filter_config(&z(&[(
            "local",
            ".git/config",
            "filter.evil.clean",
            Some("./y"),
        )]))
        .unwrap();
        assert!(authorize(wt, &changed, &trusted).is_err());
    }

    #[test]
    fn attributes_reference_is_a_token_match_ignoring_comments() {
        let text = "# filter=lfs in a comment\n*.psd filter=lfs diff=lfs -text\n*.rs text\n";
        assert!(attributes_reference(text, "lfs"));
        assert!(!attributes_reference(text, "lf"));
        assert!(!attributes_reference("# filter=lfs\n", "lfs"));
        assert!(!attributes_reference("* filter=lfsx\n", "lfs"));
    }
}
