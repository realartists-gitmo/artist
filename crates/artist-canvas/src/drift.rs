//! Noticing when a canvas leaves the design system.
//!
//! This exists because the model cannot see what it built. It is the only
//! channel that reports on *appearance* rather than on whether the code ran.
//!
//! # Why it scans the AST
//!
//! The first version scanned raw lines for `#rrggbb` and colour words, and
//! flagged `href="#add"`, `getElementById("#abc")`, git SHAs, and any object on
//! a line containing the word `color`. Its own docstring argued that false
//! positives train the model to ignore the report — which is exactly what that
//! version earned. oxc has already parsed the file one function away, so the
//! check now looks at string literals in the places that actually style
//! something, and says nothing about the rest of the program.
//!
//! # Why it looks for these things
//!
//! Hard-coded colour was never the failure that mattered. What actually breaks
//! a canvas is a prop the kit does not have — React drops it silently, the
//! model sees no error, and reports success on a card with no title. That, and
//! using a light surface on a dark ground, are what this reports now.

use oxc::{
    allocator::Allocator,
    ast::ast::{Expression, JSXAttributeItem, JSXAttributeName, JSXAttributeValue, JSXElementName},
    ast_visit::{Visit, walk},
    parser::Parser,
    span::SourceType,
};

/// One thing that will not look the way the model intended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drift {
    pub line: u32,
    pub found: String,
    pub hint: &'static str,
}

/// Props every kit component accepts, so they never read as unknown.
const UNIVERSAL: &[&str] = &["key", "ref", "children", "className", "style", "id"];

/// The kit's components and the props each one actually reads.
///
/// Kept here rather than in the JS so the check runs before the browser does.
/// A component missing from this table is simply not checked — better silent
/// than wrong, given what a false positive costs.
const KIT: &[(&str, &[&str])] = &[
    ("AppShell", &["title", "subtitle", "actions", "sidebar"]),
    ("Card", &["title", "actions"]),
    ("Button", &["variant", "tone", "size", "onClick", "disabled", "type", "title"]),
    ("Badge", &["variant", "tone"]),
    ("Input", &["label", "hint", "value", "onChange", "placeholder", "type", "checked"]),
    ("Select", &["label", "options", "value", "onChange"]),
    ("Checkbox", &["label", "checked", "onChange"]),
    ("Tabs", &["tabs", "value", "onChange"]),
    ("Dialog", &["open", "title", "onClose", "actions"]),
    ("Stack", &["gap", "horizontal"]),
    ("Split", &["initial", "min", "vertical"]),
    ("EmptyState", &["title", "hint", "action"]),
    ("DataTable", &["rows", "columns", "onRowClick", "empty", "dense", "height", "filterable"]),
    ("Plot", &["data", "series", "height", "title", "scales", "kind"]),
    ("Code", &["language", "showLines", "wrap"]),
    ("Diff", &["patch", "language"]),
    ("SchemaForm", &["schema", "value", "onChange", "onSubmit", "submitLabel"]),
    ("Transcript", &["events", "height"]),
    ("ToolLog", &["events", "limit"]),
    ("Metric", &["label", "value", "variant", "tone", "hint", "trend"]),
    ("Alert", &["variant", "tone", "title"]),
    ("Markdown", &[]),
    ("FileLink", &["path", "line"]),
    ("Sparkline", &["values", "width", "height", "tone"]),
];

/// Utility classes that are a light surface, which reads as a glare on the dark
/// ground a canvas defaults to.
fn light_surface_class(class: &str) -> bool {
    let Some(rest) = class.strip_prefix("bg-") else {
        return false;
    };
    matches!(rest.rsplit('-').next(), Some("50" | "100"))
}

struct Scan<'a> {
    source: &'a str,
    found: Vec<Drift>,
}

impl<'a> Scan<'a> {
    fn line_of(&self, offset: u32) -> u32 {
        self.source[..(offset as usize).min(self.source.len())]
            .matches('\n')
            .count() as u32
            + 1
    }
}

