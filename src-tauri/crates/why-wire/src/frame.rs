//! Socket framing: newline-delimited JSON, one [`Envelope`] per line.
//!
//! Why this framing: a unix stream socket is a byte stream with no message
//! boundaries, so the receiver needs a rule for where one envelope ends. JSON
//! written compactly never contains a raw newline (newlines inside strings are
//! escaped as `\n`), so `'\n'` is an unambiguous terminator; the receiver reads
//! up to it with a buffered reader and never needs a length prefix. The same
//! lines can be appended to a spool file as-is, and a human can `cat` both.
//!
//! The receiver must bound how much it buffers while looking for the newline,
//! or a hostile or broken peer could make it allocate without limit; that limit
//! is [`MAX_LINE_BYTES`] (enforced by the collector, WHY-13, before calling
//! [`decode_line`], which re-checks it).

use serde::Deserialize;
use thiserror::Error;

use crate::envelope::{Envelope, PROTOCOL_VERSION};

/// Largest accepted line, terminating newline included: 1 MiB.
///
/// Hook payloads seen in the WHY-45 fixtures are under 2 KB, but a Claude
/// `Write` of a big file carries the whole content (and `originalFile` on
/// overwrite), so payloads can get large. 1 MiB keeps a single bad event from
/// costing more than that in memory; the hook (WHY-13) decides what to send
/// instead of an oversized payload.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Why a line could not be encoded or decoded.
#[derive(Debug, Error)]
pub enum WireError {
    /// The line (with its newline) is longer than [`MAX_LINE_BYTES`].
    #[error("line of {len} bytes exceeds the {MAX_LINE_BYTES}-byte limit")]
    TooLarge { len: usize },
    /// Not valid UTF-8 (JSON text must be UTF-8).
    #[error("line is not valid UTF-8")]
    NotUtf8,
    /// Valid UTF-8, but not an envelope: truncated, not JSON, missing fields.
    #[error("malformed envelope: {0}")]
    Malformed(#[source] serde_json::Error),
    /// A well-formed envelope from a protocol version this build does not
    /// know. Rejected rather than guessed at: a newer format may have changed
    /// what a field means.
    #[error("envelope version {v} is not supported (this build speaks {PROTOCOL_VERSION})")]
    UnsupportedVersion { v: u32 },
}

/// Serialize `env` as one line: compact JSON followed by `'\n'`.
///
/// Fails with [`WireError::TooLarge`] when the result would exceed
/// [`MAX_LINE_BYTES`], so a sender learns that before writing anything.
pub fn encode_line(env: &Envelope) -> Result<Vec<u8>, WireError> {
    // `to_vec` cannot fail for an `Envelope` (string keys, no non-finite
    // floats in a `Value` parsed from JSON), but map it rather than unwrap.
    let mut line = serde_json::to_vec(env).map_err(WireError::Malformed)?;
    line.push(b'\n');
    if line.len() > MAX_LINE_BYTES {
        return Err(WireError::TooLarge { len: line.len() });
    }
    Ok(line)
}

/// Only the version field, read before the rest so that an envelope from a
/// newer protocol is reported as [`WireError::UnsupportedVersion`] even when
/// its other fields no longer match this build's [`Envelope`].
#[derive(Deserialize)]
struct VersionProbe {
    v: u32,
}

/// Parse one line (with or without its trailing `'\n'` / `"\r\n"`).
///
/// Unknown fields are ignored (a newer hook may add optional fields without
/// bumping the version); an unknown `v` is rejected.
pub fn decode_line(line: &[u8]) -> Result<Envelope, WireError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(WireError::TooLarge { len: line.len() });
    }
    let text = std::str::from_utf8(line).map_err(|_| WireError::NotUtf8)?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);

    let probe: VersionProbe = serde_json::from_str(text).map_err(WireError::Malformed)?;
    if probe.v != PROTOCOL_VERSION {
        return Err(WireError::UnsupportedVersion { v: probe.v });
    }
    serde_json::from_str(text).map_err(WireError::Malformed)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::envelope::Harness;

    fn sample() -> Envelope {
        Envelope {
            v: PROTOCOL_VERSION,
            harness: Harness::Codex,
            harness_version: Some("0.170.0".into()),
            event: "PreToolUse".into(),
            received_at_ms: 1_795_163_330_579,
            cwd: "/tmp/repo with spaces/sub".into(),
            pid: 4242,
            payload: json!({
                "session_id": "s1",
                "hook_event_name": "PreToolUse",
                // A newline inside a string must stay escaped on the wire.
                "tool_input": { "command": "*** Begin Patch\n+x\n*** End Patch" },
                "unicode": "héllo — 世界",
            }),
        }
    }

    #[test]
    fn round_trip_is_one_line() {
        let env = sample();
        let line = encode_line(&env).unwrap();
        assert_eq!(line.last(), Some(&b'\n'));
        // Exactly one newline: the terminator.
        assert_eq!(line.iter().filter(|&&b| b == b'\n').count(), 1);
        assert_eq!(decode_line(&line).unwrap(), env);
        // Without the newline, and with a CRLF, too.
        assert_eq!(decode_line(&line[..line.len() - 1]).unwrap(), env);
        let mut crlf = line[..line.len() - 1].to_vec();
        crlf.extend_from_slice(b"\r\n");
        assert_eq!(decode_line(&crlf).unwrap(), env);
    }

    #[test]
    fn wire_field_names_are_pinned() {
        let value = serde_json::to_value(sample()).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "cwd",
                "event",
                "harness",
                "harness_version",
                "payload",
                "pid",
                "received_at_ms",
                "v"
            ]
        );
        assert_eq!(value["harness"], json!("codex"));
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        let mut value = serde_json::to_value(sample()).unwrap();
        value["added_in_a_later_hook"] = json!({ "anything": [1, 2, 3] });
        let line = serde_json::to_vec(&value).unwrap();
        assert_eq!(decode_line(&line).unwrap(), sample());
    }

    #[test]
    fn unknown_version_is_rejected_even_if_the_shape_changed() {
        let mut value = serde_json::to_value(sample()).unwrap();
        value["v"] = json!(2);
        let line = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            decode_line(&line),
            Err(WireError::UnsupportedVersion { v: 2 })
        ));

        // A v2 envelope whose fields no longer fit v1 still says "version",
        // not "malformed": the probe reads `v` first.
        let v2 = br#"{"v":2,"completely":"different"}"#;
        assert!(matches!(
            decode_line(v2),
            Err(WireError::UnsupportedVersion { v: 2 })
        ));
    }

    #[test]
    fn oversized_lines_are_rejected_both_ways() {
        let mut env = sample();
        env.payload = json!({ "content": "x".repeat(MAX_LINE_BYTES) });
        assert!(matches!(encode_line(&env), Err(WireError::TooLarge { .. })));

        let huge = vec![b' '; MAX_LINE_BYTES + 1];
        assert!(matches!(
            decode_line(&huge),
            Err(WireError::TooLarge { len }) if len == MAX_LINE_BYTES + 1
        ));
    }

    #[test]
    fn a_line_of_exactly_the_limit_is_accepted() {
        let mut env = sample();
        env.payload = json!({ "content": "" });
        let base = encode_line(&env).unwrap().len();
        env.payload = json!({ "content": "x".repeat(MAX_LINE_BYTES - base) });
        let line = encode_line(&env).unwrap();
        assert_eq!(line.len(), MAX_LINE_BYTES);
        assert_eq!(decode_line(&line).unwrap(), env);
    }

    #[test]
    fn truncated_lines_are_malformed() {
        let line = encode_line(&sample()).unwrap();
        for cut in [1, line.len() / 2, line.len() - 2] {
            assert!(
                matches!(decode_line(&line[..cut]), Err(WireError::Malformed(_))),
                "cut at {cut}"
            );
        }
        assert!(matches!(decode_line(b""), Err(WireError::Malformed(_))));
    }

    #[test]
    fn non_utf8_is_rejected() {
        let mut line = encode_line(&sample()).unwrap();
        // Corrupt one byte inside the payload with an invalid UTF-8 sequence.
        let at = line.len() / 2;
        line[at] = 0xff;
        assert!(matches!(decode_line(&line), Err(WireError::NotUtf8)));
    }

    #[test]
    fn missing_fields_are_malformed() {
        let line = br#"{"v":1,"harness":"codex"}"#;
        assert!(matches!(decode_line(line), Err(WireError::Malformed(_))));
    }

    #[test]
    fn unknown_harness_inside_an_envelope_still_decodes() {
        let mut value = serde_json::to_value(sample()).unwrap();
        value["harness"] = json!("some-new-agent");
        let env = decode_line(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(env.harness, Harness::Unknown);
    }
}
