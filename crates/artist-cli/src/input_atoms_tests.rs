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
