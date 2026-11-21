//! How an event's [`EditOp`]s are kept (WHY-64): the rule both stores apply
//! before storing one, and the compact binary encoding the SQLite store puts in
//! its `edit_ops` table (schema v3).

use crate::contracts::WorktreeId;
use crate::provenance::{
    EditHunk, EditKind, EditOp, FileFingerprint, FileSnapshot, HunkLineHash, HunkLineKind,
    LineHash, LinesDigest, Observed, UnobservedReason,
};

use super::correlation::canonical_path;

/// The form in which `op` is stored, or `None` when it cannot be stored.
///
/// - No `worktree_id`: the op could never be found again (`edit_records` asks
///   by worktree), so it is skipped. The rest of the event, and the rest of
///   the batch, is stored as usual: losing one unresolvable op must not lose
///   the timeline.
/// - The path (and `renamed_from`) is rewritten to [`canonical_path`] form,
///   the spelling `edit_records` is queried with. A path that has none (an
///   absolute path ingest could not make relative) is skipped like above; a
///   `renamed_from` that has none is dropped.
/// - An `Unobserved` op is stored without hunks, `before` or `after`: it
///   cannot carry content it did not observe, whatever the producer sent.
pub(crate) fn stored_form(op: &EditOp) -> Option<(WorktreeId, EditOp)> {
    let worktree = op.worktree_id.clone()?;
    let path = canonical_path(&op.path)?;
    let mut stored = op.clone();
    stored.path = path;
    stored.renamed_from = op.renamed_from.as_deref().and_then(canonical_path);
    if let Observed::Unobserved(_) = stored.observed {
        stored.hunks.clear();
        stored.before = None;
        stored.after = None;
    }
    Some((worktree, stored))
}

// ---- SQLite encoding ------------------------------------------------------

/// The encoding version written to `edit_ops.format`. A later change to the
/// byte layout writes `2` and keeps a decoder for `1`, so old rows stay
/// readable for their 30 days.
pub(crate) const FORMAT_V1: i64 = 1;

pub(crate) fn encode_kind(kind: EditKind) -> &'static str {
    match kind {
        EditKind::Create => "create",
        EditKind::Update => "update",
        EditKind::Delete => "delete",
    }
}

pub(crate) fn decode_kind(s: &str) -> Option<EditKind> {
    Some(match s {
        "create" => EditKind::Create,
        "update" => EditKind::Update,
        "delete" => EditKind::Delete,
        _ => return None,
    })
}

pub(crate) fn encode_observed(observed: Observed) -> &'static str {
    match observed {
        Observed::Exact => "exact",
        Observed::Unobserved(UnobservedReason::PayloadReduced) => "payload_reduced",
        Observed::Unobserved(UnobservedReason::Unparsed) => "unparsed",
    }
}

pub(crate) fn decode_observed(s: &str) -> Option<Observed> {
    Some(match s {
        "exact" => Observed::Exact,
        "payload_reduced" => Observed::Unobserved(UnobservedReason::PayloadReduced),
        "unparsed" => Observed::Unobserved(UnobservedReason::Unparsed),
        _ => return None,
    })
}

// Format 1, all integers little-endian:
//
//   fingerprint = digest[16] line_count:u32 final_newline:u8          (21 B)
//   before      = fingerprint n:u32 hash[16] * n                (25 + 16n B)
//   after       = fingerprint
//   hunks       = count:u32 hunk * count
//   hunk        = flags:u8 [old_start:u32 if flags&1] [header[16] if flags&2]
//                 n:u32 (kind:u8 hash[16]) * n                  kind 0/1/2 =
//                                                       context/removed/added
//
// Raw bytes, not JSON: a hash is 16 bytes here and 34+ as a JSON hex string,
// and `before` holds one per line of the file (16 KB for a 1,000-line file).

const FLAG_OLD_START: u8 = 1;
const FLAG_HEADER: u8 = 2;

fn count(n: usize, what: &str) -> Result<u32, String> {
    u32::try_from(n).map_err(|_| format!("too many {what} to store: {n}"))
}

fn put_fingerprint(out: &mut Vec<u8>, fp: &FileFingerprint) {
    out.extend_from_slice(&fp.lines_digest.0);
    out.extend_from_slice(&fp.line_count.to_le_bytes());
    out.push(u8::from(fp.final_newline));
}

pub(crate) fn encode_after(fp: &FileFingerprint) -> Vec<u8> {
    let mut out = Vec::with_capacity(21);
    put_fingerprint(&mut out, fp);
    out
}

