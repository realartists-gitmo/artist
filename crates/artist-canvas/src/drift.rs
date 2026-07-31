//! Noticing when a canvas leaves the palette.
//!
//! Remapping the utilities covers what a model types by reflex, but not what it
//! types deliberately: a hard-coded `#ff0000`, an inline `style={{color:'red'}}`.
//! Those bypass the theme entirely and are exactly the thing that makes one
//! canvas look unlike every other.
//!
//! This does not prevent them. It reports them through the same channel that
//! already carries compile errors, so the model sees the drift on its next
//! `canvas status` and corrects it — the loop that already works, rather than a
//! new rule to enforce.

/// One off-palette value, positioned in the file that introduced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drift {
    pub line: u32,
    pub found: String,
    pub hint: &'static str,
}

/// Colour names CSS understands, which sidestep the palette just as a hex does.
const NAMED: &[&str] = &[
    "red", "blue", "green", "yellow", "orange", "purple", "pink", "cyan", "magenta", "lime",
    "teal", "navy", "olive", "maroon", "silver", "gold", "crimson", "salmon", "indigo",
];

/// Scan a module for values that escape the theme.
pub fn scan(source: &str) -> Vec<Drift> {
    let mut found = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let line_number = index as u32 + 1;
        // A comment explaining a colour is not a colour.
        let code = line.split("//").next().unwrap_or(line);

        if let Some(hex) = hex_literal(code) {
            found.push(Drift {
                line: line_number,
                found: hex,
                hint: "use a Tailwind class or a var(--a-*) token; hex bypasses the theme",
            });
            continue;
        }
        if code.contains("rgb(") || code.contains("rgba(") || code.contains("hsl(") {
            found.push(Drift {
                line: line_number,
                found: "rgb()/hsl()".to_owned(),
                hint: "use a Tailwind class or a var(--a-*) token",
            });
            continue;
        }
        if let Some(name) = named_colour(code) {
            found.push(Drift {
                line: line_number,
                found: name.to_owned(),
                hint: "named CSS colours are outside the palette; use a Tailwind class",
            });
        }
    }
    found
}

/// A `#rgb` or `#rrggbb` used as a value rather than as part of a URL or id.
fn hex_literal(code: &str) -> Option<String> {
    let bytes: Vec<char> = code.chars().collect();
    for (index, character) in bytes.iter().enumerate() {
        if *character != '#' {
            continue;
        }
        // `href="#top"` and `` `#${id}` `` are not colours.
        let digits: String = bytes[index + 1..]
            .iter()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if !matches!(digits.len(), 3 | 6 | 8) {
            continue;
        }
        // The run has to end there, or `#deadbeefcafe` would look like a colour.
        let after = bytes.get(index + 1 + digits.len());
        if after.is_some_and(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        return Some(format!("#{digits}"));
    }
    None
}

/// A named colour used as a CSS value, i.e. after a `:` or `=`.
fn named_colour(code: &str) -> Option<&'static str> {
    let lowered = code.to_lowercase();
    NAMED.iter().copied().find(|name| {
        [
            format!("\"{name}\""),
            format!("'{name}'"),
            format!(": {name};"),
            format!(":{name};"),
        ]
        .iter()
        .any(|pattern| lowered.contains(pattern.as_str()))
            && (lowered.contains("color") || lowered.contains("background") || lowered.contains("fill"))
    })
}

/// Render for `canvas status`.
pub fn describe(path: &str, drift: &[Drift]) -> String {
    if drift.is_empty() {
        return String::new();
    }
    let mut out = format!("\n{} off-palette value(s) in {path}:\n", drift.len());
    for item in drift.iter().take(10) {
        out.push_str(&format!(
            "  {path}:{} uses {} — {}\n",
            item.line, item.found, item.hint
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hard_coded_hex_is_reported_with_its_line() {
        let drift = scan("const a = 1;\nconst red = \"#ff0000\";\n");
        assert_eq!(drift.len(), 1);
        assert_eq!(drift[0].line, 2);
        assert_eq!(drift[0].found, "#ff0000");
    }

    #[test]
    fn short_and_alpha_hexes_count_too() {
        assert_eq!(scan("style={{color:'#f00'}}")[0].found, "#f00");
        assert_eq!(scan("style={{color:'#ff0000cc'}}")[0].found, "#ff0000cc");
    }

    /// The commonest false positives. Flagging these would train the model to
    /// ignore the report, which is worse than not reporting at all.
    #[test]
    fn anchors_ids_and_template_literals_are_not_colours() {
        for innocent in [
            "<a href=\"#top\">back</a>",
            "document.querySelector(`#${id}`)",
            "// palette note: #ff0000 was the old accent",
            "const url = \"https://x/#section\";",
        ] {
            assert!(scan(innocent).is_empty(), "false positive on: {innocent}");
        }
    }

    #[test]
    fn rgb_and_hsl_functions_are_reported() {
        assert_eq!(scan("background: rgb(255,0,0);")[0].found, "rgb()/hsl()");
        assert_eq!(scan("color: hsl(0 100% 50%);")[0].found, "rgb()/hsl()");
        // An identifier that merely contains the letters is not a call.
        assert!(scan("const rgbData = decode(buffer);").is_empty());
    }

    /// A named colour is only drift when it is being used as one.
    #[test]
    fn named_colours_are_reported_only_in_colour_context() {
        assert!(!scan("style={{ color: \"red\" }}").is_empty());
        assert!(scan("const team = \"red\";").is_empty());
        assert!(scan("<Badge tone=\"red\" />").is_empty());
    }

    #[test]
    fn a_clean_module_reports_nothing() {
        let clean = r#"
            import { Card } from "@artist/ui";
            export function App() {
              return <Card className="bg-blue-100 text-blue-700">hi</Card>;
            }
        "#;
        assert!(scan(clean).is_empty());
        assert!(describe("main.jsx", &scan(clean)).is_empty());
    }

    #[test]
    fn the_report_names_the_file_and_line() {
        let text = describe("parts/Chart.jsx", &scan("\nlet c = \"#abcdef\";"));
        assert!(text.contains("parts/Chart.jsx:2"), "{text}");
        assert!(text.contains("#abcdef"), "{text}");
    }
}
