//! Reading named values off a surface without putting the surface in context.
//!
//! The cost of "what is the order total?" today is a whole observation: six
//! hundred lines of tree rendered into the conversation so the model can find
//! one number in it, after which the six hundred lines stay there forever. This
//! answers the question and leaves the page outside.
//!
//! **No model call.** The structure a form or a spec table has is already in the
//! tree — a labelled field carries its value, and where it does not, the label
//! and the value are adjacent nodes and nothing but their adjacency relates
//! them. Three strategies cover that, all deterministic, all cheap, all
//! explainable when they get it wrong:
//!
//! 1. The node named by the label **has** a value. A textbox called "Total"
//!    whose contents are `$42.00`.
//! 2. The label and the value are **one string**, separated by a colon or a
//!    dash: `Total: $42.00`.
//! 3. The value is the **next node in document order**. `Total` then `$42.00`,
//!    which is what a two-column table and most of the web look like.
//!
//! Document order, not geometry, because a PTY row and many CDP nodes have no
//! bounds at all and the pairing has to work at every rung.
//!
//! **Prose is handled too, and still without a model.** "Summarise the refund
//! policy" looked like it needed a side-model reading the page. It does not.
//! The question is really *"give me the refund policy"* followed by a summary,
//! and the first half is structural: a heading names a section, and the section
//! is the nodes that follow it until the next heading. So a field matching a
//! heading returns that section's text — a few hundred words instead of the
//! whole page — and the model already in the conversation does the summarising
//! it is good at, on an input small enough not to matter.
//!
//! That is better than the side-model call it replaces on every axis: no second
//! model to choose, host or pay for, no extra latency, and the reasoning is done
//! by the model that actually knows what the task is.

use crate::anchors::AnchorBook;
use crate::model::{Node, Role};
use crate::program::normalize;

/// Separators that mean "label on the left, value on the right".
///
/// Deliberately short. A longer list starts splitting values that legitimately
/// contain punctuation — a time is `14:30`, and a dash sits inside plenty of
/// identifiers.
const SEPARATORS: &[char] = &[':', '=', '\u{2013}', '\u{2014}'];

/// How much of a section to return.
///
/// Generous, because the point is to replace a whole-page observation and a few
/// hundred words is already an enormous saving. Bounded, because a heading with
/// no following heading would otherwise return the rest of the document and
/// undo the saving entirely.
const SECTION_CAP: usize = 4_000;

/// How far past the label to look for a value.
///
/// One, in the ordinary case: the next node. Two exists because a label is
/// frequently followed by an empty decorative node — a spacer, an icon, a
/// styling wrapper that survived into the tree. Three would start reaching into
/// the *next* row of a table and answering with a neighbour's data, which is
/// the one failure here that is worse than not answering.
const LOOKAHEAD: usize = 2;

/// Where a value came from, so a wrong answer can be understood.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The labelled node carried the value itself.
    Value,
    /// The label and the value were one string.
    Inline,
    /// The value was the following node.
    Adjacent,
    /// The label was a heading, and this is the section under it.
    Section,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::Value => "field",
            Self::Inline => "inline",
            Self::Adjacent => "next",
            Self::Section => "section",
        }
    }
}

/// One answered field.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub field: String,
    pub value: String,
    pub source: Source,
    /// The anchor of the node the label matched, so the model can act on the
    /// thing it just read without a second call to locate it.
    pub anchor: String,
    /// Set when more than one node matched the label. The first in document
    /// order is answered with; the count is reported because "there were four
    /// things called Price" is the difference between an answer and a guess.
    pub competing: usize,
}

/// Pull each requested field out of the live anchor set.
///
/// Returns one entry per field in the order asked, `None` where nothing
/// matched — an absent field is an answer, and silently dropping it would let
/// the model read a shorter list than it requested and not notice.
pub fn extract(book: &AnchorBook, fields: &[String]) -> Vec<(String, Option<Found>)> {
    let ordered: Vec<(&String, &Node)> = book.in_order().collect();
    fields
        .iter()
        .map(|field| (field.clone(), find_field(&ordered, field)))
        .collect()
}

