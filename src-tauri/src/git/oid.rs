//! Object ids and byte/path helpers shared by the parsers.

use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};

use super::error::ParseError;

/// A git object id (commit hash) as lowercase hex.
///
/// SHA-1 repositories print 40 hex digits, SHA-256 repositories 64. We accept
/// both and do not interpret the value; it is an opaque, comparable string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid(String);

impl Oid {
    /// Validate and normalise (lowercase) a hex object id.
    pub fn parse(s: &str) -> Result<Oid, ParseError> {
        let ok_len = s.len() == 40 || s.len() == 64;
        if !ok_len || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ParseError::new(format!("not an object id: {s:?}")));
        }
        Ok(Oid(s.to_ascii_lowercase()))
    }

    /// Parse from raw bytes (git output is ASCII for ids).
    pub(crate) fn from_bytes(b: &[u8]) -> Result<Oid, ParseError> {
        let s =
            std::str::from_utf8(b).map_err(|_| ParseError::new("object id is not valid UTF-8"))?;
        Oid::parse(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The all-zero id git prints for "no commit yet" (unborn branch).
    pub fn is_null(&self) -> bool {
        self.0.bytes().all(|b| b == b'0')
    }

    /// First 7 characters, for display.
    pub fn short(&self) -> &str {
        &self.0[..7]
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Build a `PathBuf` from the raw bytes git printed.
///
/// On Unix a path is an arbitrary byte string (only `/` and NUL are special),
/// so a file called `caf\xe9.txt` in Latin-1 is legal and must round-trip. A
/// Rust `String` would force us to lose or reject it. `OsStr::from_bytes`
/// wraps the bytes without validating them.
///
/// On Windows paths are UTF-16 and git prints UTF-8, so a lossy conversion is
/// the right bridge.
#[cfg(unix)]
pub(crate) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
pub(crate) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

/// The raw bytes of a path (inverse of [`path_from_bytes`]); used for hashing.
#[cfg(unix)]
pub(crate) fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(path.as_os_str().as_bytes())
}

#[cfg(not(unix))]
pub(crate) fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    match path.to_string_lossy() {
        Cow::Borrowed(s) => Cow::Borrowed(s.as_bytes()),
        Cow::Owned(s) => Cow::Owned(s.into_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oid_accepts_sha1_and_sha256_lowercases() {
        let sha1 = "ABCDEF0123456789abcdef0123456789abcdef01";
        assert_eq!(Oid::parse(sha1).unwrap().as_str(), sha1.to_lowercase());
        assert!(Oid::parse(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn oid_rejects_garbage() {
        assert!(Oid::parse("").is_err());
        assert!(Oid::parse("xyz").is_err());
        assert!(Oid::parse(&"g".repeat(40)).is_err());
        assert!(Oid::parse(&"a".repeat(41)).is_err());
    }

    #[test]
    fn null_oid() {
        assert!(Oid::parse(&"0".repeat(40)).unwrap().is_null());
        assert!(!Oid::parse(&"1".repeat(40)).unwrap().is_null());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_round_trips() {
        let raw = b"caf\xe9.txt";
        let p = path_from_bytes(raw);
        assert_eq!(path_bytes(&p).as_ref(), raw);
        assert_eq!(p.to_string_lossy(), "caf\u{fffd}.txt");
    }
}