pub(crate) fn encode_before(snapshot: &FileSnapshot) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(25 + 16 * snapshot.lines.len());
    put_fingerprint(&mut out, &snapshot.fingerprint);
    out.extend_from_slice(&count(snapshot.lines.len(), "lines")?.to_le_bytes());
    for line in &snapshot.lines {
        out.extend_from_slice(&line.0);
    }
    Ok(out)
}

pub(crate) fn encode_hunks(hunks: &[EditHunk]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(&count(hunks.len(), "hunks")?.to_le_bytes());
    for hunk in hunks {
        let mut flags = 0;
        if hunk.old_start.is_some() {
            flags |= FLAG_OLD_START;
        }
        if hunk.header.is_some() {
            flags |= FLAG_HEADER;
        }
        out.push(flags);
        if let Some(start) = hunk.old_start {
            out.extend_from_slice(&start.to_le_bytes());
        }
        if let Some(header) = hunk.header {
            out.extend_from_slice(&header.0);
        }
        out.extend_from_slice(&count(hunk.lines.len(), "hunk lines")?.to_le_bytes());
        for line in &hunk.lines {
            out.push(match line.kind {
                HunkLineKind::Context => 0,
                HunkLineKind::Removed => 1,
                HunkLineKind::Added => 2,
            });
            out.extend_from_slice(&line.hash.0);
        }
    }
    Ok(out)
}

/// A strict reader: running out of bytes, a bad tag or bytes left over are all
/// errors (a damaged row is refused, never half-read).
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.rest.len() < n {
            return Err("truncated".into());
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        let mut b = [0u8; 4];
        b.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(b))
    }

    fn hash16(&mut self) -> Result<[u8; 16], String> {
        let mut b = [0u8; 16];
        b.copy_from_slice(self.take(16)?);
        Ok(b)
    }

    fn fingerprint(&mut self) -> Result<FileFingerprint, String> {
        let lines_digest = LinesDigest(self.hash16()?);
        let line_count = self.u32()?;
        let final_newline = match self.u8()? {
            0 => false,
            1 => true,
            other => return Err(format!("bad final-newline flag {other}")),
        };
        Ok(FileFingerprint {
            lines_digest,
            line_count,
            final_newline,
        })
    }

    /// A length read from the row, checked against what is left so a damaged
    /// count cannot make us reserve gigabytes.
    fn len(&mut self, item_bytes: usize) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n.saturating_mul(item_bytes) > self.rest.len() {
            return Err("count larger than the data".into());
        }
        Ok(n)
    }

    fn finish(self) -> Result<(), String> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(format!("{} trailing bytes", self.rest.len()))
        }
    }
}

pub(crate) fn decode_after(bytes: &[u8]) -> Result<FileFingerprint, String> {
    let mut r = Reader { rest: bytes };
    let fp = r.fingerprint()?;
    r.finish()?;
    Ok(fp)
}

pub(crate) fn decode_before(bytes: &[u8]) -> Result<FileSnapshot, String> {
    let mut r = Reader { rest: bytes };
    let fingerprint = r.fingerprint()?;
    let n = r.len(16)?;
    let mut lines = Vec::with_capacity(n);
    for _ in 0..n {
        lines.push(LineHash(r.hash16()?));
    }
    r.finish()?;
    Ok(FileSnapshot { fingerprint, lines })
}