impl<'a> Visit<'a> for Scan<'a> {
    fn visit_jsx_element(&mut self, element: &oxc::ast::ast::JSXElement<'a>) {
        // A capitalised JSX name references a binding, so oxc gives it as an
        // `IdentifierReference`; only lowercase intrinsics like `div` are a
        // plain `Identifier`. Matching one variant silently skipped every
        // component in the kit — the exact thing this check exists for.
        let component = match &element.opening_element.name {
            JSXElementName::IdentifierReference(name) => name.name.as_str(),
            JSXElementName::Identifier(name) => name.name.as_str(),
            _ => {
                walk::walk_jsx_element(self, element);
                return;
            }
        };
        let known = KIT.iter().find(|(kit, _)| *kit == component);

        for attribute in &element.opening_element.attributes {
            let JSXAttributeItem::Attribute(attribute) = attribute else {
                continue;
            };
            let JSXAttributeName::Identifier(prop) = &attribute.name else {
                continue;
            };
            let prop = prop.name.as_str();

            if let Some((_, accepted)) = known
                && !accepted.contains(&prop)
                && !UNIVERSAL.contains(&prop)
                && !prop.starts_with("aria-")
                && !prop.starts_with("data-")
            {
                self.found.push(Drift {
                    line: self.line_of(attribute.span.start),
                    found: format!("<{component} {prop}=…>"),
                    hint: "the kit does not read this prop, and React drops it silently",
                });
            }

            if prop == "className"
                && let Some(JSXAttributeValue::StringLiteral(value)) = &attribute.value
            {
                for class in value.value.split_whitespace() {
                    if light_surface_class(class) {
                        self.found.push(Drift {
                            line: self.line_of(attribute.span.start),
                            found: class.to_owned(),
                            hint: "a light surface on the dark ground a canvas defaults to; \
                                   the palette flips with the scheme, so this is already handled",
                        });
                        break;
                    }
                }
            }
        }
        walk::walk_jsx_element(self, element);
    }

    /// Colour written straight into a style object still bypasses the theme.
    fn visit_object_property(&mut self, property: &oxc::ast::ast::ObjectProperty<'a>) {
        let key = property.key.static_name().unwrap_or_default();
        let styling = matches!(
            key.as_ref(),
            "color" | "background" | "backgroundColor" | "borderColor" | "fill" | "stroke"
        );
        if styling
            && let Expression::StringLiteral(value) = &property.value
        {
            let literal = value.value.as_str();
            let hard_coded = literal.starts_with('#')
                || literal.starts_with("rgb")
                || literal.starts_with("hsl");
            if hard_coded {
                self.found.push(Drift {
                    line: self.line_of(property.span.start),
                    found: format!("{key}: {literal:?}"),
                    hint: "bypasses the theme; use a Tailwind class or a var(--a-*) token",
                });
            }
        }
        walk::walk_object_property(self, property);
    }
}

/// Scan a module for what will not look right.
pub fn scan(source: &str) -> Vec<Drift> {
    scan_as(source, "canvas.jsx")
}

/// Scan with the dialect implied by `path`.
pub fn scan_as(source: &str, path: &str) -> Vec<Drift> {
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::jsx());
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    // A file that does not parse is a compile error, which is reported by a
    // channel that positions it properly. Saying it twice helps nobody.
    if !parsed.diagnostics.is_empty() {
        return Vec::new();
    }
    let mut scan = Scan {
        source,
        found: Vec::new(),
    };
    scan.visit_program(&parsed.program);
    scan.found
}

