//! Reduced payloads: what the hook sends instead of a payload it must not (or
//! cannot) forward whole.
//!
//! Two reasons, one shape:
//!
//! * [`Reduction::Truncated`]: the payload does not fit in one wire line
//!   ([`crate::MAX_LINE_BYTES`]), e.g. a Claude `Write` of a big file carries
//!   the whole content twice;
//! * [`Reduction::Redacted`]: the payload touches an agent credential file
//!   (`~/.claude/.credentials.json`, `~/.codex/auth.json`). WHY never reads,
//!   stores or forwards those, and a `Read` of one puts its content in
//!   `tool_response`.
//!
//! The reduced payload keeps only small identifying fields at the SAME top-level
//! keys the harness used (`session_id`, `tool_name`, ...), so the adapters can
//! still place the event on the timeline (which session, which turn, which
//! tool) without knowing anything about reduction. Adapters tell the two apart
//! with [`reduction_of`]: a reduced event has no tool input to show.

use serde_json::{Map, Value};

/// Why a payload was reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reduction {
    Truncated,
    Redacted,
}

impl Reduction {
    fn flag(self) -> &'static str {
        match self {
            Reduction::Truncated => "truncated",
            Reduction::Redacted => "redacted",
        }
    }
}

/// Top-level payload fields that survive reduction: ids and tags, never
/// content. Both harnesses' payloads use these names (payloads.md §2, §3).
const KEPT_KEYS: &[&str] = &[
    "session_id",
    "turn_id",
    "prompt_id",
    "tool_use_id",
    "agent_id",
    "agent_type",
    "hook_event_name",
    "tool_name",
    "cwd",
    "transcript_path",
    "source",
    "reason",
];

/// A kept value longer than this is dropped: ids are short, and a long string
/// under an id-like key is more likely content.
const MAX_KEPT_VALUE_BYTES: usize = 512;

/// Keep `value` under `key` when it is a short string under a kept key.
fn keep(out: &mut Map<String, Value>, key: &str, value: &Value) {
    if let Value::String(s) = value
        && KEPT_KEYS.contains(&key)
        && s.len() <= MAX_KEPT_VALUE_BYTES
    {
        out.insert(key.to_owned(), value.clone());
    }
}

fn marker(mut out: Map<String, Value>, reason: Reduction, original_bytes: usize) -> Value {
    out.insert(reason.flag().to_owned(), Value::Bool(true));
    out.insert(
        "original_bytes".to_owned(),
        Value::from(u64::try_from(original_bytes).unwrap_or(u64::MAX)),
    );
    Value::Object(out)
}

/// The reduced form of `payload`; `original_bytes` is the size of the
/// original payload as received, for display ("1.9 MB payload not captured").
pub fn reduce_payload(payload: &Value, reason: Reduction, original_bytes: usize) -> Value {
    let mut out = Map::new();
    if let Some(obj) = payload.as_object() {
        for (key, value) in obj {
            keep(&mut out, key, value);
        }
    }
    marker(out, reason, original_bytes)
}

/// Like [`reduce_payload`], for a payload too big to parse whole: reads the
/// kept fields from the START of its JSON text (`prefix`), skipping every other
/// value without building it (`IgnoredAny`), so memory stays flat however big
/// the skipped values are. Both harnesses write the ids first (`session_id`,
/// `transcript_path`, `cwd`, `hook_event_name`, `tool_name`, ...), long before
/// the huge `tool_input` / `tool_response`. A prefix that ends mid-value is
/// fine: whatever was read before the cut is kept.
pub fn reduce_prefix(prefix: &[u8], reason: Reduction, original_bytes: u64) -> Value {
    use std::fmt;

    use serde::Deserializer as _;
    use serde::de::{IgnoredAny, MapAccess, Visitor};

    struct Collect<'a>(&'a mut Map<String, Value>);

    impl<'de> Visitor<'de> for Collect<'_> {
        type Value = ();

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a JSON object")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            while let Some(key) = map.next_key::<String>()? {
                if KEPT_KEYS.contains(&key.as_str()) {
                    let value: Value = map.next_value()?;
                    keep(self.0, &key, &value);
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(())
        }
    }

    let mut out = Map::new();
    // The result is deliberately ignored: a cut-off prefix ends in a parse
    // error, and `out` holds everything read before it.
    let _ = serde_json::Deserializer::from_slice(prefix).deserialize_map(Collect(&mut out));
    marker(
        out,
        reason,
        usize::try_from(original_bytes).unwrap_or(usize::MAX),
    )
}

