//! Parser for Codex's `apply_patch` format, the text in
//! `tool_input.command` of an `apply_patch` hook.
//!
//! ```text
//! *** Begin Patch
//! *** Add File: src/new.py
//! +first line
//! *** Update File: src/old.py
//! *** Move to: src/renamed.py        (optional)
//! @@ def add(a, b):                  (optional context header)
//!  context line
//! -removed line
//! +added line
//! *** Delete File: src/gone.py
//! *** End Patch
//! ```
//!
//! This is the whole reason Codex edits are cheap to observe: the harness
//! sends ONE patch string (unlike Claude Code's one-tool-call-per-edit
//! `Edit`/`Write`), so the hook carries what changed in every file.
//!
//! The format is the model's output, not a stable API, so the parser is
//! **lenient and total**: it never panics, never indexes blindly, and reports
//! what it could not understand in [`ParsedPatch::problems`] instead of
//! failing. A patch that parses to nothing is still a recorded attempt; the
//! caller decides what that means (outcome stays `unknown`).

/// Cap on files per patch. A hostile or runaway patch cannot make the adapter
/// allocate without bound (the wire already limits one payload, this bounds
/// what we derive from it).
pub const MAX_EDITS: usize = 1000;
const MAX_PROBLEMS: usize = 32;
const MAX_PROBLEM_BYTES: usize = 256;

/// What a patch section does to its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchOp {
    Add,
    Update,
    Delete,
}

/// One line of a hunk, without its leading marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    Context(String),
    Added(String),
    Removed(String),
}

/// One `@@` block (or the whole body of an added file).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hunk {
    /// Text after `@@` (usually the enclosing function), if any.
    pub header: Option<String>,
    pub lines: Vec<HunkLine>,
}

impl Hunk {
    pub fn added(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| matches!(l, HunkLine::Added(_)))
            .count()
    }

    pub fn removed(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| matches!(l, HunkLine::Removed(_)))
            .count()
    }
}

/// The patch's effect on one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    /// As written in the patch (absolute for Codex 0.170 in the main checkout,
    /// relative to the session cwd in a linked worktree: both seen).
    pub path: String,
    pub op: PatchOp,
    /// `*** Move to:` (a rename), for `Update` only.
    pub move_to: Option<String>,
    pub hunks: Vec<Hunk>,
}

impl FileEdit {
    pub fn added(&self) -> usize {
        self.hunks.iter().map(Hunk::added).sum()
    }

