//! The one line-hashing policy of v0.3 (decision 29): how a file's bytes become
//! a sequence of [`LineHash`]es, and how that sequence is summed up in a
//! [`FileFingerprint`].
//!
//! # The policy
//!
//! - A line is the bytes up to a `\n`. A single `\r` right before that `\n`
//!   belongs to the terminator, so a CRLF file and its LF twin hash the same
//!   (editors and `core.autocrlf` flip line endings without anyone "writing"
//!   the line). A lone `\r` anywhere else is content, including a `\r` at the
//!   very end of a file with no final newline (it terminates nothing).
//! - The last line hashes the same with or without a trailing newline; whether
//!   the file ends in one is kept separately as `final_newline`.
//! - The empty file has zero lines. `"\n"` has one empty line.
//! - Bytes, not text: non-UTF-8 bytes are hashed as they are (never a lossy
//!   conversion, which could map two different lines to one).
//! - **No whitespace or case normalization.** A formatter that re-indents a
//!   line has rewritten it, and its new origin is the formatter, on purpose.
//!
//! # What the hashes do NOT promise
//!
//! WHY stores these hashes instead of line text, so `events.db` never holds the
//! code itself. That is the whole promise. The hashes are NOT secret and NOT
//! irreversible: they are unsalted, so anyone holding the database can hash a
//! guess and compare. A long line of real code is impractical to guess, but a
//! short, low-entropy one (`API_KEY=hunter2` in a one-line `.env` an agent
//! wrote) can be found by brute force. A per-install secret key would close
//! that gap; it is not added yet (the owner decides). Every hash goes through
//! [`LineHasher`], so a keyed scheme is one new [`LineHasher`] constructor and
//! a new domain-separator version, without touching any call site.

use std::fmt;

use sha2::{Digest, Sha256};

/// Domain separator of a line hash. Part of the persisted format: a stored
/// hash is only comparable with hashes made under the same separator, so a
/// change to the recipe needs a NEW version here (`v2`), never an edit.
const LINE_DOMAIN: &[u8] = b"why:line:v1\0";
/// Domain separator of a [`LinesDigest`] (different from [`LINE_DOMAIN`] so a
/// one-line file's digest can never equal some line's hash).
const LINES_DOMAIN: &[u8] = b"why:lines:v1\0";

/// The hash of one line: the first 16 bytes (128 bits) of SHA-256 over
/// `b"why:line:v1\0" ++ line bytes` (terminator excluded, see the module docs).
///
/// 128 bits: a collision between two different lines needs about 2^64 distinct
/// lines (the birthday bound); one person's repositories hold millions. A
/// collision would only ever make two lines *match*, never invent an author.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LineHash(pub [u8; 16]);

/// A digest over a SEQUENCE of line hashes (not over raw file bytes, so the
/// CRLF policy above holds for it too, and a replayed state, which is only
/// hashes, can be checked against it without any text).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LinesDigest(pub [u8; 16]);

fn hex(bytes: &[u8; 16], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for b in bytes {
        write!(f, "{b:02x}")?;
    }
    Ok(())
}

// Hex in `Debug`: test failures and logs show `LineHash(ff53…)`, not 16 numbers.
impl fmt::Debug for LineHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LineHash(")?;
        hex(&self.0, f)?;
        f.write_str(")")
    }
}

impl fmt::Display for LineHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex(&self.0, f)
    }
}

impl fmt::Debug for LinesDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LinesDigest(")?;
        hex(&self.0, f)?;
        f.write_str(")")
    }
}

/// A whole file summed up without its text: enough to VERIFY that a replayed
/// state is the file the harness saw, not enough to repair one that is not
/// (that needs the line sequence, see [`super::edit::FileSnapshot`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileFingerprint {
    pub lines_digest: LinesDigest,
    /// Saturates at `u32::MAX` (a 4-billion-line file cannot reach the hook:
    /// payloads are capped at 1 MiB); the digest still covers every line.
    pub line_count: u32,
    pub final_newline: bool,
}

/// The lines of `text` under the policy above, borrowed (no allocation).
/// Returned by [`split_lines`].
#[derive(Debug, Clone)]
pub struct Lines<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            // Also covers "the text ended with `\n`": no empty line after it.
            return None;
        }
        match self.rest.iter().position(|&b| b == b'\n') {
            Some(end) => {
                let line = &self.rest[..end];
                self.rest = &self.rest[end + 1..];
                // The `\r` of a CRLF terminator is not content.
                Some(line.strip_suffix(b"\r").unwrap_or(line))
            }
            None => {
                // The last line, without a newline: everything is content,
                // including a trailing lone `\r`.
                let line = self.rest;
                self.rest = &[];
                Some(line)
            }
        }
    }
}

