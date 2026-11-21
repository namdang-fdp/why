//! Claude `Write` / `Edit` → [`EditOp`] (v0.3 line provenance, WHY-65).
//!
//! An op is read ONLY from the `tool_response` of a successful `PostToolUse`:
//! that object is the harness's own report of what it did to the file
//! (`structuredPatch` + `originalFile`, payloads.md §2 fact 1). `tool_input` is
//! what the model ASKED for; a user can edit the proposal in the permission
//! prompt (`userModified`), so the input is never used as evidence.
//!
//! The rule for anything unexpected is the same everywhere in this file: the
//! whole op becomes `Unobserved(Unparsed)`, with no hunks. A partly understood
//! edit is never turned into a partial guess (decision 29): the engine would
//! rather mark the lines `Unknown` than attribute them wrongly.
//!
//! # Decisions where the payload leaves room
//!
//! - **`\ No newline at end of file`** is a marker, not a line of the file, so
//!   it is skipped and never hashed. Whether the file ends in a newline is
//!   already carried by the fingerprints made from `originalFile` / `content`
//!   (`final_newline`); the marker adds nothing a fingerprint lacks. Any other
//!   line starting with `\` is garbage. (No fixture contains the marker: its
//!   exact shape in a real payload is not verified.)
//! - **`userModified: true`** on a `Write`: `after` stays `None` and a created
//!   file is `Unparsed`, because we cannot prove the response's `content` is
//!   what reached the disk. An `Edit`/overwrite keeps its `structuredPatch`,
//!   which the harness computed from the real before/after.
//! - **Counts are checked.** A hunk's `oldLines` / `newLines` must equal its
//!   context + removed / context + added lines, or the patch is garbage.
//! - **CRLF**: one trailing `\r` per line is dropped before hashing (see
//!   [`crate::adapters::hash_edit_line`]).

use serde_json::Value;

use crate::adapters::hash_edit_line;
use crate::provenance::{
    EditHunk, EditKind, EditOp, FileSnapshot, HunkLineHash, HunkLineKind, LineHasher, Observed,
    UnobservedReason,
};

/// What the harness told us the file looked like after the edit, as ops want it.
struct Read {
    kind: EditKind,
    before: Option<FileSnapshot>,
    after: Option<crate::provenance::FileFingerprint>,
    hunks: Vec<EditHunk>,
}

/// An op that records "this file was edited" and nothing else.
///
/// `kind` is `Update`: the payload may not say whether the file was created, and
/// the engine must not rely on it for an unobserved op (see [`EditOp::kind`]).
pub fn unobserved(path: &str, reason: UnobservedReason) -> EditOp {
    EditOp {
        worktree_id: None,
        path: path.to_owned(),
        kind: EditKind::Update,
        renamed_from: None,
        before: None,
        after: None,
        hunks: Vec::new(),
        observed: Observed::Unobserved(reason),
    }
}

/// The op of a successful `PostToolUse` whose payload arrived whole.
/// `path` is the file the harness says it edited (absolute, not yet resolved).
pub fn from_response(tool_name: &str, path: &str, response: &Value) -> EditOp {
    match read(tool_name, response) {
        Some(r) => EditOp {
            worktree_id: None,
            path: path.to_owned(),
            kind: r.kind,
            renamed_from: None,
            before: r.before,
            after: r.after,
            hunks: r.hunks,
            observed: Observed::Exact,
        },
        None => unobserved(path, UnobservedReason::Unparsed),
    }
}

fn read(tool_name: &str, response: &Value) -> Option<Read> {
    let obj = response.as_object()?;
    let hasher = LineHasher::default();
    let patch = obj.get("structuredPatch")?.as_array()?;
    let user_modified = obj.get("userModified") == Some(&Value::Bool(true));
    let kind = obj.get("type").and_then(Value::as_str);

    match (tool_name, kind) {
        // A created file: Claude sends an EMPTY patch, so the one hunk of added
        // lines is made from `content` here.
        ("Write", Some("create")) => {
            let content = obj.get("content")?.as_str()?;
            let no_original = obj.get("originalFile").is_none_or(Value::is_null);
            if !patch.is_empty() || !no_original || user_modified {
                return None;
            }
            let lines: Vec<HunkLineHash> = hasher
                .hash_lines(content.as_bytes())
                .into_iter()
                .map(|hash| HunkLineHash {
                    kind: HunkLineKind::Added,
                    hash,
                })
                .collect();
            Some(Read {
                kind: EditKind::Create,
                before: None,
                after: Some(hasher.fingerprint(content.as_bytes())),
                // An empty file has no hunk (zero lines, nothing to place).
                hunks: if lines.is_empty() {
                    Vec::new()
                } else {
                    vec![EditHunk {
                        old_start: None,
                        header: None,
                        lines,
                    }]
                },
            })
        }
        // An overwrite (`Write`, type "update") or an `Edit` (no `type`).
        ("Write", Some("update")) | ("Edit", None) => {
            let original = obj.get("originalFile")?.as_str()?;
            let after = if tool_name == "Write" {
                let content = obj.get("content")?.as_str()?;
                (!user_modified).then(|| hasher.fingerprint(content.as_bytes()))
            } else {
                None
            };
            Some(Read {
                kind: EditKind::Update,
                before: Some(FileSnapshot::of_text(&hasher, original.as_bytes())),
                after,
                hunks: hunks_of(&hasher, patch)?,
            })
        }
        _ => None,
    }
}

