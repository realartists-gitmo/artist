//! Temporary, stateless TECA line addressing.
//!
//! TECA deliberately lives behind a small seam. The current byte encoding is
//! deterministic and testable, but it is not a public compatibility promise;
//! changing it later must only require changing `address_bytes` and the
//! decoder below.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TecaLine {
    pub anchor: String,
    pub content: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TecaSpan {
    pub anchor: String,
    pub start: usize,
    pub end: usize,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TecaError {
    InvalidAddress(String),
    Stale(String),
    Ambiguous(String),
}

impl std::fmt::Display for TecaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAddress(value) => write!(f, "invalid TECA address: {value}"),
            Self::Stale(value) => write!(f, "stale TECA address: {value}"),
            Self::Ambiguous(value) => write!(f, "ambiguous TECA address: {value}"),
        }
    }
}

impl std::error::Error for TecaError {}

#[derive(Clone, Debug, Default)]
pub struct TecaSnapshot {
    lines: Vec<TecaLine>,
}

impl TecaSnapshot {
    pub fn from_source(source: &str) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        let mut parents: Vec<(usize, String)> = Vec::new();
        let mut sibling_ranks = BTreeMap::<(String, String), usize>::new();

        for raw in source.split_inclusive('\n') {
            let content = raw.trim_end_matches(['\n', '\r']).to_owned();
            let indent = content.len() - content.trim_start_matches([' ', '\t']).len();
            while parents
                .last()
                .is_some_and(|(parent_indent, _)| *parent_indent >= indent)
            {
                parents.pop();
            }
            let parent = parents
                .iter()
                .map(|(_, shape)| shape.as_str())
                .collect::<Vec<_>>()
                .join("/");
            let normalized = normalize_content(&content);
            let rank_key = (parent.clone(), normalized.clone());
            let rank = sibling_ranks.entry(rank_key).or_default();
            let current_rank = *rank;
            *rank += 1;
            let parent_hash = digest_hex(parent.as_bytes());
            let content_hash = digest_hex(&address_bytes(&parent, &normalized, current_rank));
            let anchor = format!(
                "teca:v1:{parent_hash}:{content_hash}:{current_rank:x}:{:x}",
                content.len()
            );
            let end = start + content.len();
            lines.push(TecaLine {
                anchor,
                content,
                start,
                end,
            });
            if opens_scope(raw) {
                parents.push((indent, normalize_scope(&lines.last().unwrap().content)));
            }
            start += raw.len();
        }

        // `split_inclusive` omits the final empty line after a trailing
        // newline; that is presentation-only and must not become an editable
        // identity.
        Self { lines }
    }

    pub fn lines(&self) -> &[TecaLine] {
        &self.lines
    }

    pub fn anchors(&self) -> Vec<String> {
        self.lines.iter().map(|line| line.anchor.clone()).collect()
    }

    pub fn resolve(&self, address: &str) -> Result<TecaSpan, TecaError> {
        let (parent_hash, content_hash, rank, length) = decode_address(address)?;
        let matches: Vec<_> = self
            .lines
            .iter()
            .enumerate()
            .filter(|(index, line)| {
                let (parent, normalized) = self.semantic_parts_at(*index);
                digest_hex(parent.as_bytes()) == parent_hash
                    && digest_hex(&address_bytes(&parent, &normalized, rank)) == content_hash
                    && sibling_rank(self, *index, &parent, &normalized) == rank
                    && line.content.len() == length
            })
            .map(|(_, line)| line)
            .collect();
        match matches.as_slice() {
            [line] => Ok(TecaSpan {
                anchor: line.anchor.clone(),
                start: line.start,
                end: line.end,
                content: line.content.clone(),
            }),
            [] => Err(TecaError::Stale(address.to_owned())),
            _ => Err(TecaError::Ambiguous(address.to_owned())),
        }
    }

    fn semantic_parts_at(&self, target_index: usize) -> (String, String) {
        let mut parents: Vec<(usize, String)> = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            let indent = line.content.len() - line.content.trim_start_matches([' ', '\t']).len();
            while parents
                .last()
                .is_some_and(|(parent_indent, _)| *parent_indent >= indent)
            {
                parents.pop();
            }
            let parent = parents
                .iter()
                .map(|(_, shape)| shape.as_str())
                .collect::<Vec<_>>()
                .join("/");
            let normalized = normalize_content(&line.content);
            if index == target_index {
                return (parent, normalized);
            }
            if opens_scope(&line.content) {
                parents.push((indent, normalize_scope(&line.content)));
            }
        }
        (String::new(), String::new())
    }
}

