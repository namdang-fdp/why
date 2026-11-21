//! A small unified diff for the preview. Config files are tiny and our edits
//! are local, so a textbook LCS on the lines between the common prefix and
//! suffix is plenty; no diff crate needed.

const CONTEXT: usize = 3;
/// Above this many cells the middle section is shown as one replaced block
/// instead of building the LCS table.
const MAX_CELLS: usize = 4_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Del,
    Add,
}

/// Unified diff of `before` to `after` (`""` when they are equal). A missing
/// file is `None`; it diffs as an empty file, against `/dev/null`.
pub fn unified_diff(label: &str, before: Option<&str>, after: Option<&str>) -> String {
    let (a, b) = (before.unwrap_or(""), after.unwrap_or(""));
    if before == after || (a == b && before.is_some() == after.is_some()) {
        return String::new();
    }
    let a_lines: Vec<&str> = a.split_inclusive('\n').collect();
    let b_lines: Vec<&str> = b.split_inclusive('\n').collect();
    let ops = script(&a_lines, &b_lines);

    let mut out = format!(
        "--- {}\n+++ {}\n",
        if before.is_some() {
            format!("a/{label}")
        } else {
            "/dev/null".into()
        },
        if after.is_some() {
            format!("b/{label}")
        } else {
            "/dev/null".into()
        },
    );
    // Indexes into `ops` of changed lines, grouped into hunks with context.
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, (op, _))| *op != Op::Same)
        .map(|(i, _)| i)
        .collect();
    let mut i = 0;
    while i < changed.len() {
        let first = changed[i];
        let mut last = first;
        while i + 1 < changed.len() && changed[i + 1] - last <= 2 * CONTEXT + 1 {
            i += 1;
            last = changed[i];
        }
        i += 1;
        let from = first.saturating_sub(CONTEXT);
        let to = (last + CONTEXT + 1).min(ops.len());
        let (mut a_start, mut b_start) = (1, 1);
        for (op, _) in &ops[..from] {
            a_start += usize::from(*op != Op::Add);
            b_start += usize::from(*op != Op::Del);
        }
        let a_len = ops[from..to].iter().filter(|(o, _)| *o != Op::Add).count();
        let b_len = ops[from..to].iter().filter(|(o, _)| *o != Op::Del).count();
        // Unified diff convention: an empty range is reported one line earlier.
        let start = |s: usize, len: usize| if len == 0 { s - 1 } else { s };
        out.push_str(&format!(
            "@@ -{},{a_len} +{},{b_len} @@\n",
            start(a_start, a_len),
            start(b_start, b_len)
        ));
        for (op, line) in &ops[from..to] {
            out.push(match op {
                Op::Same => ' ',
                Op::Del => '-',
                Op::Add => '+',
            });
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    out
}

fn script<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(Op, &'a str)> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);

    let mut ops: Vec<(Op, &str)> = a[..prefix].iter().map(|l| (Op::Same, *l)).collect();
    if am.len().saturating_mul(bm.len()) > MAX_CELLS {
        ops.extend(am.iter().map(|l| (Op::Del, *l)));
        ops.extend(bm.iter().map(|l| (Op::Add, *l)));
    } else {
        // lcs[i][j] = LCS length of am[i..] and bm[j..].
        let (n, m) = (am.len(), bm.len());
        let mut lcs = vec![vec![0u32; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i][j] = if am[i] == bm[j] {
                    lcs[i + 1][j + 1] + 1
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i] == bm[j] {
                ops.push((Op::Same, am[i]));
                i += 1;
                j += 1;
            } else if j >= m || (i < n && lcs[i + 1][j] >= lcs[i][j + 1]) {
                ops.push((Op::Del, am[i]));
                i += 1;
            } else {
                ops.push((Op::Add, bm[j]));
                j += 1;
            }
        }
    }
    ops.extend(a[a.len() - suffix..].iter().map(|l| (Op::Same, *l)));
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_texts_have_no_diff() {
        assert_eq!(unified_diff("f", Some("a\n"), Some("a\n")), "");
        assert_eq!(unified_diff("f", None, None), "");
    }

    #[test]
    fn an_insertion_shows_context_and_hunk_header() {
        let before = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let after = "1\n2\n3\n4\nNEW\n5\n6\n7\n8\n";
        assert_eq!(
            unified_diff("f", Some(before), Some(after)),
            "--- a/f\n+++ b/f\n@@ -2,6 +2,7 @@\n 2\n 3\n 4\n+NEW\n 5\n 6\n 7\n"
        );
    }

    #[test]
    fn a_new_file_diffs_against_dev_null_and_a_deleted_one_too() {
        assert_eq!(
            unified_diff("f", None, Some("x\n")),
            "--- /dev/null\n+++ b/f\n@@ -0,0 +1,1 @@\n+x\n"
        );
        assert_eq!(
            unified_diff("f", Some("x\n"), None),
            "--- a/f\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-x\n"
        );
    }

    #[test]
    fn a_missing_trailing_newline_is_visible() {
        let d = unified_diff("f", Some("a"), Some("a\n"));
        assert!(d.contains("-a\n\\ No newline at end of file\n+a\n"), "{d}");
    }

    #[test]
    fn distant_changes_make_separate_hunks() {
        let before: String = (1..=30).map(|i| format!("{i}\n")).collect();
        let after = before
            .replacen("\n2\n", "\ntwo\n", 1)
            .replacen("\n29\n", "\ntw9\n", 1);
        // Line 1 stays; lines 2 and 29 change.
        let d = unified_diff("f", Some(&before), Some(&after));
        assert_eq!(d.matches("@@ -").count(), 2, "{d}");
    }
}
