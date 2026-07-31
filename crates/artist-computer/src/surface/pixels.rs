//! Rung 3: pixels, and the set-of-mark overlay.
//!
//! **Observation only, deliberately.** Minting anchors from a quantized pixel
//! grid is coordinates wearing a hat: it reintroduces every failure the anchor
//! design exists to prevent, without the identity that makes an anchor
//! meaningful. A surface that reaches this rung with no tree at all reports "no
//! actionable surface" — a loud, honest failure the model can act on, rather
//! than a picture it is invited to guess at.
//!
//! What this rung *is* good for is showing the model a frame annotated with the
//! **same anchors** as the structured view. Most implementations keep two
//! vocabularies — a pixel one and a DOM one — that cannot refer to each other,
//! so a model that spots something in the screenshot has no way to name it.
//! Here the box labels are the anchors, so the two channels are one.

use crate::anchors::Observation;
use crate::model::{Frame, Rect};

/// Colour of an overlay box and its label plate, as RGBA.
const MARK: [u8; 4] = [0xFF, 0x30, 0x60, 0xFF];
const LABEL_BACKGROUND: [u8; 4] = [0x10, 0x10, 0x10, 0xFF];
const BOX_THICKNESS: u32 = 2;

/// Draw anchor boxes onto a captured frame.
///
/// Only nodes whose bounds are known are drawn — a node the backend could not
/// locate is simply absent rather than boxed at the origin, which would point
/// the model at the wrong part of the screen.
pub fn annotate(frame: &Frame, observation: &Observation) -> Frame {
    let mut out = frame.clone();
    for entry in &observation.entries {
        let Some(bounds) = entry.node.bounds else {
            continue;
        };
        draw_box(&mut out, bounds, MARK);
        draw_label(&mut out, bounds, &entry.anchor);
    }
    out
}

fn put(frame: &mut Frame, x: i64, y: i64, colour: [u8; 4]) {
    if x < 0 || y < 0 || x >= i64::from(frame.width) || y >= i64::from(frame.height) {
        return;
    }
    let offset = ((y as u32 * frame.width + x as u32) * 4) as usize;
    if let Some(pixel) = frame.rgba.get_mut(offset..offset + 4) {
        pixel.copy_from_slice(&colour);
    }
}

fn draw_box(frame: &mut Frame, bounds: Rect, colour: [u8; 4]) {
    let (x0, y0) = (i64::from(bounds.x), i64::from(bounds.y));
    let (x1, y1) = (
        x0 + i64::from(bounds.width),
        y0 + i64::from(bounds.height),
    );
    for thickness in 0..i64::from(BOX_THICKNESS) {
        for x in x0..x1 {
            put(frame, x, y0 + thickness, colour);
            put(frame, x, y1 - 1 - thickness, colour);
        }
        for y in y0..y1 {
            put(frame, x0 + thickness, y, colour);
            put(frame, x1 - 1 - thickness, y, colour);
        }
    }
}

/// A small solid plate above each box.
///
/// Deliberately not rendered glyphs: bundling a font renderer to draw a
/// three-letter token would be a large dependency for a debugging affordance.
/// The plate marks *where* the anchor is; the observation text says what it is
/// called, and the two are read together.
fn draw_label(frame: &mut Frame, bounds: Rect, anchor: &str) {
    let width = (anchor.len() as u32 * 6).clamp(12, 120);
    let height = 10u32;
    let top = i64::from(bounds.y) - i64::from(height) - 1;
    for y in 0..i64::from(height) {
        for x in 0..i64::from(width) {
            put(
                frame,
                i64::from(bounds.x) + x,
                top + y,
                if y == 0 || y == i64::from(height) - 1 {
                    MARK
                } else {
                    LABEL_BACKGROUND
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchors::AnchorBook;
    use crate::model::{Node, Role, Snapshot};

    fn blank(width: u32, height: u32) -> Frame {
        Frame::new(width, height, vec![0u8; (width * height * 4) as usize])
    }

    fn observed(nodes: Vec<Node>) -> Observation {
        let mut book = AnchorBook::new();
        book.observe(&Snapshot::new(nodes), false)
    }

    #[test]
    fn a_box_is_drawn_around_a_node_with_bounds() {
        let frame = blank(100, 100);
        let observation = observed(vec![Node::new("a", Role::Button, "Save").with_bounds(Rect {
            x: 20,
            y: 30,
            width: 40,
            height: 20,
        })]);

        let marked = annotate(&frame, &observation);
        // Top-left corner of the box.
        assert_eq!(marked.pixel(20, 30), Some((0xFF, 0x30, 0x60, 0xFF)));
        // Bottom-right, inside the box.
        assert_eq!(marked.pixel(59, 49), Some((0xFF, 0x30, 0x60, 0xFF)));
        // Well outside it, untouched.
        assert_eq!(marked.pixel(90, 90), Some((0, 0, 0, 0)));
    }

    #[test]
    fn a_node_without_bounds_is_not_drawn_at_the_origin() {
        // Boxing an unlocatable node at (0,0) would point the model at the
        // wrong part of the screen with full confidence.
        let frame = blank(50, 50);
        let observation = observed(vec![Node::new("a", Role::Button, "Save")]);
        let marked = annotate(&frame, &observation);
        assert_eq!(marked.rgba, frame.rgba);
    }

    #[test]
    fn a_box_at_the_edge_does_not_panic_or_wrap() {
        let frame = blank(40, 40);
        let observation = observed(vec![
            Node::new("a", Role::Button, "edge").with_bounds(Rect {
                x: 30,
                y: 30,
                width: 40,
                height: 40,
            }),
            // Its label plate would be drawn above the top of the frame.
            Node::new("b", Role::Button, "top").with_bounds(Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            }),
        ]);
        let marked = annotate(&frame, &observation);
        assert_eq!(marked.rgba.len(), frame.rgba.len());
    }

    #[test]
    fn the_annotated_frame_still_encodes_to_png() {
        let frame = blank(32, 32);
        let observation = observed(vec![Node::new("a", Role::Button, "x").with_bounds(Rect {
            x: 4,
            y: 8,
            width: 16,
            height: 8,
        })]);
        let png = annotate(&frame, &observation).to_png().unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    #[test]
    fn marks_use_the_same_anchors_as_the_structured_view() {
        // The property that makes the two channels one vocabulary: whatever the
        // observation text calls an element is what the overlay marks.
        let mut book = AnchorBook::new();
        let observation = book.observe(
            &Snapshot::new(vec![Node::new("a", Role::Button, "Save").with_bounds(Rect {
                x: 1,
                y: 20,
                width: 10,
                height: 10,
            })]),
            false,
        );
        let text = crate::render::observation("win:1", &observation, Some("ab12cd"));
        let anchor = &observation.entries[0].anchor;

        assert!(text.contains(anchor));
        // And the overlay drew something for that same entry.
        let marked = annotate(&blank(40, 40), &observation);
        assert_ne!(marked.rgba, blank(40, 40).rgba);
    }
}
