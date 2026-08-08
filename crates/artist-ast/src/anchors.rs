use std::collections::HashMap;
use std::path::Path;

use ast_grep_core::Language;
use ast_grep_language::{LanguageExt, SupportLang};

const IDENTITY_MAGIC: &[u8] = b"artist.anchor.identity.v1\0";
const MODE_STRUCTURED: &[u8] = b"structured";
const MODE_TEXT: &[u8] = b"text";
const SEMANTIC_KEY_FIELDS: &[&str] = &["name", "declarator", "path", "key", "label"];

/// Return the complete deterministic v1 occurrence identity for every logical line.
///
/// Recognized tree-sitter/ast-grep languages use one shared CST extractor. Markdown
/// uses the same extractor over Artist's existing tree-sitter-md parser. SQL uses
/// Artist's existing structural declaration parser. Unknown or unparseable text
/// falls back to exact line bytes. Every identity always carries its equivalent-
/// occurrence rank; that rank is computed before address generation and never from
/// address collisions.
pub fn line_anchor_identities(path: &Path, source: &str) -> Vec<Vec<u8>> {
    if source.is_empty() {
        return Vec::new();
    }

    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if matches!(ext.as_str(), "md" | "markdown" | "mdx" | "mdown") {
        if let Some(base) = markdown_base_identities(source) {
            return finish_identities(base);
        }
        return fallback_identities(source);
    }

    if matches!(ext.as_str(), "sql" | "ddl" | "dml") {
        return sql_identities(path, source);
    }

    let lang = SupportLang::from_path(path).or_else(|| {
        if path.extension().is_none() {
            shebang_language(source)
        } else {
            None
        }
    });

    let Some(lang) = lang else {
        return fallback_identities(source);
    };

    let root = lang.ast_grep(source);
    let raw_root = root.root().get_inner_node();
    if tree_has_errors(raw_root) {
        return fallback_identities(source);
    }

    finish_identities(structured_base_identities(
        source,
        raw_root,
        &lang.to_string().to_ascii_lowercase(),
    ))
}

fn shebang_language(source: &str) -> Option<SupportLang> {
    let first = source.lines().next()?;
    let shebang = first.strip_prefix("#!")?.trim();
    let mut tokens = shebang.split_whitespace();
    let command = tokens.next()?;
    let command_basename = command.rsplit('/').next().unwrap_or(command);
    let program = if command_basename == "env" {
        let mut program = tokens.next()?;
        while program.starts_with('-') || program.contains('=') {
            program = tokens.next()?;
        }
        program.rsplit('/').next().unwrap_or(program)
    } else {
        command_basename
    };
    let program = program
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '.')
        .to_ascii_lowercase();
    match program.as_str() {
        "python" | "pypy" => Some(SupportLang::Python),
        "ruby" | "rb" => Some(SupportLang::Ruby),
        "node" | "nodejs" | "bun" | "deno" => Some(SupportLang::TypeScript),
        "php" => Some(SupportLang::Php),
        "bash" | "sh" | "zsh" | "ksh" => Some(SupportLang::Bash),
        "lua" | "luajit" => Some(SupportLang::Lua),
        _ => None,
    }
}

fn markdown_base_identities(source: &str) -> Option<Vec<Vec<u8>>> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_md::LANGUAGE.into()).ok()?;
    let tree = parser.parse(source.as_bytes(), None)?;
    let root = tree.root_node();
    if tree_has_errors(root) {
        return None;
    }
    Some(structured_base_identities(source, root, "markdown"))
}

#[derive(Debug)]
struct StructuralFrame {
    kind: Vec<u8>,
    role: Vec<u8>,
    semantic: Option<(Vec<u8>, Vec<u8>)>,
}

fn serialize_structured_identity(
    language: &[u8],
    node_kind: &[u8],
    role: &[u8],
    ancestors: &[StructuralFrame],
    content: &[u8],
) -> Vec<u8> {
    let mut identity = Vec::new();
    identity.extend_from_slice(IDENTITY_MAGIC);
    push_field(&mut identity, 1, MODE_STRUCTURED);
    push_field(&mut identity, 2, language);
    push_field(&mut identity, 3, node_kind);
    push_field(&mut identity, 4, role);
    for ancestor in ancestors {
        let mut frame = Vec::new();
        push_field(&mut frame, 1, &ancestor.kind);
        push_field(&mut frame, 2, &ancestor.role);
        if let Some((field, value)) = &ancestor.semantic {
            push_field(&mut frame, 3, field);
            push_field(&mut frame, 4, value);
        }
        push_field(&mut identity, 5, &frame);
    }
    push_field(&mut identity, 6, content);
    identity
}

