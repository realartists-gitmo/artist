//! Markdown, rendered in Rust.
//!
//! Models write markdown by default, and the `report` template was rendering
//! model-authored prose as a raw string — so `**bold**` reached the user as
//! literal asterisks. One of five templates could not display the format its
//! own content arrives in.
//!
//! Rendering here rather than vendoring a JS library keeps the binary honest
//! (nothing new reaches the browser), and lets fenced code blocks go through
//! the same `syntect` highlighting the `<Code>` component already uses — so a
//! ```rust block in a report looks like a `<Code lang="rust">` beside it.
//!
//! # Raw HTML is dropped
//!
//! The markdown here is written by a model, and the result is injected into a
//! page that holds the session key. Passing raw HTML through would make
//! `<script>` in a model's prose a script in the user's canvas, so HTML events
//! are discarded rather than escaped-and-shown: prose that happens to contain a
//! tag is far more likely to be a mistake than an intention.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Render markdown to HTML safe to inject into a canvas.
pub fn render(source: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);

    // Rewritten as a stream, then rendered in one pass. Rendering event by
    // event breaks every construct whose HTML spans more than one event —
    // `push_html` carries state, so a table came out as loose cells.
    let mut events = Vec::new();
    let mut fence: Option<(String, String)> = None;

    for event in Parser::new_ext(source, options) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match &kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("txt").to_owned()
                    }
                    CodeBlockKind::Indented => "txt".to_owned(),
                };
                fence = Some((language, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((language, code)) = fence.take() {
                    // Substituted as HTML we generated, which is why source
                    // HTML has to be dropped in the arm below rather than here.
                    events.push(Event::Html(highlighted_block(&code, &language).into()));
                }
            }
            Event::Text(text) if fence.is_some() => {
                if let Some((_, code)) = fence.as_mut() {
                    code.push_str(&text);
                }
            }
            // Raw HTML from a model's prose is dropped, not rendered.
            Event::Html(_) | Event::InlineHtml(_) => {}
            other => events.push(other),
        }
    }

    let mut out = String::with_capacity(source.len() * 2);
    pulldown_cmark::html::push_html(&mut out, events.into_iter());
    out
}

/// A fenced block, coloured by the same highlighter the `<Code>` component uses.
fn highlighted_block(code: &str, language: &str) -> String {
    let highlighted = crate::highlight::highlight(code.trim_end_matches('\n'), language, true);
    let mut out = String::from("<pre class=\"a-md-code\"><code>");
    for line in highlighted.lines {
        for span in line {
            out.push_str(&format!(
                "<span style=\"color:{}{}\">{}</span>",
                span.color,
                if span.bold { ";font-weight:600" } else { "" },
                escape(&span.text)
            ));
        }
        out.push('\n');
    }
    out.push_str("</code></pre>");
    out
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_basics_a_model_writes_render() {
        let html = render("# Title\n\nSome **bold** and `code` and a [link](https://x).");
        assert!(html.contains("<h1>Title</h1>"), "{html}");
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(html.contains("<code>code</code>"), "{html}");
        assert!(html.contains("href=\"https://x\""), "{html}");
    }

    #[test]
    fn lists_and_tables_render() {
        let html = render("- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(html.contains("<li>one</li>"), "{html}");
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("<td>1</td>"), "{html}");
    }

    /// A fenced block should look like the `<Code>` component beside it, not
    /// like undifferentiated text.
    #[test]
    fn fenced_code_is_highlighted_by_the_same_highlighter() {
        let html = render("```rust\nfn main() { let x = 1; }\n```");
        assert!(html.contains("a-md-code"), "{html}");
        // Several colours means it was actually highlighted.
        let colours: std::collections::HashSet<_> = html
            .match_indices("color:#")
            .map(|(index, _)| &html[index + 6..index + 13])
            .collect();
        assert!(colours.len() > 1, "not highlighted: {colours:?}");
        assert!(html.contains("fn"), "{html}");
    }

    #[test]
    fn an_unfenced_block_still_renders() {
        assert!(render("    indented code\n").contains("a-md-code"));
    }

    /// The markdown is model-written and lands in a page holding the session
    /// key, so a tag in prose must not become a tag in the document.
    #[test]
    fn raw_html_is_dropped() {
        // The tags go; the text between them stays, as text. That is the safe
        // outcome — prose is prose — and the tag can no longer execute.
        let html = render("Before <script>alert(1)</script> after.");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("</script"), "{html}");
        assert!(html.contains("Before"), "{html}");
        assert!(html.contains("after."), "{html}");

        let inline = render("An <img src=x onerror=alert(1)> image.");
        assert!(!inline.contains("onerror"), "{inline}");
    }

    /// Code content must be escaped, or a block containing markup escapes it.
    #[test]
    fn code_content_cannot_break_out() {
        let html = render("```html\n<script>alert(1)</script>\n```");
        assert!(!html.contains("<script>"), "{html}");
        // Highlighting splits the text across spans, so the escaped form is
        // checked piecewise rather than as one contiguous string.
        assert!(html.contains("&lt;"), "{html}");
        assert!(html.contains("&gt;"), "{html}");
    }

    #[test]
    fn empty_input_is_empty_output() {
        assert!(render("").is_empty());
    }
}