    pub fn removed(&self) -> usize {
        self.hunks.iter().map(Hunk::removed).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedPatch {
    pub edits: Vec<FileEdit>,
    /// `*** End Patch` was seen. Without it the patch was cut off (or never
    /// well-formed); the edits found so far are still reported.
    pub complete: bool,
    /// The first problems found, capped in count and size. Empty for a clean patch.
    pub problems: Vec<String>,
}

impl ParsedPatch {
    pub fn is_clean(&self) -> bool {
        self.complete && self.problems.is_empty()
    }

    fn problem(&mut self, message: impl FnOnce() -> String) {
        if self.problems.len() < MAX_PROBLEMS {
            let mut message = message();
            let mut end = message.len().min(MAX_PROBLEM_BYTES);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            self.problems.push(message);
        }
    }
}

// Bound input excerpts before formatting. The same file path can appear in
// many diagnostics, and Debug formatting can expand escapes.
fn preview(text: &str) -> &str {
    match text.char_indices().nth(40) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// Split into lines, tolerating CRLF.
///
/// A CRLF is only stripped when EVERY line has it: then the whole patch was
/// written with Windows line endings. A lone `\r` on one line is more likely
/// part of the file's own content (a CRLF file being patched) and is kept.
fn lines_of(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    // `"a\n"` splits into ["a", ""]: the final empty piece is not a line.
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let all_crlf = !lines.is_empty() && lines.iter().all(|l| l.ends_with('\r'));
    if all_crlf {
        for l in &mut lines {
            *l = l.strip_suffix('\r').unwrap_or(l);
        }
    }
    lines
}

/// `*** Add File: <path>` and friends: the path is everything after the colon,
/// so spaces and non-ASCII characters in it are preserved. Only the single
/// separating space and trailing whitespace are removed.
fn header_value<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(prefix)?;
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    Some(rest.trim_end())
}

/// Parse a patch. Total: any input yields a [`ParsedPatch`].
pub fn parse_patch(text: &str) -> ParsedPatch {
    let mut out = ParsedPatch::default();
    let lines = lines_of(text);
    let mut iter = lines.into_iter().peekable();

    // Leading blank lines are harmless; anything else before the marker is
    // not a patch we recognise, but we keep scanning for sections anyway so
    // a model that added a preamble still gets its edits recorded.
    let mut seen_begin = false;
    while let Some(line) = iter.peek() {
        if line.trim().is_empty() {
            iter.next();
        } else {
            break;
        }
    }
    if iter.peek().is_some_and(|l| l.trim() == "*** Begin Patch") {
        seen_begin = true;
        iter.next();
    }
    if !seen_begin {
        out.problem(|| "missing `*** Begin Patch`".into());
    }

    let mut current: Option<FileEdit> = None;
    let mut hunk: Option<Hunk> = None;

    // Close the open hunk into the open file. Empty hunks (an `@@` with no
    // lines under it) carry no information and are dropped.
    fn flush_hunk(current: &mut Option<FileEdit>, hunk: &mut Option<Hunk>) {
        if let (Some(file), Some(h)) = (current.as_mut(), hunk.take())
            && !h.lines.is_empty()
        {
            file.hunks.push(h);
        }
    }

    for line in iter {
        if line.trim() == "*** End Patch" {
            flush_hunk(&mut current, &mut hunk);
            if let Some(file) = current.take() {
                out.edits.push(file);
            }
            out.complete = true;
            break;
        }

        let section = if let Some(p) = header_value(line, "*** Add File:") {
            Some((PatchOp::Add, p))
        } else if let Some(p) = header_value(line, "*** Update File:") {
            Some((PatchOp::Update, p))
        } else {
            header_value(line, "*** Delete File:").map(|p| (PatchOp::Delete, p))
        };
        if let Some((op, path)) = section {
            flush_hunk(&mut current, &mut hunk);
            if let Some(file) = current.take() {
                out.edits.push(file);
            }
            if out.edits.len() >= MAX_EDITS {
                out.problem(|| format!("more than {MAX_EDITS} files; the rest is ignored"));
                break;
            }
            if path.is_empty() {
                out.problem(|| "section header without a path".into());
            }
            current = Some(FileEdit {
                path: path.to_owned(),
                op,
                move_to: None,
                hunks: Vec::new(),
            });
            continue;
        }

        let Some(file) = current.as_mut() else {
            // Content before any section header.
            if !line.trim().is_empty() {
                out.problem(|| "line outside any file section".into());
            }
            continue;
        };

        if let Some(dest) = header_value(line, "*** Move to:") {
            if file.op == PatchOp::Update && file.hunks.is_empty() && hunk.is_none() {
                file.move_to = Some(dest.to_owned());
            } else {
                out.problem(|| "`*** Move to:` outside an Update header".into());
            }
            continue;
        }
        if line.trim() == "*** End of File" {
            // A marker for the applier ("this hunk ends the file"); it is not
            // content.
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@") {
            flush_hunk(&mut current, &mut hunk);
            let header = rest.trim();
            hunk = Some(Hunk {
                header: (!header.is_empty()).then(|| header.to_owned()),
                lines: Vec::new(),
            });
            continue;
        }
        if line.starts_with("***") {
            out.problem(|| format!("unknown directive {:?}", preview(line)));
            continue;
        }

        // A content line. The first hunk of an Update may omit its `@@`.
        let target = hunk.get_or_insert_with(Hunk::default);
        let parsed = match line.chars().next() {
            Some('+') => Some(HunkLine::Added(line[1..].to_owned())),
            Some('-') => Some(HunkLine::Removed(line[1..].to_owned())),
            Some(' ') => Some(HunkLine::Context(line[1..].to_owned())),
            // The applier accepts an empty line as an empty context line
            // (editors strip the lone space).
            None => Some(HunkLine::Context(String::new())),
            Some(_) => None,
        };
        match parsed {
            Some(l) => target.lines.push(l),
            None => out.problem(|| {
                format!(
                    "unrecognised line in {:?}: {:?}",
                    preview(&file.path),
                    preview(line)
                )
            }),
        }
    }

    // Reached the end of input without `*** End Patch`: keep what was read.
    if !out.complete {
        flush_hunk(&mut current, &mut hunk);
        if let Some(file) = current.take() {
            out.edits.push(file);
        }
        out.problem(|| "missing `*** End Patch`".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_stay_small_even_for_many_errors_in_one_section() {
        let path = "世\u{e000}".repeat(1024);
        let invalid_lines = "malformed\n".repeat(256);
        let patch =
            format!("*** Begin Patch\n*** Update File: {path}\n{invalid_lines}*** End Patch");
        let parsed = parse_patch(&patch);
        assert!(parsed.complete);
        assert!(!parsed.is_clean());
        assert_eq!(parsed.edits[0].path, path);
        assert!(parsed.problems.len() <= 32);
        assert!(parsed.problems.iter().all(|p| p.len() <= 256));
    }

    #[test]
    fn line_marker_slice_is_char_boundary_safe() {
        // `line[1..]` after matching an ASCII marker is always on a boundary;
        // multi-byte content right after the marker must not panic.
        let p = parse_patch("*** Begin Patch\n*** Add File: a\n+日本語\n*** End Patch");
        assert_eq!(
            p.edits[0].hunks[0].lines,
            vec![HunkLine::Added("日本語".into())]
        );
    }
}