fn find_field(ordered: &[(&String, &Node)], field: &str) -> Option<Found> {
    let wanted = normalize(field);
    if wanted.is_empty() {
        return None;
    }

    let mut answer: Option<Found> = None;
    let mut competing = 0usize;

    for (index, (anchor, node)) in ordered.iter().enumerate() {
        let name = normalize(&node.name);
        let matched = name == wanted || starts_with_label(&name, &wanted);
        if !matched {
            continue;
        }
        competing += 1;
        if answer.is_some() {
            // Later matches are counted, not considered. Document order is the
            // tiebreak because it is the order a person reads in, and because
            // any other rule here would be arbitrary.
            continue;
        }

        // A heading names a *section*, not a value. Answering it with the next
        // node would return the first line of a policy and call it the policy.
        //
        // And when the section is empty the answer is *nothing*, never the
        // strategies below: the node after an empty heading is the next
        // heading, so falling through answered "Refund policy" with the words
        // "Privacy policy" — a confidently wrong answer, which is worse than a
        // missing one because it looks exactly like a correct one. This cost a
        // test.
        if matches!(node.role, Role::Heading) {
            if let Some(text) = section_after(ordered, index) {
                answer = Some(Found {
                    field: field.to_owned(),
                    value: text,
                    source: Source::Section,
                    anchor: (*anchor).clone(),
                    competing: 0,
                });
            }
            continue;
        }

        // 1. The labelled node carries the value.
        if let Some(value) = node.value.as_deref().map(str::trim)
            && !value.is_empty()
        {
            answer = Some(Found {
                field: field.to_owned(),
                value: value.to_owned(),
                source: Source::Value,
                anchor: (*anchor).clone(),
                competing: 0,
            });
            continue;
        }

        // 2. Label and value are one string.
        if let Some(value) = split_inline(&node.name, &wanted) {
            answer = Some(Found {
                field: field.to_owned(),
                value,
                source: Source::Inline,
                anchor: (*anchor).clone(),
                competing: 0,
            });
            continue;
        }

        // 3. The value is the next node that says anything.
        if let Some(value) = following_value(ordered, index) {
            answer = Some(Found {
                field: field.to_owned(),
                value,
                source: Source::Adjacent,
                anchor: (*anchor).clone(),
                competing: 0,
            });
        }
    }

    answer.map(|found| Found {
        competing: competing.saturating_sub(1),
        ..found
    })
}

/// Whether `name` is the label followed by a separator rather than a longer word.
///
/// `Total:` is the field "Total". `Totally different` is not, and matching it
/// would answer a question about the order total with a sentence about
/// something else — the failure mode this whole module has to avoid, because
/// unlike a missing answer it looks exactly like a correct one.
fn starts_with_label(name: &str, wanted: &str) -> bool {
    let Some(rest) = name.strip_prefix(wanted) else {
        return false;
    };
    rest.starts_with(SEPARATORS)
}

fn split_inline(name: &str, wanted: &str) -> Option<String> {
    let normalized = normalize(name);
    if !starts_with_label(&normalized, wanted) {
        return None;
    }
    // Split the *original* string rather than the normalized one: normalizing
    // is for matching, and returning a case-folded, mnemonic-stripped value
    // would hand back something the surface never said.
    let (_, rest) = name.split_once(SEPARATORS)?;
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_owned())
}

/// Everything under a heading, up to the next one.
///
/// Stops at the next heading of *any* level rather than tracking depth: a
/// subsection belongs to the section above it, so including it is right, and
/// stopping at a sibling heading is what keeps one policy from swallowing the
/// next.
fn section_after(ordered: &[(&String, &Node)], index: usize) -> Option<String> {
    let heading = normalize(&ordered[index].1.name);
    let mut text = String::new();
    let mut last = String::new();
    for (_, node) in ordered.iter().skip(index + 1) {
        if matches!(node.role, Role::Heading) {
            break;
        }
        for part in [node.name.trim(), node.value.as_deref().unwrap_or("").trim()] {
            if part.is_empty() {
                continue;
            }
            // An accessibility tree names a control *and* its text child
            // identically — a button "Request refund" contains a text node
            // saying "Request refund" — and a heading's own text child repeats
            // the heading. Both put the same words in twice, which reads as a
            // stutter and wastes the context this mode exists to save.
            let normalized = normalize(part);
            if normalized == last || normalized == heading {
                continue;
            }
            last = normalized;
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(part);
        }
        if text.len() >= SECTION_CAP {
            // Truncated on a character boundary, and said so — a silently cut
            // policy reads as a complete one.
            let mut cut = SECTION_CAP.min(text.len());
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push_str(" …(section continues)");
            break;
        }
    }
    (!text.trim().is_empty()).then(|| text.trim().to_owned())
}

