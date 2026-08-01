//! JSX/TSX to plain ESM, in-process.
//!
//! This is the piece that lets a canvas exist without Node. Oxc parses,
//! transforms, and prints in the same process that serves the request, so a
//! module is compiled between the browser asking for it and the bytes going
//! out — no bundler, no watcher-triggered rebuild, no `node_modules`.
//!
//! Failures are reported as `file:line:column` diagnostics rather than a bare
//! message, because the model reads them: a canvas that fails to compile is
//! debugged through the tool result, not by the user pasting a stack trace.

use std::path::Path;

use oxc::{
    allocator::Allocator,
    codegen::{Codegen, CodegenOptions},
    parser::Parser,
    semantic::SemanticBuilder,
    span::SourceType,
    transformer::{JsxOptions, JsxRuntime, ReactRefreshOptions, TransformOptions, Transformer},
};
use serde::Serialize;

/// One compile failure, positioned in the file the model wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub message: String,
    /// 1-based, to match every editor and every compiler the model has seen.
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{path}: {}", first_message(.diagnostics))]
pub struct TransformError {
    pub path: String,
    pub diagnostics: Vec<Diagnostic>,
}

fn first_message(diagnostics: &[Diagnostic]) -> &str {
    diagnostics
        .first()
        .map(|diagnostic| diagnostic.message.as_str())
        .unwrap_or("could not be parsed")
}

/// A successfully compiled module.
#[derive(Debug, Clone)]
pub struct Transformed {
    pub code: String,
    /// Recoverable complaints. Oxc parses through most of these, so the module
    /// is still served; they ride along so `canvas status` can surface them.
    pub warnings: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Emit React Fast Refresh registrations. Requires the refresh runtime to
    /// be installed on the page, so the server turns this on only when it is
    /// also serving the prologue that defines `$RefreshReg$`.
    pub refresh: bool,
    /// Use the development JSX runtime, which threads source locations into
    /// every element. That is what makes a React error point at the model's
    /// own file instead of somewhere inside the reconciler.
    pub development: bool,
}

/// Where a module specifier sits in the source it was written in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Specifier {
    pub value: String,
    /// Byte offsets of the string literal, quotes included.
    pub start: u32,
    pub end: u32,
}

/// Every module specifier in `source`, static and dynamic, in source order.
///
/// The exporter needs these because a module inlined as a `data:` URL has no
/// base URL to resolve against — `./App.jsx` simply fails there, which was
/// confirmed in a browser before any of this was built. So relative specifiers
/// are rewritten to synthetic bare ones that the inline import map covers, and
/// the rewrite happens on the *source*: spans only mean anything against the
/// text they were parsed from, not against compiled output.
pub fn specifiers(path: &Path, source: &str) -> Vec<Specifier> {
    use oxc::{
        ast::ast,
        ast_visit::{Visit, walk},
    };

    #[derive(Default)]
    struct Collect {
        found: Vec<Specifier>,
    }

    impl Collect {
        fn take(&mut self, literal: &ast::StringLiteral<'_>) {
            self.found.push(Specifier {
                value: literal.value.to_string(),
                start: literal.span.start,
                end: literal.span.end,
            });
        }
    }

    impl<'a> Visit<'a> for Collect {
        fn visit_import_declaration(&mut self, it: &ast::ImportDeclaration<'a>) {
            self.take(&it.source);
        }
        fn visit_export_all_declaration(&mut self, it: &ast::ExportAllDeclaration<'a>) {
            self.take(&it.source);
        }
        fn visit_export_named_declaration(&mut self, it: &ast::ExportNamedDeclaration<'a>) {
            if let Some(source) = &it.source {
                self.take(source);
            }
        }
        // `import()` counts: a canvas that lazy-loads a panel would otherwise
        // export with a dangling reference and fail only once clicked.
        fn visit_import_expression(&mut self, it: &ast::ImportExpression<'a>) {
            if let ast::Expression::StringLiteral(literal) = &it.source {
                self.take(literal);
            }
            walk::walk_import_expression(self, it);
        }
    }

    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::jsx());
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();

    let mut collect = Collect::default();
    collect.visit_program(&parsed.program);
    collect.found.sort_by_key(|specifier| specifier.start);
    collect.found
}