/// Split `text` into lines (see the module docs). Linear in `text.len()`.
pub fn split_lines(text: &[u8]) -> Lines<'_> {
    Lines { rest: text }
}

/// Whether `text` ends with a line terminator. The empty file has none.
pub fn has_final_newline(text: &[u8]) -> bool {
    text.last() == Some(&b'\n')
}

/// Which recipe a [`LineHasher`] uses. Private: a new scheme (say a keyed
/// BLAKE/HMAC with a per-install secret) is added here, plus a constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheme {
    /// SHA-256 with the public `v1` domain separators, no key.
    UnkeyedV1,
}

/// THE way to hash lines. Every hash WHY stores or compares is made here, so
/// changing the scheme (adding a key) cannot miss a call site. `Default` is
/// the unkeyed `v1` recipe that is pinned by a golden test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineHasher {
    scheme: Scheme,
}

impl Default for LineHasher {
    fn default() -> Self {
        Self::unkeyed_v1()
    }
}

impl LineHasher {
    /// The unkeyed `v1` recipe (see [`LineHash`]).
    pub const fn unkeyed_v1() -> Self {
        Self {
            scheme: Scheme::UnkeyedV1,
        }
    }

    /// The hash of one line's content (no terminator: callers pass what
    /// [`split_lines`] yields).
    pub fn hash_line(&self, line: &[u8]) -> LineHash {
        match self.scheme {
            Scheme::UnkeyedV1 => {
                let mut h = Sha256::new();
                h.update(LINE_DOMAIN);
                h.update(line);
                LineHash(first_16(&h.finalize()))
            }
        }
    }

    /// Every line of `text`, hashed, in order. One pass, one allocation of
    /// 16 bytes per line.
    pub fn hash_lines(&self, text: &[u8]) -> Vec<LineHash> {
        split_lines(text).map(|l| self.hash_line(l)).collect()
    }

    /// The digest of a line-hash sequence: SHA-256 over the domain separator,
    /// the count as u64 little-endian, then each 16-byte hash, cut to 16 bytes.
    /// The count prefix is not needed for uniqueness (hashes are fixed size)
    /// but makes the recipe self-describing.
    pub fn digest(&self, hashes: &[LineHash]) -> LinesDigest {
        match self.scheme {
            Scheme::UnkeyedV1 => {
                let mut h = Sha256::new();
                h.update(LINES_DOMAIN);
                h.update((hashes.len() as u64).to_le_bytes());
                for line in hashes {
                    h.update(line.0);
                }
                LinesDigest(first_16(&h.finalize()))
            }
        }
    }

    /// The fingerprint of a file given as its line hashes.
    pub fn fingerprint_of(&self, hashes: &[LineHash], final_newline: bool) -> FileFingerprint {
        FileFingerprint {
            lines_digest: self.digest(hashes),
            line_count: u32::try_from(hashes.len()).unwrap_or(u32::MAX),
            final_newline,
        }
    }

    /// The fingerprint of a file given as its bytes.
    pub fn fingerprint(&self, text: &[u8]) -> FileFingerprint {
        self.fingerprint_of(&self.hash_lines(text), has_final_newline(text))
    }
}