pub(crate) fn decode_hunks(bytes: &[u8]) -> Result<Vec<EditHunk>, String> {
    let mut r = Reader { rest: bytes };
    // The smallest hunk is 5 bytes (flags + an empty line count).
    let n = r.len(5)?;
    let mut hunks = Vec::with_capacity(n);
    for _ in 0..n {
        let flags = r.u8()?;
        if flags & !(FLAG_OLD_START | FLAG_HEADER) != 0 {
            return Err(format!("unknown hunk flags {flags:#x}"));
        }
        let old_start = if flags & FLAG_OLD_START != 0 {
            Some(r.u32()?)
        } else {
            None
        };
        let header = if flags & FLAG_HEADER != 0 {
            Some(LineHash(r.hash16()?))
        } else {
            None
        };
        let count = r.len(17)?;
        let mut lines = Vec::with_capacity(count);
        for _ in 0..count {
            let kind = match r.u8()? {
                0 => HunkLineKind::Context,
                1 => HunkLineKind::Removed,
                2 => HunkLineKind::Added,
                other => return Err(format!("unknown hunk line kind {other}")),
            };
            lines.push(HunkLineHash {
                kind,
                hash: LineHash(r.hash16()?),
            });
        }
        hunks.push(EditHunk {
            old_start,
            header,
            lines,
        });
    }
    r.finish()?;
    Ok(hunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::{LineHasher, hash_lines};

    fn sample_hunks() -> Vec<EditHunk> {
        let h = LineHasher::default();
        vec![
            EditHunk {
                old_start: Some(3),
                header: None,
                lines: vec![
                    HunkLineHash {
                        kind: HunkLineKind::Context,
                        hash: h.hash_line(b"a"),
                    },
                    HunkLineHash {
                        kind: HunkLineKind::Removed,
                        hash: h.hash_line(b"b"),
                    },
                    HunkLineHash {
                        kind: HunkLineKind::Added,
                        hash: h.hash_line(b"c"),
                    },
                ],
            },
            EditHunk {
                old_start: None,
                header: Some(h.hash_line(b"def f():")),
                lines: Vec::new(),
            },
        ]
    }

    #[test]
    fn hunks_before_and_after_round_trip() {
        let hunks = sample_hunks();
        assert_eq!(decode_hunks(&encode_hunks(&hunks).unwrap()).unwrap(), hunks);
        assert_eq!(decode_hunks(&encode_hunks(&[]).unwrap()).unwrap(), vec![]);

        let h = LineHasher::default();
        let before = FileSnapshot::of_text(&h, b"x\ny\nz");
        let bytes = encode_before(&before).unwrap();
        assert_eq!(bytes.len(), 25 + 3 * 16);
        assert_eq!(decode_before(&bytes).unwrap(), before);

        let after = h.fingerprint(b"x\n");
        assert_eq!(decode_after(&encode_after(&after)).unwrap(), after);
    }

    #[test]
    fn damaged_bytes_are_refused_not_half_read() {
        let good = encode_hunks(&sample_hunks()).unwrap();
        assert!(decode_hunks(&good[..good.len() - 1]).is_err(), "truncated");
        let mut extra = good.clone();
        extra.push(0);
        assert!(decode_hunks(&extra).is_err(), "trailing byte");
        let mut bad_kind = good.clone();
        bad_kind[4 + 1 + 4 + 4] = 9; // first line's kind byte
        assert!(decode_hunks(&bad_kind).is_err(), "bad kind");
        let mut bad_flags = good;
        bad_flags[4] = 0x80;
        assert!(decode_hunks(&bad_flags).is_err(), "bad flags");
        // A count that promises far more than the row holds.
        assert!(decode_hunks(&u32::MAX.to_le_bytes()).is_err());

        let before = encode_before(&FileSnapshot::of_lines(
            &LineHasher::default(),
            hash_lines(b"a\n"),
            true,
        ))
        .unwrap();
        assert!(decode_before(&before[..before.len() - 1]).is_err());
        let mut bad_flag = encode_after(&LineHasher::default().fingerprint(b""));
        bad_flag[20] = 2;
        assert!(decode_after(&bad_flag).is_err());
    }

    #[test]
    fn stored_form_needs_a_worktree_and_a_canonical_path() {
        let op = EditOp {
            worktree_id: Some(WorktreeId("wt".into())),
            path: "./src/../lib.rs".into(),
            kind: EditKind::Update,
            renamed_from: Some("/abs/old.rs".into()),
            before: None,
            after: None,
            hunks: sample_hunks(),
            observed: Observed::Exact,
        };
        let (wt, stored) = stored_form(&op).unwrap();
        assert_eq!(wt.0, "wt");
        assert_eq!(stored.path, "lib.rs");
        assert_eq!(stored.renamed_from, None, "not canonical: dropped");

        let no_wt = EditOp {
            worktree_id: None,
            ..op.clone()
        };
        assert!(stored_form(&no_wt).is_none());
        let absolute = EditOp {
            path: "/abs/lib.rs".into(),
            ..op.clone()
        };
        assert!(stored_form(&absolute).is_none());

        let unobserved = EditOp {
            observed: Observed::Unobserved(UnobservedReason::PayloadReduced),
            after: Some(LineHasher::default().fingerprint(b"")),
            ..op
        };
        let (_, stored) = stored_form(&unobserved).unwrap();
        assert!(stored.hunks.is_empty() && stored.after.is_none());
    }
}