/// Render for `canvas status`.
pub fn describe(path: &str, drift: &[Drift]) -> String {
    if drift.is_empty() {
        return String::new();
    }
    let mut out = format!("\n{} thing(s) in {path} that will not look right:\n", drift.len());
    for item in drift.iter().take(10) {
        out.push_str(&format!(
            "  {path}:{} {} — {}\n",
            item.line, item.found, item.hint
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(source: &str) -> Vec<String> {
        scan(source).into_iter().map(|d| d.found).collect()
    }

    /// The failure that actually happens: React drops an unknown prop, nothing
    /// errors, and `status` says clean while the card has no title.
    #[test]
    fn an_unknown_kit_prop_is_reported() {
        let drift = scan(r#"const a = <Card header="Results">rows</Card>;"#);
        assert_eq!(drift.len(), 1);
        assert_eq!(drift[0].found, "<Card header=…>");
        assert_eq!(drift[0].line, 1);

        assert!(found(r#"const a = <Button primary>Go</Button>;"#).contains(&"<Button primary=…>".to_owned()));
        assert!(found(r#"const a = <Badge variant="error">1</Badge>;"#).is_empty(), "variant is accepted");
    }

    #[test]
    fn props_the_kit_reads_are_not_reported() {
        assert!(found(r#"const a = <Card title="Results" actions={<X/>}>rows</Card>;"#).is_empty());
        assert!(found(r#"const a = <Card className="p-4" style={{}} key="1">x</Card>;"#).is_empty());
        assert!(found(r#"const a = <Card aria-label="results" data-test="x">y</Card>;"#).is_empty());
    }

    /// A component the table does not know is not checked. Better silent than
    /// wrong: a false positive teaches the model to ignore the whole report.
    #[test]
    fn unknown_components_are_left_alone() {
        assert!(found(r#"const a = <Metric label="x" value={1} />;"#).is_empty());
        assert!(found(r#"const a = <MyOwnThing whatever={1} />;"#).is_empty());
        assert!(found(r#"const a = <div anything="1" />;"#).is_empty());
    }

    /// Everything the line-based version flagged wrongly.
    #[test]
    fn the_old_false_positives_are_gone() {
        for innocent in [
            r##"const a = <a href="#top">back</a>;"##,
            r##"const a = <a href="#add">jump</a>;"##,
            r##"document.getElementById("#abc");"##,
            r##"const sha = "#abc123";"##,
            r#"const colorScheme = { primary: "blue" };"#,
            r#"const teamColors = { a: "red" };"#,
            r#"const url = "https://x/#section";"#,
            r#"const label = "the colour is red";"#,
        ] {
            assert!(scan(innocent).is_empty(), "false positive on: {innocent}");
        }
    }

    /// Colour written into a style object still bypasses the theme.
    #[test]
    fn hard_coded_colour_in_a_style_object_is_reported() {
        assert_eq!(found(r##"const a = <div style={{ color: "#ff0000" }} />;"##).len(), 1);
        assert_eq!(found(r#"const a = <div style={{ backgroundColor: "rgb(1,2,3)" }} />;"#).len(), 1);
        // A token or a class is the correct form and says nothing.
        assert!(found(r#"const a = <div style={{ color: "var(--a-fg)" }} />;"#).is_empty());
        // A non-styling key with a hash value is not a colour.
        assert!(found(r##"const a = { anchor: "#top" };"##).is_empty());
    }

    /// The palette flips with the scheme now, so a hand-picked light surface is
    /// both unnecessary and wrong on the dark ground.
    #[test]
    fn a_light_surface_class_is_reported() {
        let drift = scan(r#"const a = <div className="bg-red-100 text-red-800">x</div>;"#);
        assert_eq!(drift.len(), 1);
        assert_eq!(drift[0].found, "bg-red-100");

        assert!(found(r#"const a = <div className="bg-red-700 text-white">x</div>;"#).is_empty());
        assert!(found(r#"const a = <div className="text-red-100">x</div>;"#).is_empty());
    }

    /// A file that will not parse is already reported, with a position.
    #[test]
    fn a_broken_file_is_left_to_the_compiler() {
        assert!(scan("const x = ;").is_empty());
    }

    #[test]
    fn the_report_names_the_file_and_line() {
        let text = describe("parts/Chart.jsx", &scan("\n\nconst a = <Card header=\"x\" />;"));
        assert!(text.contains("parts/Chart.jsx:3"), "{text}");
        assert!(text.contains("header"), "{text}");
    }
}
