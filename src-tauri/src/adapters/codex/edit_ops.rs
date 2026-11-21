//! Codex `apply_patch` → [`EditOp`]s (v0.3 line provenance, WHY-65).
//!
//! One op per file section of the patch, in patch order. Codex reports no
//! positions and no old file, so hunks have `old_start: None`, and `before` /
//! `after` are `None`: the replay finds each hunk by its context (decision 30).
//!
//! The caller creates ops only for a `PostToolUse` whose outcome is `Ok`: a
//! rejected patch fires no Post at all (payloads.md §3 fact 2) and a failed one
//! says `Exit code: N`, and neither changed any file.
//!
//! A section the parser flagged ([`FileEdit::flawed`]) becomes
//! `Unobserved(Unparsed)` for THAT file only; the other sections of the same
//! patch stay exact. A section without a path produces no op (an op needs a
//! file to belong to).

use super::patch::{FileEdit, HunkLine as PatchLine, PatchOp, parse_patch};
use crate::adapters::hash_edit_line;
use crate::provenance::{
    EditHunk, EditKind, EditOp, HunkLineHash, HunkLineKind, LineHasher, Observed, UnobservedReason,
};

/// The ops of the patch text in `tool_input.command`. Paths are made absolute
/// with `absolutize` (a relative one is relative to the event's cwd).
pub fn edit_ops(patch_text: &str, absolutize: impl Fn(&str) -> String) -> Vec<EditOp> {
    let hasher = LineHasher::default();
    parse_patch(patch_text)
        .edits
        .iter()
        .filter(|e| !e.path.is_empty())
        .map(|e| op_of(&hasher, e, &absolutize))
        .collect()
}

fn op_of(hasher: &LineHasher, edit: &FileEdit, absolutize: &impl Fn(&str) -> String) -> EditOp {
    // `*** Move to:` renames: the file now lives at the NEW path.
    let (path, renamed_from) = match edit.move_to.as_deref().filter(|to| !to.is_empty()) {
        Some(to) => (absolutize(to), Some(absolutize(&edit.path))),
        None => (absolutize(&edit.path), None),
    };
    let kind = match edit.op {
        PatchOp::Add => EditKind::Create,
        PatchOp::Update => EditKind::Update,
        PatchOp::Delete => EditKind::Delete,
    };
    let op = |observed, hunks| EditOp {
        worktree_id: None,
        path: path.clone(),
        kind,
        renamed_from: renamed_from.clone(),
        before: None,
        after: None,
        hunks,
        observed,
    };
    let unparsed = || op(Observed::Unobserved(UnobservedReason::Unparsed), Vec::new());

    if edit.flawed {
        return unparsed();
    }
    // A section's lines must fit its operation: an added file is only `+`
    // lines, a deleted one has no body at all. Anything else is not a patch
    // we understand, and is not guessed at.
    let fits = match edit.op {
        PatchOp::Add => edit
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .all(|l| matches!(l, PatchLine::Added(_))),
        PatchOp::Delete => edit.hunks.is_empty(),
        PatchOp::Update => true,
    };
    if !fits {
        return unparsed();
    }

    let hunks = match edit.op {
        // A created file is ONE hunk of added lines (an empty file has none).
        PatchOp::Add => {
            let lines: Vec<HunkLineHash> = edit
                .hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .map(|l| hash_line(hasher, l))
                .collect();
            if lines.is_empty() {
                Vec::new()
            } else {
                vec![EditHunk {
                    old_start: None,
                    header: None,
                    lines,
                }]
            }
        }
        PatchOp::Delete => Vec::new(),
        PatchOp::Update => edit
            .hunks
            .iter()
            .map(|h| EditHunk {
                old_start: None,
                // The `@@ text` is a locating hint, hashed like a line. The
                // parser has already trimmed it, so for an indented source line
                // the header will not equal that line's hash: the engine must
                // read "no match" as "no hint", never as a mismatch.
                header: h.header.as_deref().map(|text| hash_edit_line(hasher, text)),
                lines: h.lines.iter().map(|l| hash_line(hasher, l)).collect(),
            })
            .collect(),
    };
    op(Observed::Exact, hunks)
}

