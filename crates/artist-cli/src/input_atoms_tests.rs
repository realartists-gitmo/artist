use super::*;

#[test]
fn pasted_text_expands_and_deletes_atomically() {
    let mut atoms = InputAtoms::default();
    let mut text = "say ".to_owned();
    let mut cursor = text.len();
    atoms.insert_paste(&mut text, &mut cursor, "hello\nworld");
    assert_eq!(text, "say [pasted 11 characters]");
    assert_eq!(atoms.expand(&text).text, "say hello\nworld");
    assert!(atoms.remove_for_backspace(&mut text, &mut cursor));
    assert_eq!(text, "say ");
}

#[test]
fn trusted_image_path_becomes_an_atomic_attachment() {
    let path = std::env::temp_dir().join(format!("artist-paste-{}-test.png", std::process::id()));
    image::save_buffer(&path, &[255, 0, 0, 255], 1, 1, image::ColorType::Rgba8).unwrap();
    let mut atoms = InputAtoms::default();
    let mut text = String::new();
    let mut cursor = 0;
    atoms.insert_paste(&mut text, &mut cursor, path.to_str().unwrap());
    assert!(text.starts_with("[artist-paste-"));
    let expanded = atoms.expand(&text);
    assert_eq!(expanded.images.len(), 1);
    cursor = 0;
    assert!(atoms.remove_for_delete(&mut text, &mut cursor));
    assert!(text.is_empty());
}

/// Every atom range must stay inside the text it indexes. A stale range is
/// handed straight back by `insertion_point`, which the caller uses as a byte
/// cursor — so the failure surfaces far away, as an out-of-bounds slice while
/// wrapping the input for display.
fn assert_ranges_within(atoms: &InputAtoms, text: &str) {
    for atom in &atoms.0 {
        assert!(
            atom.range.end <= text.len(),
            "atom {:?} escapes text of length {}",
            atom.range,
            text.len()
        );
        assert!(text.is_char_boundary(atom.range.start));
        assert!(text.is_char_boundary(atom.range.end));
    }
    assert!(atoms.insertion_point(text.len()) <= text.len());
}

fn with_paste(prefix: &str, suffix: &str) -> (InputAtoms, String, usize) {
    let mut atoms = InputAtoms::default();
    let mut text = prefix.to_owned();
    let mut cursor = text.len();
    atoms.insert_paste(&mut text, &mut cursor, "hello\nworld");
    text.push_str(suffix);
    (atoms, text, cursor)
}

/// The crash: replacing a span that overlaps an atom left the atom's range
/// pointing past the shortened string.
#[test]
fn a_replacement_spanning_an_atom_drops_it_and_leaves_ranges_valid() {
    let (mut atoms, mut text, _) = with_paste("say ", " now");
    let range = 2..text.len() - 1;

    text.replace_range(range.clone(), "x");
    atoms.remove_text(range.start, range.end);
    atoms.insert_text(range.start, 1);

    assert_ranges_within(&atoms, &text);
    assert_eq!(atoms.expand(&text).text, text, "no atom survives the cut");
}

#[test]
fn a_removal_after_an_atom_leaves_it_anchored() {
    let (mut atoms, mut text, _) = with_paste("say ", " now");
    let start = text.len() - 2;
    let end = text.len();
    text.replace_range(start..end, "");
    atoms.remove_text(start, end);

    assert_ranges_within(&atoms, &text);
    assert_eq!(atoms.expand(&text).text, "say hello\nworld n");
}

#[test]
fn a_removal_before_an_atom_shifts_it() {
    let (mut atoms, mut text, _) = with_paste("say ", "");
    text.replace_range(0..2, "");
    atoms.remove_text(0, 2);

    assert_ranges_within(&atoms, &text);
    assert_eq!(atoms.expand(&text).text, "y hello\nworld");
}

#[test]
fn a_removal_strictly_inside_an_atom_drops_it() {
    let (mut atoms, mut text, _) = with_paste("say ", "");
    text.replace_range(6..9, "");
    atoms.remove_text(6, 9);

    assert_ranges_within(&atoms, &text);
    assert_eq!(atoms.expand(&text).text, text);
}

#[test]
fn an_insertion_inside_an_atom_drops_it() {
    let (mut atoms, mut text, _) = with_paste("say ", "");
    text.insert_str(6, "zz");
    atoms.insert_text(6, 2);

    assert_ranges_within(&atoms, &text);
    assert_eq!(atoms.expand(&text).text, text);
}

#[test]
fn empty_edits_leave_every_atom_intact() {
    let (mut atoms, text, _) = with_paste("say ", "");
    atoms.remove_text(6, 6);
    atoms.insert_text(6, 0);

    assert_ranges_within(&atoms, &text);
    assert_eq!(atoms.expand(&text).text, "say hello\nworld");
}

/// Multi-byte content is where this first surfaced: the stale range overshot
/// by exactly the width of the characters that had been removed.
#[test]
fn multibyte_text_around_an_atom_stays_anchored() {
    let (mut atoms, mut text, _) = with_paste("⠀⠀", "⠀⠀");
    let start = text.len() - 6;
    let end = text.len();
    text.replace_range(start..end, "");
    atoms.remove_text(start, end);

    assert_ranges_within(&atoms, &text);
    assert!(atoms.expand(&text).text.starts_with("⠀⠀hello\nworld"));
}

/// The invariant, stated over arbitrary edit sequences rather than the handful
/// of cases someone thought to write down. Both atom bugs so far were found by
/// crashing in production; this is the shape that finds them first.
mod properties {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Op {
        Paste(String),
        Insert(String, u8),
        Remove(u8, u8),
    }

    /// Snap a percentage into a char boundary of the current text, so the
    /// generated positions are always ones a caller could really pass.
    fn boundary(text: &str, percent: u8) -> usize {
        let mut index = text.len() * usize::from(percent.min(100)) / 100;
        while index < text.len() && !text.is_char_boundary(index) {
            index += 1;
        }
        index.min(text.len())
    }

    fn operations() -> impl Strategy<Value = Vec<Op>> {
        // Multi-byte characters are in the alphabet on purpose: byte-versus-char
        // confusion is exactly what strands a range.
        let content = "[a b\n⠀é]{1,6}";
        prop::collection::vec(
            prop_oneof![
                content.prop_map(Op::Paste),
                (content, any::<u8>()).prop_map(|(text, at)| Op::Insert(text, at)),
                (any::<u8>(), any::<u8>()).prop_map(|(start, end)| Op::Remove(start, end)),
            ],
            0..12,
        )
    }

    proptest! {
        #[test]
        fn atom_ranges_never_escape_the_text_they_index(ops in operations()) {
            let mut atoms = InputAtoms::default();
            let mut text = String::new();

            for op in ops {
                match op {
                    Op::Paste(value) => {
                        let mut cursor = text.len();
                        atoms.insert_paste(&mut text, &mut cursor, &value);
                        prop_assert!(cursor <= text.len());
                    }
                    Op::Insert(value, at) => {
                        let at = boundary(&text, at);
                        atoms.insert_text(at, value.len());
                        text.insert_str(at, &value);
                    }
                    Op::Remove(start, end) => {
                        let start = boundary(&text, start);
                        let end = boundary(&text, end).max(start);
                        text.replace_range(start..end, "");
                        atoms.remove_text(start, end);
                    }
                }

                assert_ranges_within(&atoms, &text);
                // Expanding is where a stranded range actually panicked.
                let _ = atoms.expand(&text);
            }
        }
    }
}
