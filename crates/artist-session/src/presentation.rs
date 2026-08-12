//! Lossless model-presentation grammar.
//!
//! Canonical session events always retain the original text. This module only
//! produces a compact, explicitly reversible *view* for a model-facing
//! transport. It has no side effects and never treats its encoded form as
//! semantic source data.
//!
//! Grammar (UTF-8 terminals):
//!
//! ```text
//! value  := item*
//! item   := literal | '\\' scalar | '×' count '{' value '}'
//! count  := [1-9][0-9]*
//! ```
//!
//! `§`, `×`, `{`, `}`, and `\\` are always escaped as literals. The encoder's
//! frozen discovery strategy is deliberately modest and predictable: it folds
//! only maximal runs of an identical Unicode scalar of length at least three,
//! and only when the resulting grammar is shorter than the escaped literals.
//! Dictionary substitution is a separate, durable layer; it must define its
//! references before this grammar is emitted.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Where a canonical presentation originated. Anchors and occurrence identity
/// are optional because a caller may be encoding generated text, but when they
/// exist they travel with the mapping rather than being reconstructed from
/// display offsets later.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationSource {
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<String>,
}

/// One lossless correspondence between a contiguous encoded grammar item and
/// the canonical byte interval it decodes to. A repetition maps one compact
/// item to its whole expanded interval; literal/escape items map one scalar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationSpan {
    pub encoded_start: usize,
    pub encoded_end: usize,
    pub canonical_start: usize,
    pub canonical_end: usize,
    pub repeated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presentation {
    /// Exact canonical text. This is the value to persist and source-ground.
    pub canonical: String,
    /// Reversible compact representation suitable for display to a model.
    pub encoded: String,
    /// Explicit mapping from the presentation grammar to canonical bytes.
    pub spans: Vec<PresentationSpan>,
    /// Durable provenance for canonical-byte locations, if known.
    pub source: Option<PresentationSource>,
}

/// The presentation/canonical pair recorded at the model boundary.  Fresh
/// results deliberately use [`Self::literal`]: the model saw ordinary text,
/// not a retroactively encoded grammar.  Compacted results may instead carry
/// the grammar representation produced by [`Presentation`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPresentation {
    pub visible: String,
    pub canonical: String,
    pub spans: Vec<PresentationSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PresentationSource>,
    /// `literal` for fresh output; `presentation-v1` for the reversible
    /// compact grammar.
    pub encoding: String,
}

impl ModelPresentation {
    pub fn literal(text: impl Into<String>) -> Self {
        let text = text.into();
        let len = text.len();
        Self {
            visible: text.clone(),
            canonical: text,
            spans: (len > 0)
                .then_some(PresentationSpan {
                    encoded_start: 0,
                    encoded_end: len,
                    canonical_start: 0,
                    canonical_end: len,
                    repeated: false,
                })
                .into_iter()
                .collect(),
            source: None,
            encoding: "literal".into(),
        }
    }