fn hash_line(hasher: &LineHasher, line: &PatchLine) -> HunkLineHash {
    let (kind, text) = match line {
        PatchLine::Context(t) => (HunkLineKind::Context, t),
        PatchLine::Removed(t) => (HunkLineKind::Removed, t),
        PatchLine::Added(t) => (HunkLineKind::Added, t),
    };
    HunkLineHash {
        kind,
        hash: hash_edit_line(hasher, text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(p: &str) -> String {
        if p.starts_with('/') {
            p.to_owned()
        } else {
            format!("/wt/{p}")
        }
    }

    fn h(text: &str) -> crate::provenance::LineHash {
        LineHasher::default().hash_line(text.as_bytes())
    }

    #[test]
    fn add_update_move_and_delete_in_one_patch() {
        let patch = "*** Begin Patch\n\
            *** Add File: new.py\n+a\n+b\n\
            *** Update File: /abs/calc.py\n@@ def add(a, b):\n     return a + b\n-x\n+y\n\
            *** Update File: old.py\n*** Move to: moved.py\n@@\n keep\n-gone\n+here\n\
            *** Delete File: dead.py\n\
            *** End Patch";
        let ops = edit_ops(patch, abs);
        assert_eq!(ops.len(), 4);

        assert_eq!(ops[0].kind, EditKind::Create);
        assert_eq!(ops[0].path, "/wt/new.py");
        assert_eq!(ops[0].hunks.len(), 1);
        assert_eq!(ops[0].hunks[0].lines[1].hash, h("b"));
        assert!(ops[0].before.is_none() && ops[0].after.is_none());

        assert_eq!(ops[1].kind, EditKind::Update);
        assert_eq!(ops[1].path, "/abs/calc.py");
        assert_eq!(ops[1].hunks[0].old_start, None);
        assert_eq!(ops[1].hunks[0].header, Some(h("def add(a, b):")));
        let kinds: Vec<_> = ops[1].hunks[0].lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                HunkLineKind::Context,
                HunkLineKind::Removed,
                HunkLineKind::Added
            ]
        );
        // The context line is `    return a + b` (marker space removed).
        assert_eq!(ops[1].hunks[0].lines[0].hash, h("    return a + b"));

        assert_eq!(ops[2].path, "/wt/moved.py");
        assert_eq!(ops[2].renamed_from.as_deref(), Some("/wt/old.py"));
        assert_eq!(ops[2].hunks[0].header, None, "empty `@@` has no header");

        assert_eq!(ops[3].kind, EditKind::Delete);
        assert!(ops[3].hunks.is_empty());
        assert!(ops.iter().all(|o| o.observed == Observed::Exact));
    }

    #[test]
    fn a_flawed_section_is_unparsed_for_that_file_only() {
        let patch = "*** Begin Patch\n\
            *** Update File: a.py\n@@\n-x\n+y\n\
            *** Update File: b.py\n@@\n-x\nrubbish\n+y\n\
            *** Add File: c.py\n+one\n\
            *** End Patch";
        let ops = edit_ops(patch, abs);
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0].observed, Observed::Exact);
        assert_eq!(
            ops[1].observed,
            Observed::Unobserved(UnobservedReason::Unparsed)
        );
        assert!(ops[1].hunks.is_empty());
        assert_eq!(ops[1].path, "/wt/b.py");
        assert_eq!(ops[2].observed, Observed::Exact);
    }

    #[test]
    fn sections_that_do_not_fit_their_operation_are_unparsed() {
        let patch = "*** Begin Patch\n\
            *** Add File: a.py\n+ok\n-nope\n\
            *** Delete File: b.py\n-body\n\
            *** End Patch";
        let ops = edit_ops(patch, abs);
        assert_eq!(ops.len(), 2);
        for op in &ops {
            assert_eq!(
                op.observed,
                Observed::Unobserved(UnobservedReason::Unparsed)
            );
        }
    }

    #[test]
    fn a_cut_off_patch_flags_its_last_section_and_nothing_without_a_path_is_an_op() {
        let patch = "*** Begin Patch\n*** Add File: a.py\n+x\n*** Add File: b.py\n+y";
        let ops = edit_ops(patch, abs);
        assert_eq!(ops[0].observed, Observed::Exact);
        assert_eq!(
            ops[1].observed,
            Observed::Unobserved(UnobservedReason::Unparsed)
        );

        let none = "*** Begin Patch\n*** Add File:\n+x\n*** End Patch";
        assert!(edit_ops(none, abs).is_empty());
        assert!(edit_ops("not a patch at all", abs).is_empty());
        assert!(edit_ops("", abs).is_empty());
    }

    #[test]
    fn crlf_content_hashes_like_lf() {
        let lf = edit_ops(
            "*** Begin Patch\n*** Update File: a\n@@\n-x\n+y\n*** End Patch",
            abs,
        );
        // A CRLF FILE being patched keeps `\r` on the hunk lines while the
        // patch's own terminators are `\n` (the parser keeps a lone `\r`).
        let crlf = edit_ops(
            "*** Begin Patch\n*** Update File: a\n@@\n-x\r\n+y\r\n*** End Patch",
            abs,
        );
        assert_eq!(lf[0].hunks, crlf[0].hunks);
    }
}
