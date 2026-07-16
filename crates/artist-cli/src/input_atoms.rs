use crate::input_images::{ImagePaste, image_paste};
use std::ops::Range;

const MAX_PASTE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct InputAtom {
    range: Range<usize>,
    replacement: String,
    image: Option<ImagePaste>,
}

#[derive(Clone, Default)]
pub(crate) struct InputAtoms(Vec<InputAtom>);

pub(crate) struct ExpandedInput {
    pub text: String,
    pub images: Vec<ImagePaste>,
}

impl InputAtoms {
    pub fn insertion_point(&self, at: usize) -> usize {
        self.0
            .iter()
            .find(|atom| atom.range.start < at && at < atom.range.end)
            .map_or(at, |atom| atom.range.end)
    }

    pub fn insert_text(&mut self, at: usize, bytes: usize) {
        for atom in &mut self.0 {
            if atom.range.start >= at {
                atom.range.start += bytes;
                atom.range.end += bytes;
            }
        }
    }

    pub fn insert_paste(&mut self, text: &mut String, cursor: &mut usize, value: &str) {
        self.insert_paste_kind(text, cursor, value, true);
    }

    pub fn insert_text_paste(&mut self, text: &mut String, cursor: &mut usize, value: &str) {
        self.insert_paste_kind(text, cursor, value, false);
    }

    fn insert_paste_kind(
        &mut self,
        text: &mut String,
        cursor: &mut usize,
        value: &str,
        allow_image: bool,
    ) {
        if self.0.len() >= 32 {
            return;
        }
        let image = allow_image
            .then(|| image_paste(value))
            .flatten()
            .filter(|_| self.0.iter().filter(|atom| atom.image.is_some()).count() < 4);
        let value = if image.is_none() {
            bounded_text(value)
        } else {
            value.to_owned()
        };
        let display = image.as_ref().map_or_else(
            || format!("[pasted {} characters]", value.chars().count()),
            |image| {
                format!(
                    "[{}]",
                    image
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("image")
                )
            },
        );
        self.insert_text(*cursor, display.len());
        let start = *cursor;
        text.insert_str(start, &display);
        *cursor += display.len();
        self.0.push(InputAtom {
            range: start..*cursor,
            replacement: if image.is_some() { display } else { value },
            image,
        });
        self.0.sort_by_key(|atom| atom.range.start);
    }

    pub fn remove_text(&mut self, start: usize, end: usize) {
        let removed = end.saturating_sub(start);
        for atom in &mut self.0 {
            if atom.range.start >= end {
                atom.range.start -= removed;
                atom.range.end -= removed;
            }
        }
    }

    pub fn remove_for_backspace(&mut self, text: &mut String, cursor: &mut usize) -> bool {
        let Some(index) = self
            .0
            .iter()
            .position(|atom| atom.range.start < *cursor && *cursor <= atom.range.end)
        else {
            return false;
        };
        self.remove_atom(index, text, cursor);
        true
    }

    pub fn remove_for_delete(&mut self, text: &mut String, cursor: &mut usize) -> bool {
        let Some(index) = self
            .0
            .iter()
            .position(|atom| atom.range.start <= *cursor && *cursor < atom.range.end)
        else {
            return false;
        };
        self.remove_atom(index, text, cursor);
        true
    }

    pub fn move_left(&self, cursor: usize) -> Option<usize> {
        self.0
            .iter()
            .find(|atom| atom.range.start < cursor && cursor <= atom.range.end)
            .map(|atom| atom.range.start)
    }

    pub fn move_right(&self, cursor: usize) -> Option<usize> {
        self.0
            .iter()
            .find(|atom| atom.range.start <= cursor && cursor < atom.range.end)
            .map(|atom| atom.range.end)
    }

    pub fn expand(&self, display: &str) -> ExpandedInput {
        let mut text = String::new();
        let mut images = Vec::new();
        let mut offset = 0;
        for atom in &self.0 {
            text.push_str(&display[offset..atom.range.start]);
            text.push_str(&atom.replacement);
            if let Some(image) = &atom.image {
                images.push(image.clone());
            }
            offset = atom.range.end;
        }
        text.push_str(&display[offset..]);
        ExpandedInput { text, images }
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    fn remove_atom(&mut self, index: usize, text: &mut String, cursor: &mut usize) {
        let atom = self.0.remove(index);
        let removed = atom.range.end - atom.range.start;
        text.drain(atom.range.clone());
        *cursor = atom.range.start;
        for later in &mut self.0 {
            if later.range.start >= atom.range.end {
                later.range.start -= removed;
                later.range.end -= removed;
            }
        }
    }
}

fn bounded_text(value: &str) -> String {
    if value.len() <= MAX_PASTE_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_PASTE_BYTES;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
#[path = "input_atoms_tests.rs"]
mod tests;
