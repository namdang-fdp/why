//! Pure extractors for the facts the adapter reads out of Claude payloads.
//!
//! `NormalizedEvent` has one free-text field and no slot for "this prompt was
//! synthetic" or "this edit created a file", so these facts are exposed here as
//! typed values (and tested), and the adapter renders what it can into the
//! event's `text`. A later model change (see the WHY-14 report) can carry them
//! as real fields without re-parsing payloads.
//!
//! Everything here is a pure function of `serde_json::Value`s and tolerates any
//! shape: a payload comes from another program and may be anything.

use serde_json::Value;

/// A string field, or `None` if absent or not a string.
pub fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// `str_field`, but an empty string counts as absent.
pub fn non_empty<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    str_field(v, key).filter(|s| !s.is_empty())
}

/// The file an edit-like tool targets: `file_path` (Write/Edit) or
/// `notebook_path` (NotebookEdit).
pub fn target_path(tool_input: &Value) -> Option<&str> {
    non_empty(tool_input, "file_path").or_else(|| non_empty(tool_input, "notebook_path"))
}

/// A synthetic prompt is one the human did not type: after an async subagent
/// finishes, Claude Code feeds a `<task-notification>…` block to the main agent
/// as a `UserPromptSubmit` (payloads.md §2, fact 6). The UI should say "not
/// typed by you" for it.
pub fn is_synthetic_prompt(prompt: &str) -> bool {
    prompt.trim_start().starts_with("<task-notification>")
}

/// The exit code in a `PostToolUseFailure.error` ("Exit code 3\n…"), when the
/// text starts with that marker. `None` for an interrupt or any other failure.
/// Only ever read from the failure event, never guessed from output text.
pub fn exit_code(error: &str) -> Option<u32> {
    let rest = error.strip_prefix("Exit code ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// First line of `text`, trimmed.
pub fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

/// What a `tool_response` string looks like when the harness refused to run
/// the call (payloads.md §2, fact 4): `<tool_use_error>…</tool_use_error>`.
pub fn tool_use_error(response: &Value) -> Option<&str> {
    let s = response.as_str()?.trim_start();
    let inner = s.strip_prefix("<tool_use_error>")?;
    Some(inner.strip_suffix("</tool_use_error>").unwrap_or(inner))
}

/// How an edit tool changed its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// `Write` to a path that did not exist (`type: "create"`): an add.
    Create,
    /// `Write` over an existing file (`type: "update"`).
    Update,
    /// `Edit`/`NotebookEdit`/anything else with a `structuredPatch`.
    Edit,
}

/// One unified-diff hunk from `structuredPatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Lines with their `" "`, `"-"`, `"+"` prefix, exactly as reported.
    pub lines: Vec<String>,
}

/// The Observed evidence of a successful edit, from `PostToolUse.tool_response`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditDetails {
    pub kind: EditKind,
    pub path: Option<String>,
    pub hunks: Vec<Hunk>,
    /// The harness reported the file's previous content (`originalFile` is a
    /// string): the pre-image exists, so the edit can be replayed exactly.
    pub had_original: bool,
    pub added: u32,
    pub removed: u32,
}