    pub fn with_source(mut self, source: PresentationSource) -> Self {
        self.source = Some(source);
        self
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PresentationError {
    #[error("presentation escape is missing its escaped scalar")]
    DanglingEscape,
    #[error("repetition count is missing or invalid at byte {0}")]
    InvalidCount(usize),
    #[error("repetition body is missing an opening brace at byte {0}")]
    MissingOpenBrace(usize),
    #[error("repetition body is missing a closing brace")]
    MissingCloseBrace,
    #[error("unescaped reserved presentation character `{0}` at byte {1}")]
    UnescapedReserved(char, usize),
    #[error("encoded presentation does not match its retained canonical text")]
    CanonicalMismatch,
}

impl Presentation {
    pub fn from_canonical(canonical: impl Into<String>) -> Self {
        let canonical = canonical.into();
        let (encoded, spans) = encode_with_spans(&canonical);
        Self {
            encoded,
            canonical,
            spans,
            source: None,
        }
    }

    pub fn with_source(mut self, source: PresentationSource) -> Self {
        self.source = Some(source);
        self
    }

    /// Verify that this presentation still decodes to its retained canonical
    /// text. A caller can use this at a transport boundary before emitting it.
    pub fn verify(&self) -> Result<(), PresentationError> {
        if decode(&self.encoded)? == self.canonical && spans_cover(&self.spans, &self) {
            Ok(())
        } else {
            Err(PresentationError::CanonicalMismatch)
        }
    }
}

/// Encode canonical text using the frozen lossless grammar.
pub fn encode(value: &str) -> String {
    encode_with_spans(value).0
}

/// Encode canonical text and retain a byte-precise mapping to its source.
pub fn encode_with_spans(value: &str) -> (String, Vec<PresentationSpan>) {
    let chars = value.char_indices().collect::<Vec<_>>();
    let mut encoded = String::new();
    let mut spans = Vec::new();
    let mut cursor = 0usize;
    while cursor < chars.len() {
        let (canonical_start, character) = chars[cursor];
        let mut end = cursor + 1;
        while end < chars.len() && chars[end].1 == character {
            end += 1;
        }
        let count = end - cursor;
        let literal = escape_scalar(character);
        let repeat = format!("×{count}{{{literal}}}");
        if count >= 3 && repeat.len() < literal.len() * count {
            let encoded_start = encoded.len();
            encoded.push_str(&repeat);
            spans.push(PresentationSpan {
                encoded_start,
                encoded_end: encoded.len(),
                canonical_start,
                canonical_end: chars
                    .get(end)
                    .map(|(offset, _)| *offset)
                    .unwrap_or(value.len()),
                repeated: true,
            });
        } else {
            for (canonical_start, _) in &chars[cursor..end] {
                let encoded_start = encoded.len();
                encoded.push_str(&literal);
                spans.push(PresentationSpan {
                    encoded_start,
                    encoded_end: encoded.len(),
                    canonical_start: *canonical_start,
                    canonical_end: *canonical_start + character.len_utf8(),
                    repeated: false,
                });
            }
        }
        cursor = end;
    }
    (encoded, spans)
}

fn spans_cover(spans: &[PresentationSpan], presentation: &Presentation) -> bool {
    let mut encoded = 0usize;
    let mut canonical = 0usize;
    for span in spans {
        if span.encoded_start != encoded
            || span.canonical_start != canonical
            || span.encoded_end < span.encoded_start
            || span.canonical_end < span.canonical_start
            || !presentation.encoded.is_char_boundary(span.encoded_start)
            || !presentation.encoded.is_char_boundary(span.encoded_end)
            || !presentation
                .canonical
                .is_char_boundary(span.canonical_start)
            || !presentation.canonical.is_char_boundary(span.canonical_end)
        {
            return false;
        }
        let Some(encoded_item) = presentation
            .encoded
            .get(span.encoded_start..span.encoded_end)
        else {
            return false;
        };
        let Some(canonical_item) = presentation
            .canonical
            .get(span.canonical_start..span.canonical_end)
        else {
            return false;
        };
        if decode(encoded_item).ok().as_deref() != Some(canonical_item) {
            return false;
        }
        encoded = span.encoded_end;
        canonical = span.canonical_end;
    }
    encoded == presentation.encoded.len() && canonical == presentation.canonical.len()
}

/// Decode an encoded presentation into exact canonical text.
pub fn decode(value: &str) -> Result<String, PresentationError> {
    let mut cursor = 0usize;
    let decoded = decode_until(value, &mut cursor, false)?;
    debug_assert_eq!(cursor, value.len());
    Ok(decoded)
}

fn decode_until(
    value: &str,
    cursor: &mut usize,
    stop_at_close_brace: bool,
) -> Result<String, PresentationError> {
    let mut output = String::new();
    while *cursor < value.len() {
        let start = *cursor;
        let character = next_scalar(value, cursor).expect("cursor is in bounds");
        match character {
            '}' if stop_at_close_brace => return Ok(output),
            '}' | '{' | '§' => return Err(PresentationError::UnescapedReserved(character, start)),
            '\\' => {
                let escaped =
                    next_scalar(value, cursor).ok_or(PresentationError::DanglingEscape)?;
                output.push(escaped);
            }
            '×' => {
                let count_start = *cursor;
                let mut digits = String::new();
                while let Some(next) = value[*cursor..].chars().next() {
                    if !next.is_ascii_digit() {
                        break;
                    }
                    *cursor += next.len_utf8();
                    digits.push(next);
                }
                let count = digits
                    .parse::<usize>()
                    .ok()
                    .filter(|count| *count > 0 && !digits.starts_with('0'))
                    .ok_or(PresentationError::InvalidCount(count_start))?;
                if next_scalar(value, cursor) != Some('{') {
                    return Err(PresentationError::MissingOpenBrace(*cursor));
                }
                let body = decode_until(value, cursor, true)?;
                output.reserve(body.len().saturating_mul(count));
                for _ in 0..count {
                    output.push_str(&body);
                }
            }
            _ => output.push(character),
        }
    }
    if stop_at_close_brace {
        Err(PresentationError::MissingCloseBrace)
    } else {
        Ok(output)
    }
}

fn next_scalar(value: &str, cursor: &mut usize) -> Option<char> {
    let character = value.get(*cursor..)?.chars().next()?;
    *cursor += character.len_utf8();
    Some(character)
}

fn escape_scalar(character: char) -> String {
    match character {
        '§' | '×' | '{' | '}' | '\\' => format!("\\{character}"),
        _ => character.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_round_trip_preserves_reserved_syntax_and_unicode() {
        let canonical = "§ × { } \\ café🙂\naaaaaaaa";
        let encoded = encode(canonical);
        assert!(encoded.contains("×8{a}"));
        assert_eq!(decode(&encoded).unwrap(), canonical);
        Presentation::from_canonical(canonical).verify().unwrap();
    }

    #[test]
    fn nested_repetitions_are_unambiguous() {
        assert_eq!(
            decode("×2{left ×3{x} right}").unwrap(),
            "left xxx rightleft xxx right"
        );
    }

    #[test]
    fn spans_map_compact_and_escaped_items_to_exact_canonical_bytes() {
        let presentation = Presentation::from_canonical("§aaaa").with_source(PresentationSource {
            path: "bash://run/log".into(),
            anchors: vec!["#alpha".into()],
            occurrence: Some("event:42".into()),
        });
        assert_eq!(presentation.source.as_ref().unwrap().path, "bash://run/log");
        assert_eq!(presentation.spans.len(), 2);
        assert_eq!(
            &presentation.encoded[..presentation.spans[0].encoded_end],
            "\\§"
        );
        assert_eq!(
            &presentation.canonical
                [presentation.spans[1].canonical_start..presentation.spans[1].canonical_end],
            "aaaa"
        );
        assert!(presentation.spans[1].repeated);
        presentation.verify().unwrap();
    }

    #[test]
    fn literal_boundary_record_preserves_what_the_model_saw() {
        let presentation =
            ModelPresentation::literal("§ ordinary output").with_source(PresentationSource {
                path: "src/lib.rs".into(),
                anchors: vec!["L1".into()],
                occurrence: Some("tool:call-1".into()),
            });
        assert_eq!(presentation.encoding, "literal");
        assert_eq!(presentation.visible, "§ ordinary output");
        assert_eq!(presentation.canonical, presentation.visible);
        assert_eq!(
            presentation.spans[0].canonical_end,
            presentation.visible.len()
        );
        assert_eq!(presentation.source.unwrap().path, "src/lib.rs");
    }

    #[test]
    fn malformed_reserved_syntax_is_rejected_instead_of_guessed() {
        assert!(matches!(
            decode("×0{x}"),
            Err(PresentationError::InvalidCount(_))
        ));
        assert!(matches!(
            decode("×2{x"),
            Err(PresentationError::MissingCloseBrace)
        ));
        assert!(matches!(
            decode("§raw"),
            Err(PresentationError::UnescapedReserved('§', _))
        ));
    }

    #[test]
    fn encoder_round_trips_exhaustive_short_adversarial_text() {
        fn visit(prefix: &mut String, alphabet: &[char], remaining: usize) {
            assert_eq!(decode(&encode(prefix)).unwrap(), *prefix, "{prefix:?}");
            if remaining == 0 {
                return;
            }
            for character in alphabet {
                prefix.push(*character);
                visit(prefix, alphabet, remaining - 1);
                prefix.pop();
            }
        }
        let mut value = String::new();
        visit(&mut value, &['a', '§', '×', '{', '}', '\\', '🙂'], 4);
    }
}