/// The first thing after `index` that actually says something.
fn following_value(ordered: &[(&String, &Node)], index: usize) -> Option<String> {
    for (_, node) in ordered.iter().skip(index + 1).take(LOOKAHEAD) {
        if let Some(value) = node.value.as_deref().map(str::trim)
            && !value.is_empty()
        {
            return Some(value.to_owned());
        }
        let name = node.name.trim();
        if !name.is_empty() {
            return Some(name.to_owned());
        }
    }
    None
}

/// Render the answers.
///
/// Flat `field = value` lines rather than the observation format, because this
/// is an answer and not a description of a screen. The source is named on each
/// line: "the labelled node carried this" and "this was the text next to the
/// label" are different levels of confidence, and the model is the one that has
/// to decide whether to trust the second.
pub fn render(surface: &str, found: &[(String, Option<Found>)]) -> String {
    let answered = found.iter().filter(|(_, hit)| hit.is_some()).count();
    let mut out = format!(
        "<extract surface=\"{surface}\" found=\"{answered}/{}\">\n",
        found.len()
    );
    for (field, hit) in found {
        match hit {
            Some(hit) => {
                out.push_str(&format!(
                    "{field} = {:?} ({}, {})",
                    crate::render::truncate_public(&hit.value),
                    hit.source.label(),
                    hit.anchor
                ));
                if hit.competing > 0 {
                    out.push_str(&format!(
                        " [{} other element(s) share this label]",
                        hit.competing
                    ));
                }
                out.push('\n');
            }
            None => out.push_str(&format!("{field} = not found\n")),
        }
    }
    if answered < found.len() {
        out.push_str(
            "Fields marked not found are not on this surface under that name. \
             `find` will show what it does call things.\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Snapshot;

    fn node(binding: &str, role: Role, name: &str) -> Node {
        Node::new(binding, role, name)
    }

    fn book_of(nodes: Vec<Node>) -> AnchorBook {
        let mut book = AnchorBook::new();
        book.observe(&Snapshot::new(nodes), true);
        book
    }

    fn want(fields: &[&str]) -> Vec<String> {
        fields.iter().map(|field| (*field).to_string()).collect()
    }

    #[test]
    fn a_labelled_field_answers_with_its_own_value() {
        let book = book_of(vec![node("a", Role::TextBox, "Total").with_value("$42.00")]);
        let found = extract(&book, &want(&["Total"]));
        let hit = found[0].1.as_ref().unwrap();
        assert_eq!(hit.value, "$42.00");
        assert_eq!(hit.source, Source::Value);
    }

    #[test]
    fn a_label_and_value_in_one_string_are_split() {
        let book = book_of(vec![node("a", Role::Text, "Total: $42.00")]);
        let hit = extract(&book, &want(&["Total"]))[0].1.clone().unwrap();
        assert_eq!(hit.value, "$42.00");
        assert_eq!(hit.source, Source::Inline);
    }

    #[test]
    fn a_value_in_the_next_node_is_paired_with_its_label() {
        // The two-column table, which is most of the web.
        let book = book_of(vec![
            node("a", Role::Text, "Total"),
            node("b", Role::Text, "$42.00"),
        ]);
        let hit = extract(&book, &want(&["Total"]))[0].1.clone().unwrap();
        assert_eq!(hit.value, "$42.00");
        assert_eq!(hit.source, Source::Adjacent);
    }

    #[test]
    fn pairing_follows_document_order_not_hash_order() {
        // The live set is a HashMap. If pairing walked it, "Total" would be
        // paired with whichever node hashing happened to put next — a wrong
        // answer that looks exactly like a right one, and a different wrong
        // answer on every run.
        let mut nodes = vec![node("label", Role::Text, "Total")];
        for index in 0..40 {
            nodes.push(node(&format!("n{index}"), Role::Text, &format!("{index}")));
        }
        nodes.insert(1, node("value", Role::Text, "$42.00"));
        let book = book_of(nodes);
        for _ in 0..8 {
            let hit = extract(&book, &want(&["Total"]))[0].1.clone().unwrap();
            assert_eq!(hit.value, "$42.00", "pairing must be reproducible");
        }
    }

    #[test]
    fn a_longer_word_starting_with_the_label_is_not_the_label() {
        // "Totally different" is not the order total, and answering with it
        // would be indistinguishable from a correct answer.
        let book = book_of(vec![
            node("a", Role::Text, "Totally different thing"),
            node("b", Role::Text, "nonsense"),
        ]);
        assert!(extract(&book, &want(&["Total"]))[0].1.is_none());
    }

    #[test]
    fn a_label_with_no_value_after_it_is_reported_missing() {
        let book = book_of(vec![node("a", Role::Text, "Total")]);
        let found = extract(&book, &want(&["Total"]));
        assert!(found[0].1.is_none());

        let rendered = render("win:1", &found);
        assert!(rendered.contains("Total = not found"), "{rendered}");
        assert!(
            rendered.contains("find"),
            "the recovery must be named: {rendered}"
        );
    }

    #[test]
    fn every_requested_field_comes_back_even_when_absent() {
        // Dropping the misses would let the model read a shorter list than it
        // asked for and not notice which one went missing.
        let book = book_of(vec![node("a", Role::TextBox, "Total").with_value("$42.00")]);
        let found = extract(&book, &want(&["Total", "Delivery date", "Tax"]));
        assert_eq!(found.len(), 3);
        assert_eq!(
            found
                .iter()
                .map(|(field, _)| field.as_str())
                .collect::<Vec<_>>(),
            vec!["Total", "Delivery date", "Tax"],
            "fields must come back in the order they were asked for"
        );
        assert!(found[1].1.is_none() && found[2].1.is_none());
    }

    #[test]
    fn a_repeated_label_answers_once_and_says_how_many_others() {
        let book = book_of(vec![
            node("a", Role::Text, "Price"),
            node("b", Role::Text, "$1.00"),
            node("c", Role::Text, "Price"),
            node("d", Role::Text, "$2.00"),
        ]);
        let hit = extract(&book, &want(&["Price"]))[0].1.clone().unwrap();
        assert_eq!(hit.value, "$1.00", "the first in document order wins");
        assert_eq!(hit.competing, 1);

        let rendered = render("win:1", &extract(&book, &want(&["Price"])));
        assert!(rendered.contains("share this label"), "{rendered}");
    }

    #[test]
    fn a_decorative_node_between_label_and_value_is_stepped_over() {
        let book = book_of(vec![
            node("a", Role::Text, "Total"),
            node("spacer", Role::Image, ""),
            node("b", Role::Text, "$42.00"),
        ]);
        let hit = extract(&book, &want(&["Total"]))[0].1.clone().unwrap();
        assert_eq!(hit.value, "$42.00");
    }

    #[test]
    fn the_search_does_not_reach_into_the_next_row() {
        // Beyond the lookahead the next thing is another record's data, and
        // answering with it is worse than not answering.
        let mut nodes = vec![node("a", Role::Text, "Total")];
        for index in 0..LOOKAHEAD {
            nodes.push(node(&format!("gap{index}"), Role::Image, ""));
        }
        nodes.push(node("other", Role::Text, "someone else's number"));
        let book = book_of(nodes);
        assert!(extract(&book, &want(&["Total"]))[0].1.is_none());
    }

    #[test]
    fn the_answer_keeps_the_surfaces_own_spelling() {
        // Normalization is for matching. Handing back a case-folded value would
        // report something the surface never said.
        let book = book_of(vec![node("a", Role::Text, "Status: DELIVERED")]);
        let hit = extract(&book, &want(&["status"]))[0].1.clone().unwrap();
        assert_eq!(hit.value, "DELIVERED");
    }

    #[test]
    fn the_anchor_that_matched_comes_back_with_the_answer() {
        // So reading a field and then acting on it is one call, not two.
        let book = book_of(vec![node("a", Role::TextBox, "Total").with_value("$42.00")]);
        let found = extract(&book, &want(&["Total"]));
        let hit = found[0].1.as_ref().unwrap();
        assert!(
            book.resolve(&hit.anchor).is_ok(),
            "{} did not resolve",
            hit.anchor
        );
    }

    #[test]
    fn an_empty_field_name_matches_nothing_rather_than_everything() {
        let book = book_of(vec![node("a", Role::Text, "Total")]);
        assert!(extract(&book, &want(&["", "   "]))[0].1.is_none());
    }

    fn heading(binding: &str, name: &str) -> Node {
        Node::new(binding, Role::Heading, name)
    }

    #[test]
    fn a_heading_returns_its_section_rather_than_its_first_line() {
        // The prose case, and the reason it needs no side-model: the question
        // "summarise the refund policy" is really "give me the refund policy"
        // followed by a summary, and the first half is structural.
        let book = book_of(vec![
            heading("h1", "Refund policy"),
            node("p1", Role::Text, "Refunds are available within 30 days."),
            node("p2", Role::Text, "Shipping is not refunded."),
            heading("h2", "Privacy policy"),
            node("p3", Role::Text, "We keep your data forever."),
        ]);
        let hit = extract(&book, &want(&["Refund policy"]))[0]
            .1
            .clone()
            .unwrap();
        assert_eq!(hit.source, Source::Section);
        assert!(hit.value.contains("within 30 days"), "{}", hit.value);
        assert!(
            hit.value.contains("Shipping is not refunded"),
            "{}",
            hit.value
        );
        // And it stops at the next heading — one policy must not swallow the
        // next, which is the failure that would make the answer confidently
        // wrong rather than merely incomplete.
        assert!(!hit.value.contains("data forever"), "{}", hit.value);
    }

    #[test]
    fn a_section_that_runs_on_is_cut_and_says_so() {
        // A heading with no heading after it would otherwise return the rest of
        // the document, undoing the whole saving.
        // Each paragraph distinct: adjacent identical text is collapsed (a
        // control and its text child say the same thing), so a fixture of 400
        // identical lines would become one and never reach the cap.
        let mut nodes = vec![heading("h1", "Terms")];
        for index in 0..400 {
            nodes.push(node(
                &format!("p{index}"),
                Role::Text,
                &format!("clause {index} of the boilerplate terms and conditions herein"),
            ));
        }
        let book = book_of(nodes);
        let hit = extract(&book, &want(&["Terms"]))[0].1.clone().unwrap();
        assert!(
            hit.value.len() <= SECTION_CAP + 40,
            "{} chars",
            hit.value.len()
        );
        assert!(
            hit.value.contains("section continues"),
            "a silently cut policy reads as a complete one"
        );
    }

    #[test]
    fn an_empty_section_falls_through_rather_than_answering_with_nothing() {
        // A heading immediately followed by another heading has no section. It
        // must not answer with an empty string, which would look like a found
        // value that happens to be blank.
        let book = book_of(vec![heading("h1", "Empty"), heading("h2", "Next")]);
        assert!(extract(&book, &want(&["Empty"]))[0].1.is_none());
    }

    #[test]
    fn a_labelled_value_is_still_a_value_even_next_to_headings() {
        // Sections must not hijack the ordinary case: only a *heading* names a
        // section, and a textbox called "Total" is still a field.
        let book = book_of(vec![
            heading("h1", "Order"),
            node("f", Role::TextBox, "Total").with_value("$42.00"),
        ]);
        let hit = extract(&book, &want(&["Total"]))[0].1.clone().unwrap();
        assert_eq!(hit.source, Source::Value);
        assert_eq!(hit.value, "$42.00");
    }
}

#[cfg(test)]
mod section_tests {
    use super::*;
    use crate::model::Snapshot;

    fn book_of(nodes: Vec<Node>) -> AnchorBook {
        let mut book = AnchorBook::new();
        book.observe(&Snapshot::new(nodes), true);
        book
    }

    #[test]
    fn a_section_does_not_repeat_the_heading_or_stutter_its_controls() {
        // Both found by extracting from a real page. An accessibility tree
        // names a control and its text child identically, and a heading's own
        // text child repeats the heading — so the section came back as
        // "Refund policy Refund policy Refunds are…" and
        // "…Request refund Request refund".
        let book = book_of(vec![
            Node::new("h", Role::Heading, "Refund policy"),
            Node::new("ht", Role::Text, "Refund policy"),
            Node::new("p", Role::Text, "Refunds within 30 days."),
            Node::new("b", Role::Button, "Request refund"),
            Node::new("bt", Role::Text, "Request refund"),
        ]);
        let hit = extract(&book, &["Refund policy".to_string()])[0]
            .1
            .clone()
            .unwrap();

        assert_eq!(hit.source, Source::Section);
        assert!(
            hit.value.contains("Refunds within 30 days."),
            "{}",
            hit.value
        );
        assert_eq!(
            hit.value.matches("Request refund").count(),
            1,
            "a control and its text child must not both appear: {}",
            hit.value
        );
        assert!(
            !hit.value.contains("Refund policy"),
            "the section must not repeat its own heading: {}",
            hit.value
        );
    }

    #[test]
    fn distinct_repeated_words_are_still_kept() {
        // The de-duplication is adjacent-only on purpose: a section that
        // legitimately says the same short thing twice, apart, still says it
        // twice.
        let book = book_of(vec![
            Node::new("h", Role::Heading, "Steps"),
            Node::new("a", Role::Text, "Save"),
            Node::new("b", Role::Text, "Then close"),
            Node::new("c", Role::Text, "Save"),
        ]);
        let hit = extract(&book, &["Steps".to_string()])[0].1.clone().unwrap();
        assert_eq!(hit.value.matches("Save").count(), 2, "{}", hit.value);
    }
}