fn first_16(digest: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// [`LineHasher::hash_lines`] with the default hasher.
pub fn hash_lines(text: &[u8]) -> Vec<LineHash> {
    LineHasher::default().hash_lines(text)
}

/// [`LineHasher::fingerprint`] with the default hasher.
pub fn fingerprint(text: &[u8]) -> FileFingerprint {
    LineHasher::default().fingerprint(text)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn lines(text: &[u8]) -> Vec<&[u8]> {
        split_lines(text).collect()
    }

    fn h(line: &[u8]) -> LineHash {
        LineHasher::default().hash_line(line)
    }

    #[test]
    fn line_hash_derivation_is_pinned() {
        // Stored in `events.db`: if this value changes, every stored hash
        // stops matching new ones. Change the domain separator version
        // (`why:line:v2`) instead of editing these expectations.
        // Recomputed independently (Python hashlib):
        // sha256(b"why:line:v1\0" + line)[:16].hex()
        assert_eq!(
            h(b"fn main() {}").to_string(),
            "ff5380e80831bc6609ad4750510ce8ed"
        );
        assert_eq!(h(b"").to_string(), "97362ba67f23805265b6ceb21ccd7833");
    }

    #[test]
    fn lines_digest_derivation_is_pinned() {
        // sha256(b"why:lines:v1\0" + u64le(n) + hashes...)[:16].hex()
        let hasher = LineHasher::default();
        let digest = hasher.digest(&[h(b"a"), h(b""), h(b"b")]);
        assert_eq!(
            format!("{digest:?}"),
            "LinesDigest(1ae8b9906127ce15056a75bb45aaba48)"
        );
        assert_eq!(
            format!("{:?}", hasher.digest(&[])),
            "LinesDigest(7b461b2a4758c774d2778a80eb7f8baa)"
        );
    }

    #[test]
    fn crlf_and_lf_hash_the_same() {
        assert_eq!(hash_lines(b"a\r\nb\r\n"), hash_lines(b"a\nb\n"));
        assert_eq!(lines(b"a\r\nb"), vec![&b"a"[..], b"b"]);
        // Mixed endings in one file are fine too.
        assert_eq!(hash_lines(b"a\r\nb\n"), hash_lines(b"a\nb\n"));
    }

    #[test]
    fn a_lone_cr_is_content() {
        assert_eq!(lines(b"a\rb\n"), vec![&b"a\rb"[..]]);
        assert_ne!(h(b"a\rb"), h(b"ab"));
        // Only ONE `\r` belongs to the terminator.
        assert_eq!(lines(b"a\r\r\n"), vec![&b"a\r"[..]]);
        // At the end of a file without a final newline it terminates nothing.
        assert_eq!(lines(b"a\r"), vec![&b"a\r"[..]]);
    }

    #[test]
    fn the_last_line_hashes_the_same_with_or_without_a_newline() {
        assert_eq!(hash_lines(b"a\nb"), hash_lines(b"a\nb\n"));
        let with = fingerprint(b"a\nb\n");
        let without = fingerprint(b"a\nb");
        assert_eq!(with.lines_digest, without.lines_digest);
        assert_eq!(with.line_count, 2);
        assert!(with.final_newline);
        assert!(!without.final_newline);
    }

    #[test]
    fn empty_file_and_a_single_newline() {
        assert!(lines(b"").is_empty());
        let empty = fingerprint(b"");
        assert_eq!(empty.line_count, 0);
        assert!(!empty.final_newline);

        assert_eq!(lines(b"\n"), vec![&b""[..]]);
        let one = fingerprint(b"\n");
        assert_eq!(one.line_count, 1);
        assert!(one.final_newline);
        assert_ne!(empty.lines_digest, one.lines_digest);
        // Blank lines are lines.
        assert_eq!(lines(b"\n\n"), vec![&b""[..], b""]);
    }

    #[test]
    fn non_utf8_bytes_hash_as_bytes() {
        let latin1 = b"caf\xe9\n\xff\xfe\n";
        assert_eq!(lines(latin1), vec![&b"caf\xe9"[..], b"\xff\xfe"]);
        // Two invalid sequences that a lossy conversion would both turn into
        // U+FFFD must stay different.
        assert_ne!(h(b"\xff"), h(b"\xfe"));
        assert_ne!(h(b"\xff"), h("\u{fffd}".as_bytes()));
    }

    #[test]
    fn no_whitespace_or_case_normalization() {
        assert_ne!(h(b"x = 1"), h(b"x  = 1"));
        assert_ne!(h(b"x = 1"), h(b"x = 1 "));
        assert_ne!(h(b"\tx"), h(b"    x"));
        assert_ne!(h(b"Foo"), h(b"foo"));
    }

    #[test]
    fn fingerprint_is_eol_agnostic_and_sees_one_changed_line() {
        let lf = fingerprint(b"one\ntwo\nthree\n");
        assert_eq!(fingerprint(b"one\r\ntwo\r\nthree\r\n"), lf);
        assert_ne!(
            fingerprint(b"one\ntwo!\nthree\n").lines_digest,
            lf.lines_digest
        );
        // Order matters.
        assert_ne!(
            fingerprint(b"two\none\nthree\n").lines_digest,
            lf.lines_digest
        );
    }

    #[test]
    fn a_million_lines_hash_in_linear_time() {
        let mut text = Vec::with_capacity(12_000_000);
        for i in 0..1_000_000u32 {
            text.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        let start = Instant::now();
        let fp = fingerprint(&text);
        let took = start.elapsed();
        assert_eq!(fp.line_count, 1_000_000);
        // Generous: a debug build with optimized dependencies does this in well
        // under a second; quadratic work (re-scanning from the start for each
        // line) would take hours.
        assert!(took < Duration::from_secs(20), "took {took:?}");
    }

    #[test]
    fn a_single_50_mb_line_is_one_line() {
        let text = vec![b'x'; 50 * 1024 * 1024];
        let start = Instant::now();
        let fp = fingerprint(&text);
        assert_eq!(fp.line_count, 1);
        assert!(!fp.final_newline);
        assert!(start.elapsed() < Duration::from_secs(20));
    }
}