/// The unified-diff note that the line before it has no line terminator.
const NO_NEWLINE_MARKER: &str = "\\ No newline at end of file";

fn count(field: &Value, key: &str) -> Option<u32> {
    u32::try_from(field.get(key)?.as_u64()?).ok()
}

/// Every hunk of a `structuredPatch`, or `None` if any is not understood.
fn hunks_of(hasher: &LineHasher, patch: &[Value]) -> Option<Vec<EditHunk>> {
    patch
        .iter()
        .map(|hunk| {
            let old_start = count(hunk, "oldStart")?;
            let (old_lines, new_lines) = (count(hunk, "oldLines")?, count(hunk, "newLines")?);
            let raw = hunk.get("lines")?.as_array()?;
            let (mut context, mut removed, mut added) = (0u32, 0u32, 0u32);
            let mut lines = Vec::with_capacity(raw.len());
            for (i, line) in raw.iter().enumerate() {
                let line = line.as_str()?;
                let kind = match line.as_bytes().first()? {
                    b' ' => HunkLineKind::Context,
                    b'-' => HunkLineKind::Removed,
                    b'+' => HunkLineKind::Added,
                    // The "no newline" marker belongs to the line before it, so
                    // it cannot be first; it is skipped, never hashed.
                    b'\\' if i > 0 && line == NO_NEWLINE_MARKER => continue,
                    // No known prefix: the whole op is unreadable.
                    _ => return None,
                };
                match kind {
                    HunkLineKind::Context => context += 1,
                    HunkLineKind::Removed => removed += 1,
                    HunkLineKind::Added => added += 1,
                }
                // `[1..]`: the prefix is one ASCII byte, so this is a boundary.
                let text = &line[1..];
                // A line followed by the marker has no terminator, so a `\r`
                // at its end is CONTENT (like `split_lines` for the last line
                // of a file without a final newline), not half of a CRLF.
                let unterminated = raw
                    .get(i + 1)
                    .and_then(Value::as_str)
                    .is_some_and(|next| next == NO_NEWLINE_MARKER);
                lines.push(HunkLineHash {
                    kind,
                    hash: if unterminated {
                        hasher.hash_line(text.as_bytes())
                    } else {
                        hash_edit_line(hasher, text)
                    },
                });
            }
            (context + removed == old_lines && context + added == new_lines).then_some(EditHunk {
                old_start: Some(old_start),
                header: None,
                lines,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::provenance::{hash_lines, split_lines};

    fn h(text: &str) -> crate::provenance::LineHash {
        LineHasher::default().hash_line(text.as_bytes())
    }

    fn update(original: &str, hunks: Value) -> Value {
        json!({ "originalFile": original, "structuredPatch": hunks, "userModified": false })
    }

    #[test]
    fn an_edit_with_two_hunks_keeps_positions_and_patch_order() {
        let original = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let patch = json!([
            { "oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 3,
              "lines": [" a", "-b", "+B", " c"] },
            { "oldStart": 6, "oldLines": 3, "newStart": 6, "newLines": 4,
              "lines": [" f", " g", "+G2", " h"] },
        ]);
        let op = from_response("Edit", "/r/x.rs", &update(original, patch));
        assert_eq!(op.observed, Observed::Exact);
        assert_eq!(op.kind, EditKind::Update);
        assert_eq!(op.hunks.len(), 2);
        assert_eq!(op.hunks[0].old_start, Some(1));
        assert_eq!(op.hunks[1].old_start, Some(6));
        let kinds: Vec<_> = op.hunks[0].lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                HunkLineKind::Context,
                HunkLineKind::Removed,
                HunkLineKind::Added,
                HunkLineKind::Context
            ]
        );
        assert_eq!(op.hunks[0].lines[2].hash, h("B"));
        assert_eq!(op.before.unwrap().lines, hash_lines(original.as_bytes()));
        assert!(op.after.is_none(), "Edit carries no whole new file");
    }

    #[test]
    fn hunk_hashes_agree_with_the_original_file_at_old_start() {
        // The cross-check of the brief: Context/Removed lines of a hunk ARE the
        // lines of `originalFile` from `oldStart`, hashed by the same policy.
        let original = "one\ntwo\nthree\nfour\nfive\n";
        let patch = json!([{ "oldStart": 2, "oldLines": 3, "newStart": 2, "newLines": 3,
            "lines": [" two", "-three", "+3", " four"] }]);
        let op = from_response("Edit", "/r/x", &update(original, patch));
        let before = op.before.as_ref().unwrap();
        let hunk = &op.hunks[0];
        let from_file = &before.lines[(hunk.old_start.unwrap() as usize - 1)..];
        let old_side: Vec<_> = hunk
            .lines
            .iter()
            .filter(|l| l.kind != HunkLineKind::Added)
            .map(|l| l.hash)
            .collect();
        assert_eq!(&from_file[..old_side.len()], &old_side[..]);
    }

    #[test]
    fn write_create_is_one_added_hunk_without_a_position() {
        let response = json!({ "type": "create", "filePath": "/r/n.py", "content": "x = 1\ny = 2\n",
            "structuredPatch": [], "originalFile": null, "userModified": false });
        let op = from_response("Write", "/r/n.py", &response);
        assert_eq!(op.kind, EditKind::Create);
        assert_eq!(op.observed, Observed::Exact);
        assert!(op.before.is_none());
        assert_eq!(op.hunks.len(), 1);
        assert_eq!(op.hunks[0].old_start, None);
        assert!(
            op.hunks[0]
                .lines
                .iter()
                .all(|l| l.kind == HunkLineKind::Added)
        );
        assert_eq!(op.hunks[0].lines[1].hash, h("y = 2"));
        let after = op.after.unwrap();
        assert_eq!(after.line_count, 2);
        assert!(after.final_newline);

        // An empty file: created, zero lines, no hunk.
        let empty =
            json!({ "type": "create", "content": "", "structuredPatch": [], "originalFile": null });
        let op = from_response("Write", "/r/e", &empty);
        assert_eq!(op.kind, EditKind::Create);
        assert!(op.hunks.is_empty());
        assert_eq!(op.after.unwrap().line_count, 0);
    }

    #[test]
    fn write_overwrite_has_before_hunks_and_after() {
        let response = json!({ "type": "update", "content": "a\nB\n", "originalFile": "a\nb\n",
            "structuredPatch": [{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
                "lines": [" a", "-b", "+B"] }], "userModified": false });
        let op = from_response("Write", "/r/f", &response);
        assert_eq!(op.observed, Observed::Exact);
        assert_eq!(op.kind, EditKind::Update);
        assert_eq!(op.before.unwrap().fingerprint.line_count, 2);
        assert_eq!(
            op.after.unwrap(),
            LineHasher::default().fingerprint(b"a\nB\n")
        );
    }

    #[test]
    fn crlf_original_and_patch_hash_like_lf() {
        let lf = from_response(
            "Edit",
            "/r/f",
            &update(
                "a\nb\n",
                json!([{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
                    "lines": [" a", "-b", "+c"] }]),
            ),
        );
        let crlf = from_response(
            "Edit",
            "/r/f",
            &update(
                "a\r\nb\r\n",
                json!([{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
                    "lines": [" a\r", "-b\r", "+c\r"] }]),
            ),
        );
        assert_eq!(crlf.hunks, lf.hunks);
        assert_eq!(crlf.before, lf.before);
    }

    #[test]
    fn the_no_newline_marker_is_skipped_not_hashed() {
        let patch = json!([{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
            "lines": [" a", "-b", "\\ No newline at end of file", "+c",
                      "\\ No newline at end of file"] }]);
        let op = from_response("Edit", "/r/f", &update("a\nb", patch));
        assert_eq!(op.observed, Observed::Exact);
        assert_eq!(op.hunks[0].lines.len(), 3, "context, removed, added");
        assert!(!op.before.unwrap().fingerprint.final_newline);
        // A backslash line that is not the marker, or one that comes first: garbage.
        for lines in [
            json!(["\\ something else"]),
            json!(["\\ No newline at end of file", "+x"]),
        ] {
            let patch = json!([{ "oldStart": 1, "oldLines": 0, "newStart": 1, "newLines": 1, "lines": lines }]);
            let op = from_response("Edit", "/r/f", &update("", patch));
            assert_eq!(
                op.observed,
                Observed::Unobserved(UnobservedReason::Unparsed)
            );
        }
    }

    #[test]
    fn a_lone_cr_before_the_no_newline_marker_is_content_like_split_lines() {
        // The file's last line is `b\r` with NO terminator: `split_lines` keeps
        // the `\r` as content, so the hunk's hash must keep it too.
        let original = "a\nb\r";
        let patch = json!([{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
            "lines": [" a", "-b\r", "\\ No newline at end of file", "+c"] }]);
        let op = from_response("Edit", "/r/f", &update(original, patch));
        assert_eq!(op.observed, Observed::Exact);
        let before = FileSnapshot::of_text(&LineHasher::default(), original.as_bytes());
        assert_eq!(op.hunks[0].lines[1].hash, before.lines[1]);
        assert_ne!(
            op.hunks[0].lines[1].hash,
            h("b"),
            "the `\\r` is part of the line"
        );

        // Without the marker the same `\r` is half of a CRLF: stripped.
        let patch = json!([{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
            "lines": [" a", "-b\r", "+c"] }]);
        let op = from_response("Edit", "/r/f", &update("a\nb\r\n", patch));
        let before = FileSnapshot::of_text(&LineHasher::default(), b"a\nb\r\n");
        assert_eq!(op.hunks[0].lines[1].hash, before.lines[1]);
        assert_eq!(op.hunks[0].lines[1].hash, h("b"));
    }

    #[test]
    fn user_modified_write_keeps_the_patch_but_not_the_after() {
        let overwrite = json!({ "type": "update", "content": "a\nB\n", "originalFile": "a\nb\n",
            "userModified": true,
            "structuredPatch": [{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
                "lines": [" a", "-b", "+B"] }] });
        let op = from_response("Write", "/r/f", &overwrite);
        assert_eq!(op.observed, Observed::Exact);
        assert!(op.after.is_none());
        assert_eq!(op.hunks.len(), 1);

        // A created file is built from `content`, which cannot be trusted then.
        let create = json!({ "type": "create", "content": "x\n", "structuredPatch": [],
            "originalFile": null, "userModified": true });
        let op = from_response("Write", "/r/n", &create);
        assert_eq!(
            op.observed,
            Observed::Unobserved(UnobservedReason::Unparsed)
        );
        assert!(op.hunks.is_empty());
    }

    #[test]
    fn garbage_is_unparsed_never_a_partial_guess() {
        let good = json!({ "oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1,
            "lines": ["-a", "+b"] });
        let counts_wrong = json!({ "oldStart": 1, "oldLines": 5, "newStart": 1, "newLines": 1,
            "lines": ["-a", "+b"] });
        let no_prefix = json!({ "oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1,
            "lines": ["-a", "b"] });
        let empty_line = json!({ "oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1,
            "lines": ["-a", ""] });
        let bad_start = json!({ "oldStart": -1, "oldLines": 1, "newStart": 1, "newLines": 1,
            "lines": ["-a", "+b"] });
        let not_text = json!({ "oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1,
            "lines": ["-a", 7] });
        // One bad hunk spoils the whole op, even after a good one.
        for bad in [counts_wrong, no_prefix, empty_line, bad_start, not_text] {
            let op = from_response("Edit", "/r/f", &update("a\n", json!([good.clone(), bad])));
            assert_eq!(
                op.observed,
                Observed::Unobserved(UnobservedReason::Unparsed)
            );
            assert!(op.hunks.is_empty() && op.before.is_none() && op.after.is_none());
        }
        for response in [
            json!("a string"),
            json!(null),
            json!({}),
            json!({ "structuredPatch": "nope", "originalFile": "x" }),
            json!({ "structuredPatch": [], "originalFile": null }),
            json!({ "type": "update", "structuredPatch": [], "originalFile": "x" }),
            json!({ "type": "weird", "content": "x", "structuredPatch": [], "originalFile": null }),
        ] {
            let op = from_response("Write", "/r/f", &response);
            assert_eq!(
                op.observed,
                Observed::Unobserved(UnobservedReason::Unparsed)
            );
        }
        // A tool this module does not read.
        let op = from_response(
            "NotebookEdit",
            "/r/n.ipynb",
            &json!({ "structuredPatch": [] }),
        );
        assert_eq!(
            op.observed,
            Observed::Unobserved(UnobservedReason::Unparsed)
        );
        assert_eq!(op.path, "/r/n.ipynb");
    }

    #[test]
    fn lines_of_a_split_file_match_what_the_hunks_hash() {
        // Sanity for the CRLF policy shared with `provenance::split_lines`.
        assert_eq!(split_lines(b"a\r\n").next(), Some(&b"a"[..]));
        assert_eq!(hash_edit_line(&LineHasher::default(), "a\r"), h("a"));
        assert_ne!(hash_edit_line(&LineHasher::default(), "a\r\r"), h("a"));
    }
}