fn sibling_rank(
    snapshot: &TecaSnapshot,
    target_index: usize,
    parent: &str,
    normalized: &str,
) -> usize {
    let mut rank = 0;
    for (index, _line) in snapshot.lines.iter().enumerate() {
        if index == target_index {
            break;
        }
        let (candidate_parent, candidate_content) = snapshot.semantic_parts_at(index);
        if candidate_parent == parent && candidate_content == normalized {
            rank += 1;
        }
    }
    rank
}

fn normalize_content(content: &str) -> String {
    let mut output = content.trim().to_owned();
    for keyword in [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "mod ",
        "impl ",
        "class ",
        "namespace ",
        "type ",
        "const ",
        "let ",
        "var ",
    ] {
        if let Some(start) = output.find(keyword) {
            let name_start = start + keyword.len();
            let name_end = output[name_start..]
                .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .map(|offset| name_start + offset)
                .unwrap_or(output.len());
            if name_end > name_start {
                output.replace_range(name_start..name_end, "_");
            }
        }
    }
    output
}

fn normalize_scope(content: &str) -> String {
    normalize_content(content.trim_end_matches('{').trim())
}

fn opens_scope(content: &str) -> bool {
    content.trim_end().ends_with('{')
}

fn digest_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_address(address: &str) -> Result<(String, String, usize, usize), TecaError> {
    let mut parts = address.split(':');
    if parts.next() != Some("teca") || parts.next() != Some("v1") {
        return Err(TecaError::InvalidAddress(address.to_owned()));
    }
    let parent = parts
        .next()
        .ok_or_else(|| TecaError::InvalidAddress(address.into()))?;
    let content = parts
        .next()
        .ok_or_else(|| TecaError::InvalidAddress(address.into()))?;
    let rank = parts
        .next()
        .and_then(|value| usize::from_str_radix(value, 16).ok())
        .ok_or_else(|| TecaError::InvalidAddress(address.into()))?;
    let length = parts
        .next()
        .and_then(|value| usize::from_str_radix(value, 16).ok())
        .ok_or_else(|| TecaError::InvalidAddress(address.into()))?;
    if parts.next().is_some() {
        return Err(TecaError::InvalidAddress(address.to_owned()));
    }
    Ok((parent.to_owned(), content.to_owned(), rank, length))
}

/// Exact byte input for the current TECA digest. TODO(teca): this temporary
/// serialization is not a stable public contract; replace it only behind
/// this function and the matching decoder.
pub fn address_bytes(parent: &str, content: &str, rank: usize) -> Vec<u8> {
    format!("{parent}\0{content}\0{rank}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_not_line_numbers_and_detect_stale_content() {
        let snapshot = TecaSnapshot::from_source("fn main() {\n    let value = 1;\n}\n");
        let anchor = snapshot.lines()[1].anchor.clone();
        let shifted = TecaSnapshot::from_source("// comment\nfn main() {\n    let value = 1;\n}\n");
        assert!(shifted.resolve(&anchor).is_ok());
        let stale = TecaSnapshot::from_source("fn main() {\n    let value = 2;\n}\n");
        assert!(matches!(stale.resolve(&anchor), Err(TecaError::Stale(_))));
    }

    #[test]
    fn descendant_survives_ancestor_name_change() {
        let before = TecaSnapshot::from_source("fn old_name() {\n    let value = 1;\n}\n");
        let anchor = before.lines()[1].anchor.clone();
        let after = TecaSnapshot::from_source("fn new_name() {\n    let value = 1;\n}\n");
        assert!(after.resolve(&anchor).is_ok());
    }

    #[test]
    fn equivalent_siblings_rank_within_their_parent() {
        let snapshot = TecaSnapshot::from_source(
            "fn a() {\n    let x = 1;\n    let x = 1;\n}\nfn b() {\n    let x = 1;\n}\n",
        );
        let first = snapshot.lines()[1].anchor.clone();
        let second = snapshot.lines()[2].anchor.clone();
        let sibling = snapshot.lines()[4].anchor.clone();
        assert_ne!(first, second);
        assert_ne!(first, sibling);
        assert!(snapshot.resolve(&second).is_ok());
        assert!(snapshot.resolve(&sibling).is_ok());
    }
}
