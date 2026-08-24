use std::{collections::HashMap, sync::OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use teca::{AtomId, Neighborhood, NeighborhoodAddress, Resolution, default_lexicon};
use thiserror::Error;

use crate::{AnchoredEditOperation, TextReplacement};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AnchoredLine {
    pub line_number: u64,
    pub anchor: String,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AnchoredReadReply {
    pub revision: String,
    pub total_lines: u64,
    pub lines: Vec<AnchoredLine>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchoredEditPlan {
    pub revision: String,
    pub replacements: Vec<TextReplacement>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AnchorError {
    #[error("an anchored edit must contain at least one operation")]
    EmptyEdit,
    #[error("malformed line anchor `{0}`")]
    Malformed(String),
    #[error("unknown atom `{atom}` in line anchor `{anchor}`")]
    UnknownAtom { anchor: String, atom: String },
    #[error("line anchor `{0}` is stale for this revision")]
    Stale(String),
    #[error("line anchor `{anchor}` is ambiguous ({matches} matches)")]
    Ambiguous { anchor: String, matches: usize },
    #[error("end anchor occurs before start anchor")]
    ReversedRange,
    #[error("edit operations overlap or otherwise target incompatible byte ranges")]
    OverlappingEdits,
    #[error("TECA failed to construct a line neighborhood: {0}")]
    Teca(String),
}

#[derive(Clone, Debug)]
struct Line {
    content_start: usize,
    content_end: usize,
    line_end: usize,
    text: String,
}

/// A point-in-time, whole-resource TECA namespace for line anchors.
pub struct AnchoredDocument {
    revision: String,
    lines: Vec<Line>,
    neighborhood: Neighborhood,
}

impl AnchoredDocument {
    pub fn new(text: &str) -> Result<Self, AnchorError> {
        lexicon_reverse()?;
        let lines = split_lines(text);
        let mut occurrences: HashMap<Vec<u8>, u64> = HashMap::new();
        let mut neighborhood = Neighborhood::canonical();

        for (index, line) in lines.iter().enumerate() {
            let previous = index.checked_sub(1).map(|i| lines[i].text.as_bytes());
            let next = lines.get(index + 1).map(|line| line.text.as_bytes());
            let mut triple = Vec::new();
            push_field(&mut triple, b"line", line.text.as_bytes());
            push_optional_field(&mut triple, b"previous", previous);
            push_optional_field(&mut triple, b"next", next);
            let occurrence = occurrences.entry(triple.clone()).or_default();
            let mut identity = triple;
            push_field(&mut identity, b"occurrence", &occurrence.to_be_bytes());
            *occurrence += 1;

            neighborhood
                .insert_with_identifier(identity, (index as u64).to_be_bytes().to_vec())
                .map_err(teca_error)?;
        }

        Ok(Self {
            revision: sha256(text.as_bytes()),
            lines,
            neighborhood,
        })
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn read(&self, start_line: Option<u64>, line_count: Option<u64>) -> AnchoredReadReply {
        let start = start_line.unwrap_or(1).saturating_sub(1) as usize;
        let count = line_count.unwrap_or(u64::MAX) as usize;
        let lines = self
            .lines
            .iter()
            .enumerate()
            .skip(start)
            .take(count)
            .map(|(index, line)| AnchoredLine {
                line_number: index as u64 + 1,
                anchor: self.anchor_for(index),
                text: line.text.clone(),
            })
            .collect();
        AnchoredReadReply {
            revision: self.revision.clone(),
            total_lines: self.lines.len() as u64,
            lines,
        }
    }

    /// Resolve an internal one-based line position into its model-facing anchor.
    ///
    /// Numeric positions are permitted inside the kernel, but callers should
    /// expose the returned anchor rather than the position itself.
    pub fn line(&self, line_number: u64) -> Option<AnchoredLine> {
        let index = usize::try_from(line_number.checked_sub(1)?).ok()?;
        let line = self.lines.get(index)?;
        Some(AnchoredLine {
            line_number,
            anchor: self.anchor_for(index),
            text: line.text.clone(),
        })
    }

    /// Convert an internal one-based line plus a UTF-8 byte column to its
    /// absolute byte offset in this snapshot.
    pub fn byte_offset(&self, line_number: u64, column: usize) -> Option<usize> {
        let index = usize::try_from(line_number.checked_sub(1)?).ok()?;
        let line = self.lines.get(index)?;
        (column <= line.text.len() && line.text.is_char_boundary(column))
            .then(|| line.content_start + column)
    }

    pub fn plan_edit(
        &self,
        operations: &[AnchoredEditOperation],
    ) -> Result<AnchoredEditPlan, AnchorError> {
        if operations.is_empty() {
            return Err(AnchorError::EmptyEdit);
        }
        let mut replacements = Vec::with_capacity(operations.len());
        for operation in operations {
            let replacement = match operation {
                AnchoredEditOperation::Replace {
                    start,
                    end,
                    content,
                } => {
                    let (start, end) = self.resolve_range(start, end.as_deref())?;
                    TextReplacement {
                        start_byte: self.lines[start].content_start as u64,
                        end_byte: self.lines[end].content_end as u64,
                        text: content.clone(),
                    }
                }
                AnchoredEditOperation::Delete { start, end } => {
                    let (start, end) = self.resolve_range(start, end.as_deref())?;
                    TextReplacement {
                        start_byte: self.lines[start].content_start as u64,
                        end_byte: self.lines[end].line_end as u64,
                        text: String::new(),
                    }
                }
                AnchoredEditOperation::InsertBefore { anchor, content } => {
                    let line = self.resolve(anchor)?;
                    TextReplacement {
                        start_byte: self.lines[line].content_start as u64,
                        end_byte: self.lines[line].content_start as u64,
                        text: content.clone(),
                    }
                }
                AnchoredEditOperation::InsertAfter { anchor, content } => {
                    let line = self.resolve(anchor)?;
                    TextReplacement {
                        start_byte: self.lines[line].line_end as u64,
                        end_byte: self.lines[line].line_end as u64,
                        text: content.clone(),
                    }
                }
            };
            replacements.push(replacement);
        }
        replacements.sort_by_key(|replacement| (replacement.start_byte, replacement.end_byte));
        validate_replacements(&replacements)?;
        Ok(AnchoredEditPlan {
            revision: self.revision.clone(),
            replacements,
        })
    }

    fn anchor_for(&self, index: usize) -> String {
        let identifier = (index as u64).to_be_bytes();
        let entry = self
            .neighborhood
            .get_by_identifier(&identifier)
            .expect("every line has a TECA identifier");
        render_address(entry.address())
    }

    fn resolve_range(&self, start: &str, end: Option<&str>) -> Result<(usize, usize), AnchorError> {
        let start = self.resolve(start)?;
        let end = end
            .map(|end| self.resolve(end))
            .transpose()?
            .unwrap_or(start);
        if end < start {
            return Err(AnchorError::ReversedRange);
        }
        Ok((start, end))
    }

    fn resolve(&self, anchor: &str) -> Result<usize, AnchorError> {
        let address = parse_address(anchor)?;
        let entry = match self.neighborhood.resolve(&address).map_err(teca_error)? {
            Resolution::Unique(entry) => entry,
            Resolution::NotFound => return Err(AnchorError::Stale(anchor.into())),
            Resolution::Ambiguous { matches } => {
                return Err(AnchorError::Ambiguous {
                    anchor: anchor.into(),
                    matches,
                });
            }
        };
        let bytes: [u8; 8] = entry
            .identifier()
            .and_then(|identifier| identifier.try_into().ok())
            .ok_or_else(|| AnchorError::Teca("line identifier is corrupt".into()))?;
        Ok(u64::from_be_bytes(bytes) as usize)
    }
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn apply_replacements(
    text: &str,
    replacements: &[TextReplacement],
) -> Result<String, AnchorError> {
    validate_replacements(replacements)?;
    let mut output = text.to_owned();
    for replacement in replacements.iter().rev() {
        let start = replacement.start_byte as usize;
        let end = replacement.end_byte as usize;
        if start > end
            || end > output.len()
            || !output.is_char_boundary(start)
            || !output.is_char_boundary(end)
        {
            return Err(AnchorError::OverlappingEdits);
        }
        output.replace_range(start..end, &replacement.text);
    }
    Ok(output)
}

fn validate_replacements(replacements: &[TextReplacement]) -> Result<(), AnchorError> {
    for (index, current) in replacements.iter().enumerate() {
        if current.start_byte > current.end_byte {
            return Err(AnchorError::OverlappingEdits);
        }
        if let Some(previous) = index.checked_sub(1).map(|i| &replacements[i]) {
            let both_insert =
                previous.start_byte == previous.end_byte && current.start_byte == current.end_byte;
            if current.start_byte < previous.end_byte
                || (both_insert && current.start_byte == previous.start_byte)
            {
                return Err(AnchorError::OverlappingEdits);
            }
        }
    }
    Ok(())
}

fn split_lines(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut start = 0;
    for segment in text.split_inclusive('\n') {
        let line_end = start + segment.len();
        let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
        let content = without_lf.strip_suffix('\r').unwrap_or(without_lf);
        let content_end = start + content.len();
        lines.push(Line {
            content_start: start,
            content_end,
            line_end,
            text: content.to_owned(),
        });
        start = line_end;
    }
    if !text.is_empty() && start < text.len() {
        unreachable!("split_inclusive must consume all input");
    }
    lines
}

fn push_field(output: &mut Vec<u8>, name: &[u8], value: &[u8]) {
    output.extend_from_slice(&(name.len() as u64).to_be_bytes());
    output.extend_from_slice(name);
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

fn push_optional_field(output: &mut Vec<u8>, name: &[u8], value: Option<&[u8]>) {
    match value {
        Some(value) => {
            output.push(1);
            push_field(output, name, value);
        }
        None => {
            output.push(0);
            push_field(output, name, &[]);
        }
    }
}

fn render_address(address: &NeighborhoodAddress) -> String {
    address
        .atoms()
        .iter()
        .map(|atom| {
            std::str::from_utf8(
                default_lexicon()
                    .atom(*atom)
                    .expect("default atom is valid"),
            )
            .expect("default lexicon is UTF-8")
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn parse_address(anchor: &str) -> Result<NeighborhoodAddress, AnchorError> {
    if anchor.is_empty() || anchor.split('.').any(str::is_empty) {
        return Err(AnchorError::Malformed(anchor.into()));
    }
    let reverse = lexicon_reverse()?;
    let atoms = anchor
        .split('.')
        .map(|atom| {
            reverse
                .get(atom)
                .copied()
                .ok_or_else(|| AnchorError::UnknownAtom {
                    anchor: anchor.into(),
                    atom: atom.into(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    NeighborhoodAddress::new(atoms).map_err(|_| AnchorError::Malformed(anchor.into()))
}

fn lexicon_reverse() -> Result<&'static HashMap<String, AtomId>, AnchorError> {
    static REVERSE: OnceLock<Result<HashMap<String, AtomId>, String>> = OnceLock::new();
    REVERSE
        .get_or_init(|| {
            default_lexicon()
                .atoms()
                .enumerate()
                .map(|(index, atom)| {
                    let atom = std::str::from_utf8(atom)
                        .map_err(|_| "default TECA lexicon contains non-UTF-8".to_owned())?;
                    if atom.is_empty()
                        || !atom.chars().all(|character| character.is_alphanumeric())
                        || atom.contains(['.', ':'])
                    {
                        return Err(format!(
                            "default TECA lexicon atom is not wire-safe: {atom:?}"
                        ));
                    }
                    Ok((atom.to_owned(), AtomId(index as u32)))
                })
                .collect()
        })
        .as_ref()
        .map_err(|error| AnchorError::Teca(error.clone()))
}

fn teca_error(error: impl std::fmt::Display) -> AnchorError {
    AnchorError::Teca(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_reads_reuse_whole_document_anchors() {
        let document = AnchoredDocument::new("one\ntwo\nthree\n").unwrap();
        let repeated = AnchoredDocument::new("one\ntwo\nthree\n").unwrap();
        let all = document.read(None, None);
        assert_eq!(all, repeated.read(None, None));
        let partial = document.read(Some(2), Some(1));
        assert_eq!(partial.lines, vec![all.lines[1].clone()]);
        assert_eq!(partial.total_lines, 3);
    }

    #[test]
    fn duplicate_lines_have_distinct_round_trippable_anchors() {
        let document = AnchoredDocument::new("same\nsame\nsame\n").unwrap();
        let read = document.read(None, None);
        assert_ne!(read.lines[0].anchor, read.lines[1].anchor);
        assert_ne!(read.lines[1].anchor, read.lines[2].anchor);
        for (index, line) in read.lines.iter().enumerate() {
            assert_eq!(document.resolve(&line.anchor).unwrap(), index);
        }
    }

    #[test]
    fn plans_a_batch_against_one_snapshot() {
        let document = AnchoredDocument::new("one\ntwo\nthree\n").unwrap();
        let lines = document.read(None, None).lines;
        let plan = document
            .plan_edit(&[
                AnchoredEditOperation::Replace {
                    start: lines[0].anchor.clone(),
                    end: None,
                    content: "ONE".into(),
                },
                AnchoredEditOperation::Delete {
                    start: lines[2].anchor.clone(),
                    end: None,
                },
            ])
            .unwrap();
        assert_eq!(
            apply_replacements("one\ntwo\nthree\n", &plan.replacements).unwrap(),
            "ONE\ntwo\n"
        );
    }

    #[test]
    fn preserves_crlf_boundaries() {
        let document = AnchoredDocument::new("one\r\ntwo\r\n").unwrap();
        let lines = document.read(None, None).lines;
        let plan = document
            .plan_edit(&[AnchoredEditOperation::Replace {
                start: lines[0].anchor.clone(),
                end: None,
                content: "ONE".into(),
            }])
            .unwrap();
        assert_eq!(
            apply_replacements("one\r\ntwo\r\n", &plan.replacements).unwrap(),
            "ONE\r\ntwo\r\n"
        );
    }

    #[test]
    fn builds_a_realistically_sized_document() {
        let text = (0..100)
            .map(|index| format!("line {index}: ordinary source text\n"))
            .collect::<String>();
        let document = AnchoredDocument::new(&text).unwrap();
        assert_eq!(document.read(None, None).lines.len(), 100);
    }

    #[test]
    fn handles_blank_unicode_and_unterminated_lines() {
        let document = AnchoredDocument::new("α\n\nlast").unwrap();
        let lines = document.read(None, None).lines;
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["α", "", "last"]
        );
        assert_eq!(document.resolve(&lines[2].anchor).unwrap(), 2);
    }

    #[test]
    fn distinguishes_malformed_unknown_stale_and_ambiguous_anchors() {
        let document = AnchoredDocument::new(
            &(0..200)
                .map(|index| format!("collision candidate {index}\n"))
                .collect::<String>(),
        )
        .unwrap();
        assert!(matches!(
            document.resolve("a..b"),
            Err(AnchorError::Malformed(_))
        ));
        assert!(matches!(
            document.resolve("---"),
            Err(AnchorError::UnknownAtom { .. })
        ));
        let address = document.read(Some(1), Some(1)).lines[0].anchor.clone();
        let empty = AnchoredDocument::new("").unwrap();
        assert!(matches!(
            empty.resolve(&address),
            Err(AnchorError::Stale(_))
        ));
        let ambiguous = document
            .read(None, None)
            .lines
            .into_iter()
            .find_map(|line| {
                line.anchor
                    .split_once('.')
                    .map(|(prefix, _)| prefix.to_owned())
            })
            .expect("a large neighborhood has a multi-atom address");
        assert!(matches!(
            document.resolve(&ambiguous),
            Err(AnchorError::Ambiguous { .. })
        ));
    }

    #[test]
    fn insertions_are_snapshot_relative_and_preserve_surrounding_bytes() {
        let document = AnchoredDocument::new("one\ntwo\n").unwrap();
        let lines = document.read(None, None).lines;
        let plan = document
            .plan_edit(&[
                AnchoredEditOperation::InsertBefore {
                    anchor: lines[0].anchor.clone(),
                    content: "zero\n".into(),
                },
                AnchoredEditOperation::InsertAfter {
                    anchor: lines[1].anchor.clone(),
                    content: "three\n".into(),
                },
            ])
            .unwrap();
        assert_eq!(
            apply_replacements("one\ntwo\n", &plan.replacements).unwrap(),
            "zero\none\ntwo\nthree\n"
        );
    }

    #[test]
    fn rejects_reversed_overlapping_and_non_utf8_boundaries() {
        let document = AnchoredDocument::new("one\ntwo\n").unwrap();
        let lines = document.read(None, None).lines;
        assert_eq!(
            document
                .plan_edit(&[AnchoredEditOperation::Delete {
                    start: lines[1].anchor.clone(),
                    end: Some(lines[0].anchor.clone()),
                }])
                .unwrap_err(),
            AnchorError::ReversedRange
        );
        assert_eq!(
            document
                .plan_edit(&[
                    AnchoredEditOperation::Replace {
                        start: lines[0].anchor.clone(),
                        end: Some(lines[1].anchor.clone()),
                        content: "all".into(),
                    },
                    AnchoredEditOperation::Delete {
                        start: lines[1].anchor.clone(),
                        end: None,
                    },
                ])
                .unwrap_err(),
            AnchorError::OverlappingEdits
        );
        assert_eq!(
            apply_replacements(
                "é",
                &[TextReplacement {
                    start_byte: 1,
                    end_byte: 2,
                    text: String::new(),
                }]
            )
            .unwrap_err(),
            AnchorError::OverlappingEdits
        );
    }

    #[test]
    fn unrelated_edits_leave_distant_lines_addressable() {
        let before = AnchoredDocument::new("keep\nneighbor\nother\nchange\n").unwrap();
        let anchor = before.read(Some(1), Some(1)).lines[0].anchor.clone();
        let after = AnchoredDocument::new("keep\nneighbor\nother\nchanged\n").unwrap();
        assert_eq!(after.resolve(&anchor).unwrap(), 0);
    }
}