fn sql_identities(path: &Path, source: &str) -> Vec<Vec<u8>> {
    let parsed = crate::adapters::sql::parse_sql(path, source);
    if parsed.error_count != 0 {
        return fallback_identities(source);
    }

    let spans = line_spans(source);
    let mut base = Vec::with_capacity(spans.len());
    for (line_index, (start, end)) in spans.iter().copied().enumerate() {
        let line = &source[start..end];
        let line_no = line_index + 1;
        let ancestors = deepest_declaration(&parsed.declarations, line_no)
            .map(|decl| StructuralFrame {
                kind: decl
                    .native_kind
                    .as_deref()
                    .unwrap_or_else(|| decl.kind.as_str())
                    .as_bytes()
                    .to_vec(),
                role: Vec::new(),
                semantic: Some((b"name".to_vec(), decl.name.as_bytes().to_vec())),
            })
            .into_iter()
            .collect::<Vec<_>>();
        base.push(serialize_structured_identity(
            b"sql",
            b"sql_line",
            b"declaration",
            &ancestors,
            line.as_bytes(),
        ));
    }
    finish_identities(base)
}

fn deepest_declaration(
    declarations: &[crate::core::Declaration],
    line: usize,
) -> Option<&crate::core::Declaration> {
    let mut best = None;
    let mut stack: Vec<&crate::core::Declaration> = declarations.iter().collect();
    while let Some(decl) = stack.pop() {
        if decl.start_line <= line && line <= decl.end_line {
            best = Some(decl);
            stack.extend(decl.children.iter());
        }
    }
    best
}

fn structured_base_identities(
    source: &str,
    root: tree_sitter::Node<'_>,
    language: &str,
) -> Vec<Vec<u8>> {
    let spans = line_spans(source);
    let mut result = Vec::with_capacity(spans.len());

    for (start, end) in spans {
        let line = &source[start..end];
        let trimmed_start = line.len() - line.trim_start().len();
        let trimmed_end = line.trim_end().len();
        let target = if trimmed_start < trimmed_end {
            let byte_start = start + trimmed_start;
            let byte_end = start + trimmed_end;
            root.descendant_for_byte_range(byte_start, byte_end)
        } else {
            // Blank/whitespace-only lines have no leaf node of their own. Use the
            // deepest named structural container whose byte range actually contains
            // the gap. This preserves governing structure without borrowing identity
            // from either neighboring occurrence.
            Some(governing_node_at_gap(root, start))
        };
        let Some(mut target) = target else {
            result.push(text_base_identity(line.as_bytes()));
            continue;
        };
        while !target.is_named() {
            let Some(parent) = target.parent() else {
                break;
            };
            target = parent;
        }

        let mut ancestor_nodes = Vec::new();
        let mut current = target.parent();
        while let Some(node) = current {
            if node.is_named() && node.id() != root.id() {
                ancestor_nodes.push(node);
            }
            current = node.parent();
        }
        ancestor_nodes.reverse();
        let ancestors = ancestor_nodes
            .into_iter()
            .map(|ancestor| StructuralFrame {
                kind: ancestor.kind().as_bytes().to_vec(),
                role: role_in_parent(ancestor).unwrap_or("").as_bytes().to_vec(),
                semantic: semantic_key(ancestor, source)
                    .map(|(field, value)| (field.as_bytes().to_vec(), value)),
            })
            .collect::<Vec<_>>();

        let canonical = canonical_line_content(root, source, start, end);
        let content = if canonical.is_empty() && line.as_bytes().iter().all(u8::is_ascii_whitespace)
        {
            Vec::new()
        } else if canonical.is_empty() {
            line.as_bytes().to_vec()
        } else {
            canonical
        };
        result.push(serialize_structured_identity(
            language.as_bytes(),
            target.kind().as_bytes(),
            role_in_parent(target).unwrap_or("").as_bytes(),
            &ancestors,
            &content,
        ));
    }

    result
}

fn governing_node_at_gap<'tree>(
    mut node: tree_sitter::Node<'tree>,
    byte: usize,
) -> tree_sitter::Node<'tree> {
    loop {
        let mut next = None;
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.start_byte() <= byte && byte < child.end_byte() {
                next = Some(child);
                break;
            }
        }
        match next {
            Some(child) => node = child,
            None => return node,
        }
    }
}

fn semantic_key(node: tree_sitter::Node<'_>, source: &str) -> Option<(&'static str, Vec<u8>)> {
    for &field in SEMANTIC_KEY_FIELDS {
        let Some(value) = node.child_by_field_name(field) else {
            continue;
        };
        let canonical = canonical_subtree(value, source);
        if !canonical.is_empty() {
            return Some((field, canonical));
        }
    }
    None
}