/// Was this payload reduced by the hook, and why?
pub fn reduction_of(payload: &Value) -> Option<Reduction> {
    let obj = payload.as_object()?;
    [Reduction::Truncated, Reduction::Redacted]
        .into_iter()
        .find(|r| obj.get(r.flag()) == Some(&Value::Bool(true)))
}

/// The credential files of THIS user's agents, from the hook's environment:
/// `$CODEX_HOME/auth.json` (default `~/.codex`) and
/// `$CLAUDE_CONFIG_DIR/.credentials.json` (default `~/.claude`). A user who set
/// either variable keeps the file somewhere our fixed literals would never
/// match. Both the path as given and, when it exists, its canonical form are
/// returned. Relative or empty values are ignored (the agent would not have
/// resolved them against our cwd either).
pub fn credential_paths(
    codex_home: Option<&std::ffi::OsStr>,
    claude_config_dir: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Vec<String> {
    use std::path::{Path, PathBuf};
    let abs = |v: Option<&std::ffi::OsStr>| {
        v.map(Path::new)
            .filter(|p| p.is_absolute())
            .map(Path::to_path_buf)
    };
    let home = abs(home);
    let codex = abs(codex_home).or_else(|| home.as_ref().map(|h| h.join(".codex")));
    let claude = abs(claude_config_dir).or_else(|| home.as_ref().map(|h| h.join(".claude")));
    let mut out = Vec::new();
    let candidates: Vec<PathBuf> = [
        codex.map(|d| d.join("auth.json")),
        claude.map(|d| d.join(".credentials.json")),
    ]
    .into_iter()
    .flatten()
    .collect();
    for path in candidates {
        if let Ok(canonical) = std::fs::canonicalize(&path) {
            out.extend(canonical.to_str().map(str::to_owned));
        }
        out.extend(path.to_str().map(str::to_owned));
    }
    out.sort();
    out.dedup();
    out
}

/// Does this raw payload mention an agent credential file?
///
/// A byte search on purpose: no JSON parse is needed to be safe, and it also
/// catches the path inside a `Bash` command, an `apply_patch`, a `Read`
/// response or a prompt. It errs towards redacting (a project file that happens
/// to be called `.credentials.json` loses its detail, nothing more).
///
/// Three kinds of needle: the default file names (`.credentials.json`,
/// `.codex/auth.json`, so `~/.claude/...` and `$HOME/.codex/...` match), the
/// variable spellings (`$CODEX_HOME/auth.json`), and `extra`: the actual paths
/// from [`credential_paths`], searched as they appear INSIDE JSON text (so a
/// path with a quote or backslash is escaped the way the harness wrote it).
/// Not covered: a path built at run time (`cat ~/.cla*/.cred*`); WHY cannot
/// know what a shell command will read.
pub fn mentions_credentials(raw: &[u8], extra: &[String]) -> bool {
    const NEEDLES: &[&[u8]] = &[
        b".credentials.json",
        b".codex/auth.json",
        b".codex\\/auth.json",
        b".codex\\\\auth.json",
        b"CODEX_HOME/auth.json",
        b"CODEX_HOME}/auth.json",
    ];
    let has = |needle: &[u8]| !needle.is_empty() && raw.windows(needle.len()).any(|w| w == needle);
    NEEDLES.iter().any(|n| has(n))
        || extra.iter().any(|path| {
            // JSON-escape, drop the surrounding quotes; also try the `\/` form
            // some encoders use for slashes.
            let json = serde_json::to_string(path).unwrap_or_default();
            let inner = json.trim_matches('"');
            has(inner.as_bytes()) || has(inner.replace('/', "\\/").as_bytes())
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keeps_ids_and_drops_content() {
        let payload = json!({
            "session_id": "s1", "hook_event_name": "PostToolUse", "tool_name": "Write",
            "tool_use_id": "t1", "cwd": "/repo",
            "tool_input": { "file_path": "/repo/a", "content": "x".repeat(2_000_000) },
            "tool_response": { "originalFile": "secret" },
            "prompt": "do not keep me",
            "agent_id": 42,
            "reason": "y".repeat(MAX_KEPT_VALUE_BYTES + 1),
        });
        let r = reduce_payload(&payload, Reduction::Truncated, 2_000_123);
        assert_eq!(
            r,
            json!({
                "session_id": "s1", "hook_event_name": "PostToolUse", "tool_name": "Write",
                "tool_use_id": "t1", "cwd": "/repo",
                "truncated": true, "original_bytes": 2_000_123,
            })
        );
        assert_eq!(reduction_of(&r), Some(Reduction::Truncated));
        assert_eq!(reduction_of(&payload), None);
    }

    #[test]
    fn a_cut_off_prefix_still_yields_its_ids() {
        let full = format!(
            r#"{{"session_id":"s9","cwd":"/r","hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{{"content":"{}"}},"tool_use_id":"late"}}"#,
            "x".repeat(100_000)
        );
        // Whole text: the id AFTER the big value is found too.
        let r = reduce_prefix(full.as_bytes(), Reduction::Truncated, 5);
        assert_eq!(r["session_id"], "s9");
        assert_eq!(r["tool_use_id"], "late");
        assert_eq!(r["original_bytes"], 5);
        // Cut in the middle of the big value: ids before the cut survive.
        let r = reduce_prefix(&full.as_bytes()[..50_000], Reduction::Truncated, 7);
        assert_eq!(r["session_id"], "s9");
        assert_eq!(r["tool_name"], "Write");
        assert!(r.get("tool_use_id").is_none() && r.get("tool_input").is_none());
        assert_eq!(reduction_of(&r), Some(Reduction::Truncated));
        // Garbage in, marker out.
        assert_eq!(
            reduce_prefix(b"\xff not json", Reduction::Truncated, 1),
            json!({ "truncated": true, "original_bytes": 1 })
        );
    }

    #[test]
    fn a_non_object_payload_reduces_to_the_marker_only() {
        let r = reduce_payload(&json!([1, 2]), Reduction::Redacted, 5);
        assert_eq!(r, json!({ "redacted": true, "original_bytes": 5 }));
        assert_eq!(reduction_of(&r), Some(Reduction::Redacted));
    }

    #[test]
    fn configured_credential_homes_are_recognised() {
        use std::ffi::OsStr;
        let tmp = tempfile::tempdir().unwrap();
        let odd = tmp.path().join("my \"odd\" codex");
        std::fs::create_dir_all(&odd).unwrap();
        std::fs::write(odd.join("auth.json"), "{}").unwrap();
        let paths = credential_paths(
            Some(odd.as_os_str()),
            Some(OsStr::new("/cfg/claude-x")),
            Some(OsStr::new("/home/u")),
        );
        assert!(paths.contains(&odd.join("auth.json").to_string_lossy().into_owned()));
        assert!(paths.contains(&"/cfg/claude-x/.credentials.json".to_owned()));
        // The defaults under HOME are not listed when the variables override them.
        assert!(!paths.iter().any(|p| p.starts_with("/home/u")));

        // As the harness would have JSON-encoded them (quotes escaped).
        let payload = serde_json::json!({"tool_input":{"file_path": odd.join("auth.json")}});
        let raw = serde_json::to_vec(&payload).unwrap();
        assert!(mentions_credentials(&raw, &paths));
        assert!(
            !mentions_credentials(&raw, &[]),
            "no literal matches an oddly named home"
        );
        let claude = br#"{"file_path":"/cfg/claude-x/.credentials.json"}"#;
        assert!(mentions_credentials(claude, &paths));

        // Defaults come from HOME; relative values are ignored.
        let d = credential_paths(None, None, Some(OsStr::new("/home/u")));
        assert_eq!(
            d,
            [
                "/home/u/.claude/.credentials.json",
                "/home/u/.codex/auth.json"
            ]
        );
        assert!(credential_paths(Some(OsStr::new("rel")), None, None).is_empty());
        // Unrelated files do not match.
        assert!(!mentions_credentials(
            br#"{"file_path":"/cfg/claude-x/settings.json"}"#,
            &paths
        ));
    }

    #[test]
    fn credential_paths_are_recognised_in_any_field() {
        for hit in [
            r#"{"tool_input":{"file_path":"/home/u/.claude/.credentials.json"}}"#,
            r#"{"tool_input":{"command":"cat ~/.codex/auth.json"}}"#,
            r#"{"x":"C:\\Users\\u\\.codex\\auth.json"}"#,
            r#"{"x":"~/.codex\/auth.json"}"#,
            r#"{"command":"cat $CODEX_HOME/auth.json"}"#,
        ] {
            assert!(mentions_credentials(hit.as_bytes(), &[]), "{hit}");
        }
        for miss in [
            r#"{"tool_input":{"file_path":"/repo/auth.json"}}"#,
            r#"{"prompt":"fix the login"}"#,
            "",
        ] {
            assert!(!mentions_credentials(miss.as_bytes(), &[]), "{miss}");
        }
    }
}
