//! Decoding of GitLab repository file payloads.
//!
//! One implementation, backed by the `base64` crate. It replaces two hand-rolled
//! decoders that disagreed with each other: one returned a `Result`, the other a
//! `String` at any cost — truncating the rest of a file at the first bad character,
//! folding an invalid character into the output as garbage bytes, and dropping a
//! trailing group that was not a multiple of four. Code quality was then scored on
//! whatever came out, as if it were the whole file.

use crate::error::{Error, Result};
use base64::alphabet;
use base64::engine::general_purpose::GeneralPurpose;
use base64::engine::{DecodePaddingMode, GeneralPurposeConfig};
use base64::Engine as _;
use serde_json::Value;

/// Standard alphabet with padding optional. GitLab pads, but being strict about
/// padding would reject well-formed content rather than malformed content.
const GITLAB_B64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Decode base64, ignoring the line breaks GitLab inserts every 60 columns.
///
/// Malformed input is an error — never a partial result presented as complete.
pub(crate) fn decode_base64(input: &str) -> Result<Vec<u8>> {
    let cleaned: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    GITLAB_B64
        .decode(cleaned.as_bytes())
        .map_err(|e| Error::other(format!("malformed base64 file content: {e}")))
}

/// Text of a file object as returned by `GET /projects/:id/repository/files/:path`.
///
/// Honours the object's `encoding` field instead of assuming base64. Invalid UTF-8 is
/// replaced so binary files remain analysable as text; invalid base64 is an error.
pub(crate) fn file_text(file: &Value) -> Result<String> {
    let content = file["content"].as_str().unwrap_or("");
    match file["encoding"].as_str().unwrap_or("base64") {
        "base64" => Ok(String::from_utf8_lossy(&decode_base64(content)?).into_owned()),
        _ => Ok(content.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_base64, file_text};
    use serde_json::json;

    #[test]
    fn decodes_padded_unpadded_and_line_wrapped_input() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVsbG8").unwrap(), b"hello", "padding is optional");
        assert_eq!(decode_base64("aGVs\nbG8=\n").unwrap(), b"hello", "GitLab wraps lines");
        assert_eq!(decode_base64("").unwrap(), b"");
    }

    #[test]
    fn malformed_input_is_an_error_not_a_silent_partial() {
        // The hand-rolled decoder returned "he" for this — the rest of the file gone.
        assert!(decode_base64("aGVs*G8=").is_err(), "invalid character mid-stream");
        assert!(decode_base64("a").is_err(), "a lone trailing symbol cannot be data");
    }

    #[test]
    fn file_text_honours_the_encoding_field() {
        assert_eq!(file_text(&json!({"content": "aGk=", "encoding": "base64"})).unwrap(), "hi");
        assert_eq!(file_text(&json!({"content": "plain", "encoding": "text"})).unwrap(), "plain");
        // A missing encoding is GitLab's default: base64.
        assert_eq!(file_text(&json!({"content": "aGk="})).unwrap(), "hi");
        assert!(file_text(&json!({"content": "@@@", "encoding": "base64"})).is_err());
        // Binary content survives as text rather than failing the analysis.
        let bin = file_text(&json!({"content": "/w==", "encoding": "base64"})).unwrap();
        assert_eq!(bin, "\u{FFFD}");
    }
}