fn canonical_subtree(node: tree_sitter::Node<'_>, source: &str) -> Vec<u8> {
    let mut out = Vec::new();
    canonical_leaves(node, source, node.start_byte(), node.end_byte(), &mut out);
    out
}

fn canonical_line_content(
    root: tree_sitter::Node<'_>,
    source: &str,
    start: usize,
    end: usize,
) -> Vec<u8> {
    let mut out = Vec::new();
    canonical_leaves(root, source, start, end, &mut out);
    out
}

fn canonical_leaves(
    node: tree_sitter::Node<'_>,
    source: &str,
    wanted_start: usize,
    wanted_end: usize,
    out: &mut Vec<u8>,
) {
    if node.end_byte() <= wanted_start || node.start_byte() >= wanted_end {
        return;
    }
    if node.child_count() == 0 {
        let start = node.start_byte().max(wanted_start);
        let end = node.end_byte().min(wanted_end);
        if start >= end {
            return;
        }
        let fragment = &source.as_bytes()[start..end];
        if fragment.iter().all(|byte| byte.is_ascii_whitespace()) {
            return;
        }
        push_field(out, 1, node.kind().as_bytes());
        push_field(out, 2, fragment);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        canonical_leaves(child, source, wanted_start, wanted_end, out);
    }
}