/// Compile one module. `path` is used to pick the dialect (`.tsx` implies both
/// TypeScript and JSX) and to label diagnostics.
pub fn transform(
    path: &Path,
    source: &str,
    options: Options,
) -> Result<Transformed, TransformError> {
    let label = path.display().to_string();
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::jsx());

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(TransformError {
            path: label,
            diagnostics: convert(&parsed.diagnostics, source),
        });
    }

    let mut program = parsed.program;
    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();

    let transform_options = TransformOptions {
        jsx: JsxOptions {
            runtime: JsxRuntime::Automatic,
            development: options.development,
            refresh: options.refresh.then(ReactRefreshOptions::default),
            ..JsxOptions::default()
        },
        ..TransformOptions::default()
    };

    let result = Transformer::new(&allocator, path, &transform_options)
        .build_with_scoping(scoping, &mut program);
    if !result.diagnostics.is_empty() {
        return Err(TransformError {
            path: label,
            diagnostics: convert(&result.diagnostics, source),
        });
    }

    let code = Codegen::new()
        .with_options(CodegenOptions::default())
        .build(&program)
        .code;

    Ok(Transformed {
        code,
        warnings: Vec::new(),
    })
}

fn convert(errors: &[oxc::diagnostics::OxcDiagnostic], source: &str) -> Vec<Diagnostic> {
    errors
        .iter()
        .map(|error| {
            let offset = error
                .labels
                .first()
                .map(|label| label.offset() as usize)
                .unwrap_or(0);
            let (line, column) = line_column(source, offset);
            Diagnostic {
                message: error.message.to_string(),
                line,
                column,
            }
        })
        .collect()
}

/// Byte offset to 1-based line and column.
///
/// Column counts characters rather than bytes so that a diagnostic on a line
/// containing non-ASCII still points where the model would count to.
fn line_column(source: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(source.len());
    let head = &source[..offset];
    let line = head.matches('\n').count() as u32 + 1;
    let line_start = head.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let column = source[line_start..offset].chars().count() as u32 + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn compile(name: &str, source: &str, options: Options) -> Result<Transformed, TransformError> {
        transform(&PathBuf::from(name), source, options)
    }

    #[test]
    fn jsx_becomes_automatic_runtime_calls() {
        let output = compile(
            "App.jsx",
            "export default function App() { return <div className=\"a\">hi</div>; }",
            Options::default(),
        )
        .expect("valid jsx");

        // Automatic runtime means the module imports the factory itself; a
        // canvas never has to remember to `import React`.
        assert!(output.code.contains("react/jsx-runtime"), "{}", output.code);
        assert!(!output.code.contains("<div"), "{}", output.code);
    }

    #[test]
    fn typescript_annotations_are_stripped() {
        let output = compile(
            "App.tsx",
            "type P = { n: number };\nexport function App(p: P) { return <b>{p.n}</b>; }",
            Options::default(),
        )
        .expect("valid tsx");

        assert!(!output.code.contains("type P"), "{}", output.code);
        assert!(!output.code.contains(": P"), "{}", output.code);
    }

    #[test]
    fn refresh_registers_components_only_when_enabled() {
        let source = "export default function App() { return <div />; }";

        let plain = compile("App.jsx", source, Options::default()).expect("valid jsx");
        assert!(!plain.code.contains("$RefreshReg$"), "{}", plain.code);

        let refreshed = compile(
            "App.jsx",
            source,
            Options {
                refresh: true,
                ..Options::default()
            },
        )
        .expect("valid jsx");
        assert!(
            refreshed.code.contains("$RefreshReg$"),
            "{}",
            refreshed.code
        );
    }

    #[test]
    fn a_syntax_error_reports_the_line_and_column_the_model_wrote() {
        let error = compile(
            "Broken.jsx",
            "export default function App() {\n  const x = ;\n  return <div />;\n}\n",
            Options::default(),
        )
        .expect_err("missing initializer");

        assert_eq!(error.path, "Broken.jsx");
        let first = error.diagnostics.first().expect("a diagnostic");
        assert_eq!(first.line, 2, "{:?}", error.diagnostics);
        assert_eq!(first.column, 13, "{:?}", error.diagnostics);
    }

    #[test]
    fn line_column_is_one_based_and_counts_characters() {
        assert_eq!(line_column("abc", 0), (1, 1));
        assert_eq!(line_column("abc\ndef", 4), (2, 1));
        assert_eq!(line_column("abc\ndef", 6), (2, 3));
        // A multi-byte lead must not inflate the column past what a human counts.
        assert_eq!(line_column("é1", 3), (1, 3));
    }
}