fn u32_field(v: &Value, key: &str) -> u32 {
    v.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

fn count_lines(text: &str) -> u32 {
    u32::try_from(text.lines().count()).unwrap_or(u32::MAX)
}

/// Read the edit evidence out of a successful edit's `tool_response`.
/// `None` when the response is not an object (e.g. the harness gave a string).
pub fn edit_details(tool_input: &Value, response: &Value) -> Option<EditDetails> {
    if !response.is_object() {
        return None;
    }
    let kind = match str_field(response, "type") {
        Some("create") => EditKind::Create,
        Some("update") => EditKind::Update,
        _ => EditKind::Edit,
    };
    let hunks: Vec<Hunk> = response
        .get("structuredPatch")
        .and_then(Value::as_array)
        .map(|hs| {
            hs.iter()
                .filter(|h| h.is_object())
                .map(|h| Hunk {
                    old_start: u32_field(h, "oldStart"),
                    old_lines: u32_field(h, "oldLines"),
                    new_start: u32_field(h, "newStart"),
                    new_lines: u32_field(h, "newLines"),
                    lines: h
                        .get("lines")
                        .and_then(Value::as_array)
                        .map(|ls| {
                            ls.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();

    let mut added = 0u32;
    let mut removed = 0u32;
    for line in hunks.iter().flat_map(|h| &h.lines) {
        match line.as_bytes().first() {
            Some(b'+') => added = added.saturating_add(1),
            Some(b'-') => removed = removed.saturating_add(1),
            _ => {}
        }
    }
    // A created file has an empty patch: its content is the addition.
    if kind == EditKind::Create && hunks.is_empty() {
        let content = str_field(response, "content").or_else(|| str_field(tool_input, "content"));
        added = content.map(count_lines).unwrap_or(0);
    }

    Some(EditDetails {
        kind,
        path: str_field(response, "filePath")
            .map(str::to_owned)
            .or_else(|| target_path(tool_input).map(str::to_owned)),
        hunks,
        had_original: response.get("originalFile").is_some_and(Value::is_string),
        added,
        removed,
    })
}

impl EditDetails {
    /// A readable rendering for the timeline row: a header (create / update /
    /// edit, hunk and line counts, whether the old content was recorded) and
    /// then the hunks in unified-diff form. The caller caps it with `excerpt`.
    pub fn render(&self) -> String {
        let kind = match self.kind {
            EditKind::Create => "create",
            EditKind::Update => "update",
            EditKind::Edit => "edit",
        };
        let original = if self.had_original {
            ", old content recorded"
        } else {
            ""
        };
        let mut out = format!(
            "{kind}: {} hunk(s), +{} -{}{original}",
            self.hunks.len(),
            self.added,
            self.removed
        );
        for h in &self.hunks {
            out.push_str(&format!(
                "\n@@ -{},{} +{},{} @@",
                h.old_start, h.old_lines, h.new_start, h.new_lines
            ));
            for line in &h.lines {
                out.push('\n');
                out.push_str(line);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn synthetic_prompts_are_task_notifications_only() {
        assert!(is_synthetic_prompt("<task-notification>\n<task-id>x"));
        assert!(is_synthetic_prompt("  \n<task-notification>"));
        assert!(!is_synthetic_prompt("please fix <task-notification> bug"));
        assert!(!is_synthetic_prompt(""));
    }

    #[test]
    fn exit_codes_come_only_from_the_marker() {
        assert_eq!(exit_code("Exit code 3"), Some(3));
        assert_eq!(exit_code("Exit code 1\nF\nFAIL"), Some(1));
        assert_eq!(exit_code("Exit code "), None);
        assert_eq!(exit_code("exit=3"), None);
        assert_eq!(exit_code("Interrupted"), None);
        // Absurdly large: no panic, no bogus value.
        assert_eq!(exit_code("Exit code 99999999999999999999"), None);
    }

    #[test]
    fn rejection_marker_is_recognised() {
        let r = json!(
            "<tool_use_error>String to replace not found in file.\nString: X</tool_use_error>"
        );
        assert_eq!(
            tool_use_error(&r),
            Some("String to replace not found in file.\nString: X")
        );
        assert_eq!(tool_use_error(&json!("Exit code 1")), None);
        assert_eq!(tool_use_error(&json!({"a": 1})), None);
        assert_eq!(tool_use_error(&Value::Null), None);
    }

    #[test]
    fn create_counts_content_lines_update_reads_the_patch() {
        let create = json!({"type":"create","filePath":"/r/a.py","content":"a\nb\n","structuredPatch":[],"originalFile":null});
        let d = edit_details(&json!({"file_path":"/r/a.py"}), &create).unwrap();
        assert_eq!(d.kind, EditKind::Create);
        assert_eq!((d.added, d.removed, d.had_original), (2, 0, false));
        assert!(d.render().starts_with("create: 0 hunk(s), +2 -0"));

        let update = json!({
            "type":"update","filePath":"/r/a.py","originalFile":"old",
            "structuredPatch":[{"oldStart":1,"oldLines":2,"newStart":1,"newLines":2,
                "lines":[" keep","-old","+new"]}]
        });
        let d = edit_details(&json!({}), &update).unwrap();
        assert_eq!(d.kind, EditKind::Update);
        assert_eq!((d.added, d.removed, d.had_original), (1, 1, true));
        assert_eq!(d.path.as_deref(), Some("/r/a.py"));
        assert!(d.render().contains("@@ -1,2 +1,2 @@\n keep\n-old\n+new"));
    }

    #[test]
    fn edit_details_tolerates_garbage() {
        assert!(edit_details(&json!({}), &json!("string")).is_none());
        let d = edit_details(&json!(7), &json!({"structuredPatch": "nope", "type": 5})).unwrap();
        assert_eq!(d.kind, EditKind::Edit);
        assert!(d.hunks.is_empty());
        let d = edit_details(
            &json!({}),
            &json!({"structuredPatch":[1,{"lines":[1,"+x"],"oldStart":-4}]}),
        )
        .unwrap();
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(d.hunks[0].old_start, 0);
        assert_eq!(d.added, 1);
    }
}
