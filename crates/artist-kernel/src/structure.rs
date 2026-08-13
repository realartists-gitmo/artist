//! Structural line analysis used by the anchor layer.
//!
//! A provider may contribute CST node-kind context. When no provider exists,
//! or parsing fails, the analyzer deliberately falls back to exact line text
//! with an empty kind chain.

use crate::AnchorInput;
use std::{fmt, path::Path};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralLine {
    pub start_byte: usize,
    pub end_byte: usize,
    pub line_text: Vec<u8>,
    pub type_kind_chain: Vec<String>,
}

impl StructuralLine {
    pub fn anchor_input(&self) -> AnchorInput {
        AnchorInput::new(self.line_text.clone(), self.type_kind_chain.clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CstError {
    Parse { message: String },
    Unsupported { path: String },
}

impl fmt::Display for CstError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse { message } => formatter.write_str(message),
            Self::Unsupported { path } => write!(formatter, "unsupported source type: {path}"),
        }
    }
}

impl std::error::Error for CstError {}

pub trait CstProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, path: &Path) -> bool;
    fn analyze(&self, bytes: &[u8]) -> Result<Vec<StructuralLine>, CstError>;
}

#[derive(Default)]
pub struct LineFallbackProvider;

impl CstProvider for LineFallbackProvider {
    fn name(&self) -> &'static str {
        "line-fallback"
    }

    fn supports(&self, _path: &Path) -> bool {
        true
    }

    fn analyze(&self, bytes: &[u8]) -> Result<Vec<StructuralLine>, CstError> {
        Ok(line_ranges(bytes)
            .into_iter()
            .map(|(start_byte, end_byte)| StructuralLine {
                start_byte,
                end_byte,
                line_text: bytes[start_byte..end_byte].to_vec(),
                type_kind_chain: Vec::new(),
            })
            .collect())
    }
}

pub struct RustCstProvider;

impl CstProvider for RustCstProvider {
    fn name(&self) -> &'static str {
        "tree-sitter-rust"
    }

    fn supports(&self, path: &Path) -> bool {
        path.extension().is_some_and(|extension| extension == "rs")
    }

    fn analyze(&self, bytes: &[u8]) -> Result<Vec<StructuralLine>, CstError> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|error| CstError::Parse {
                message: format!("configure Rust parser: {error}"),
            })?;
        let tree = parser.parse(bytes, None).ok_or_else(|| CstError::Parse {
            message: "Rust parser returned no tree".to_owned(),
        })?;
        if tree.root_node().has_error() {
            return Err(CstError::Parse {
                message: "Rust source has parse errors".to_owned(),
            });
        }

        let ranges = line_ranges(bytes);
        Ok(ranges
            .into_iter()
            .map(|(start_byte, end_byte)| StructuralLine {
                start_byte,
                end_byte,
                line_text: bytes[start_byte..end_byte].to_vec(),
                type_kind_chain: node_kind_chain(
                    tree.root_node(),
                    line_probe_byte(bytes, start_byte, end_byte),
                ),
            })
            .collect())
    }
}

pub struct StructuralAnalyzer {
    rust: RustCstProvider,
    fallback: LineFallbackProvider,
}

impl Default for StructuralAnalyzer {
    fn default() -> Self {
        Self {
            rust: RustCstProvider,
            fallback: LineFallbackProvider,
        }
    }
}

impl StructuralAnalyzer {
    pub fn analyze(&self, path: &Path, bytes: &[u8]) -> (String, Vec<StructuralLine>) {
        if self.rust.supports(path) {
            if let Ok(lines) = self.rust.analyze(bytes) {
                return (self.rust.name().to_owned(), lines);
            }
        }
        (
            self.fallback.name().to_owned(),
            self.fallback
                .analyze(bytes)
                .expect("line fallback cannot fail"),
        )
    }
}

fn line_ranges(bytes: &[u8]) -> Vec<(usize, usize)> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    for (offset, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            let mut end = offset;
            if end > start && bytes[end - 1] == b'\r' {
                end -= 1;
            }
            ranges.push((start, end));
            start = offset + 1;
        }
    }
    if start < bytes.len() {
        ranges.push((start, bytes.len()));
    }
    ranges
}

fn node_kind_chain(root: tree_sitter::Node<'_>, byte: usize) -> Vec<String> {
    let mut nodes = Vec::new();
    let mut node = root.named_descendant_for_byte_range(byte, byte + 1);
    while let Some(current) = node {
        nodes.push(current.kind().to_owned());
        node = current.parent();
    }
    nodes.reverse();
    nodes
}

fn line_probe_byte(bytes: &[u8], start_byte: usize, end_byte: usize) -> usize {
    let first_content = bytes[start_byte..end_byte]
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(start_byte, |offset| start_byte + offset);
    first_content.min(bytes.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_provider_emits_structural_kind_chains() {
        let provider = RustCstProvider;
        let lines = provider
            .analyze(b"fn main() {\n    let answer = 42;\n}\n")
            .unwrap();
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0]
                .type_kind_chain
                .iter()
                .any(|kind| kind == "function_item")
        );
        assert!(
            lines[1]
                .type_kind_chain
                .iter()
                .any(|kind| kind == "let_declaration")
        );
    }

    #[test]
    fn malformed_rust_degrades_to_line_only_analysis() {
        let analyzer = StructuralAnalyzer::default();
        let (provider, lines) = analyzer.analyze(Path::new("main.rs"), b"fn main( {\n");
        assert_eq!(provider, "line-fallback");
        assert!(lines.iter().all(|line| line.type_kind_chain.is_empty()));
    }

    #[test]
    fn unknown_extensions_use_line_only_analysis() {
        let analyzer = StructuralAnalyzer::default();
        let (provider, lines) = analyzer.analyze(Path::new("notes.txt"), b"one\ntwo\n");
        assert_eq!(provider, "line-fallback");
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.type_kind_chain.is_empty()));
    }
}