fn role_in_parent(node: tree_sitter::Node<'_>) -> Option<&'static str> {
    let parent = node.parent()?;
    let mut cursor = parent.walk();
    for (index, child) in parent.children(&mut cursor).enumerate() {
        if child.id() == node.id() {
            return parent.field_name_for_child(index as u32);
        }
    }
    None
}

fn tree_has_errors(root: tree_sitter::Node<'_>) -> bool {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            return true;
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    false
}

fn fallback_identities(source: &str) -> Vec<Vec<u8>> {
    let base = line_spans(source)
        .into_iter()
        .map(|(start, end)| text_base_identity(&source.as_bytes()[start..end]))
        .collect();
    finish_identities(base)
}

fn text_base_identity(line: &[u8]) -> Vec<u8> {
    let mut identity = Vec::new();
    identity.extend_from_slice(IDENTITY_MAGIC);
    push_field(&mut identity, 1, MODE_TEXT);
    push_field(&mut identity, 6, line);
    identity
}

fn finish_identities(base: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut ranks: HashMap<Vec<u8>, u64> = HashMap::new();
    base.into_iter()
        .map(|mut identity| {
            let rank = ranks.entry(identity.clone()).or_insert(0);
            let value = *rank;
            *rank += 1;
            push_field(&mut identity, 7, &value.to_le_bytes());
            identity
        })
        .collect()
}

fn push_field(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    out.push(tag);
    out.extend_from_slice(&(value.len() as u64).to_le_bytes());
    out.extend_from_slice(value);
}

fn line_spans(source: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor] != b'\n' {
            cursor += 1;
        }
        let mut end = cursor;
        if cursor < bytes.len() && end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        spans.push((start, end));
        if cursor < bytes.len() {
            cursor += 1;
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(path: &str, source: &str) -> Vec<Vec<u8>> {
        line_anchor_identities(Path::new(path), source)
    }

    #[test]
    fn fallback_uses_exact_line_bytes_and_equivalent_rank() {
        let a = ids("notes.unknown", "same\n same\nsame\n");
        assert_eq!(a.len(), 3);
        assert_ne!(a[0], a[1]);
        assert_ne!(a[0], a[2]);

        let b = ids("notes.unknown", "same\nsame\n");
        assert_eq!(a[0], b[0]);
        assert_eq!(a[2], b[1]);

        assert_ne!(ids("notes.unknown", "a\r"), ids("notes.unknown", "a"));
        assert_eq!(ids("notes.unknown", "a\r\n"), ids("notes.unknown", "a\n"));
    }

    #[test]
    fn extensionless_shebang_uses_supplied_source_not_filesystem_state() {
        let values = ids(
            "definitely-does-not-exist",
            "#!/usr/bin/env python3\nx = 1\n",
        );
        assert!(values.iter().all(|value| {
            value
                .windows(MODE_STRUCTURED.len())
                .any(|window| window == MODE_STRUCTURED)
        }));
    }

    #[test]
    fn rust_identity_is_structural_not_line_number_based() {
        let before = ids("x.rs", "fn a() {\n    let x = 1;\n}\n");
        let after = ids("x.rs", "// moved\nfn a() {\n    let x = 1;\n}\n");
        assert_eq!(before[1], after[2]);
    }

    #[test]
    fn structured_identity_ignores_path_and_formatting_but_tracks_semantic_governor() {
        let a = ids("one.rs", "fn alpha() {\n    let x = 1;\n}\n");
        let b = ids(
            "different/location/two.rs",
            "fn alpha(){\n    let   x=1;\n}\n",
        );
        assert_eq!(
            a[1], b[1],
            "path and token-separating whitespace are not identity"
        );

        let c = ids("one.rs", "fn beta() {\n    let x = 1;\n}\n");
        assert_ne!(
            a[1], c[1],
            "governing semantic structure must invalidate identity"
        );
    }

    #[test]
    fn blank_lines_are_ranked_within_governing_structure() {
        let before = ids(
            "x.rs",
            "fn a() {\n\n    one();\n}\nfn b() {\n\n    two();\n}\n",
        );
        let after = ids(
            "x.rs",
            "fn a() {\n\n    one();\n}\nfn b() {\n\n\n    two();\n}\n",
        );
        assert_eq!(before[1], after[1]);
    }

    #[test]
    fn structured_blank_line_indentation_is_not_identity() {
        let empty = ids("x.rs", "fn a() {\n\n    one();\n}\n");
        let spaced = ids("x.rs", "fn a() {\n    \t\n    one();\n}\n");
        assert_eq!(empty[1], spaced[1]);
    }

    #[test]
    fn equivalent_twins_only_change_by_rank() {
        let one = ids("x.rs", "fn a() {\n    ping();\n}\n");
        let two = ids("x.rs", "fn a() {\n    ping();\n    ping();\n}\n");
        assert_eq!(one[1], two[1]);
        assert_ne!(two[1], two[2]);
    }

    #[test]
    fn every_ast_grep_builtin_language_uses_the_shared_structured_path() {
        let samples = [
            ("x.sh", "echo hi\n"),
            ("x.c", "int x;\n"),
            ("x.cpp", "int x;\n"),
            ("x.cs", "class X {}\n"),
            ("x.css", "a { color: red; }\n"),
            ("x.dart", "int x = 1;\n"),
            ("x.go", "package p\n"),
            ("x.ex", "x = 1\n"),
            ("x.hs", "x = 1\n"),
            ("x.hcl", "x = 1\n"),
            ("x.html", "<p>x</p>\n"),
            ("x.java", "class X {}\n"),
            ("x.js", "let x = 1;\n"),
            ("x.json", "{\"x\":1}\n"),
            ("x.kt", "val x = 1\n"),
            ("x.lua", "x = 1\n"),
            ("x.nix", "{ x = 1; }\n"),
            ("x.php", "<?php $x = 1; ?>\n"),
            ("x.py", "x = 1\n"),
            ("x.rb", "x = 1\n"),
            ("x.rs", "fn f() {}\n"),
            ("x.scala", "val x = 1\n"),
            ("x.sol", "contract X {}\n"),
            ("x.swift", "let x = 1\n"),
            ("x.tsx", "const X = <div/>;\n"),
            ("x.ts", "let x: number = 1;\n"),
            ("x.yaml", "x: 1\n"),
        ];
        assert_eq!(samples.len(), 27);
        for (path, source) in samples {
            let values = ids(path, source);
            assert!(!values.is_empty(), "{path}");
            assert!(
                values.iter().all(|value| value
                    .windows(MODE_STRUCTURED.len())
                    .any(|window| window == MODE_STRUCTURED)),
                "{path} unexpectedly used text fallback"
            );
        }
    }

    #[test]
    fn markdown_uses_the_shared_tree_structural_path() {
        let values = ids("README.md", "# Title\n\ntext\n");
        assert!(values[0]
            .windows(MODE_STRUCTURED.len())
            .any(|w| w == MODE_STRUCTURED));
    }

    #[test]
    fn sql_uses_artist_structural_declarations() {
        let values = ids("schema.sql", "CREATE TABLE users (\n  id INT\n);\n");
        assert!(values[0]
            .windows(MODE_STRUCTURED.len())
            .any(|w| w == MODE_STRUCTURED));
    }

    #[test]
    fn sql_identity_is_path_and_line_number_independent() {
        let before = ids("one/schema.sql", "CREATE TABLE users (\n  id INT\n);\n");
        let after = ids(
            "different/schema.sql",
            "-- moved\nCREATE TABLE users (\n  id INT\n);\n",
        );
        assert_eq!(before[0], after[1]);
        assert_eq!(before[1], after[2]);
        assert_eq!(before[2], after[3]);
    }
}
