//! Reading only what changed.
//!
//! This is the single largest win available to this rung, and it is available to
//! us and to almost nobody else. Every other pixel-driven harness re-reads the
//! whole screen every time, because it has no idea what moved. We own the
//! compositor, so we are *told* which rectangles changed — and detection cost
//! scales with area, so reading a 200×80 region instead of a 1920×1080 screen is
//! roughly a hundred and fifty times less work.
//!
//! No kernel-level tuning competes with not doing the arithmetic. A GEMM twice
//! as fast saves half of a cost this removes almost all of.
//!
//! The shape mirrors the anchor book exactly, which is not a coincidence — it is
//! the same problem. First contact reads everything; after that each look is a
//! delta, and the previous answer is carried forward for the parts nobody
//! touched.

use super::{DetectOptions, Ocr, TextBox};
use crate::model::{Frame, Rect};

/// How much a damage region grows before it is read.
///
/// Damage is reported for the pixels that changed, but a glyph that changed by
/// one pixel needs its *whole* box to be re-read or the crop cuts it in half and
/// recognition reads a fragment. A generous margin costs a little area and
/// removes a whole class of clipped-text bug.
const MARGIN: i32 = 24;

/// Beyond this share of the frame, coalescing stops paying.
///
/// Many scattered small rectangles — a caret, a clock, a progress bar — are
/// cheaper to read as one full frame than as thirty overlapping crops, each
/// with its own resize and its own model invocation.
const FULL_FRAME_SHARE: f32 = 0.45;

/// Text found on a surface, kept across looks.
#[derive(Default)]
pub struct Incremental {
    known: Vec<TextBox>,
    seen: bool,
}

impl Incremental {
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything currently believed to be on screen.
    pub fn boxes(&self) -> &[TextBox] {
        &self.known
    }

    /// Forget everything, so the next look reads the whole frame.
    ///
    /// For a navigation or a window resize, where carrying anything forward
    /// would be carrying forward a description of a screen that no longer
    /// exists.
    pub fn reset(&mut self) {
        self.known.clear();
        self.seen = false;
    }

    /// Update from a frame, re-reading only the damaged parts.
    ///
    /// `damage` is what the compositor reported since the last look. Empty
    /// damage on a surface already seen means nothing changed, and the cheapest
    /// correct thing to do is nothing at all.
    pub fn update(
        &mut self,
        ocr: &Ocr,
        frame: &Frame,
        damage: &[Rect],
        options: &DetectOptions,
    ) -> Result<&[TextBox], String> {
        if !self.seen {
            self.known = ocr.read(frame, None, options)?;
            self.seen = true;
            return Ok(&self.known);
        }
        if damage.is_empty() {
            return Ok(&self.known);
        }

        let regions = coalesce(damage, frame, MARGIN);
        // One region covering most of the frame is a full re-read wearing a
        // disguise; do it honestly and keep the code path that follows simple.
        let covered: u64 = regions
            .iter()
            .map(|r| u64::from(r.width) * u64::from(r.height))
            .sum();
        let whole = u64::from(frame.width) * u64::from(frame.height);
        if whole > 0 && covered as f32 / whole as f32 >= FULL_FRAME_SHARE {
            self.known = ocr.read(frame, None, options)?;
            return Ok(&self.known);
        }

        // Drop what used to be in the damaged area — it may have moved, changed
        // or gone — then read that area afresh. Anything outside is untouched by
        // construction and is carried forward.
        self.known
            .retain(|found| !regions.iter().any(|region| overlaps(*region, found.rect)));

        for region in &regions {
            self.known.extend(ocr.read(frame, Some(*region), options)?);
        }

        // Two damaged regions can both cover one run of text, so the same box
        // arrives twice.
        dedupe(&mut self.known);
        self.known.sort_by_key(|found| (found.rect.y, found.rect.x));
        Ok(&self.known)
    }
}

/// Merge overlapping damage into a few larger rectangles.
///
/// Thirty small rectangles is thirty resizes and thirty model invocations, and
/// the per-call overhead swamps the pixels saved. Merging anything that touches
/// after the margin is applied trades a little redundant area for far fewer
/// calls.
fn coalesce(damage: &[Rect], frame: &Frame, margin: i32) -> Vec<Rect> {
    let mut regions: Vec<Rect> = damage
        .iter()
        .map(|rect| grow(*rect, margin, frame))
        .filter(|rect| rect.width > 0 && rect.height > 0)
        .collect();

    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..regions.len() {
            for j in (i + 1)..regions.len() {
                if overlaps(regions[i], regions[j]) {
                    regions[i] = union(regions[i], regions[j]);
                    regions.remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    regions
}

fn grow(rect: Rect, margin: i32, frame: &Frame) -> Rect {
    let left = (rect.x - margin).max(0);
    let top = (rect.y - margin).max(0);
    let right = (rect.x + rect.width as i32 + margin).min(frame.width as i32);
    let bottom = (rect.y + rect.height as i32 + margin).min(frame.height as i32);
    Rect {
        x: left,
        y: top,
        width: (right - left).max(0) as u32,
        height: (bottom - top).max(0) as u32,
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    let (a_right, a_bottom) = (a.x + a.width as i32, a.y + a.height as i32);
    let (b_right, b_bottom) = (b.x + b.width as i32, b.y + b.height as i32);
    a.x < b_right && b.x < a_right && a.y < b_bottom && b.y < a_bottom
}

fn union(a: Rect, b: Rect) -> Rect {
    let left = a.x.min(b.x);
    let top = a.y.min(b.y);
    let right = (a.x + a.width as i32).max(b.x + b.width as i32);
    let bottom = (a.y + a.height as i32).max(b.y + b.height as i32);
    Rect {
        x: left,
        y: top,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    }
}

/// Drop boxes that name the same run of text twice.
///
/// Keyed on position rather than text: two different readings of the same place
/// are still one thing, and keeping both would offer the model a choice between
/// two names for one button.
fn dedupe(boxes: &mut Vec<TextBox>) {
    let mut kept: Vec<TextBox> = Vec::with_capacity(boxes.len());
    for found in boxes.drain(..) {
        if let Some(existing) = kept
            .iter_mut()
            .find(|held| mostly_same(held.rect, found.rect))
        {
            // Prefer the more confident reading of the two.
            if found.confidence > existing.confidence {
                *existing = found;
            }
            continue;
        }
        kept.push(found);
    }
    *boxes = kept;
}

/// Whether two rectangles describe the same thing, allowing for the couple of
/// pixels that resizing and unclipping move a box by.
fn mostly_same(a: Rect, b: Rect) -> bool {
    let slack = 6;
    (a.x - b.x).abs() <= slack
        && (a.y - b.y).abs() <= slack
        && (a.width as i32 - b.width as i32).abs() <= slack
        && (a.height as i32 - b.height as i32).abs() <= slack
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32) -> Frame {
        Frame::new(width, height, vec![0xFF; (width * height * 4) as usize])
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn touching_damage_becomes_one_region() {
        // Two rectangles 10px apart merge once the margin is applied, because
        // reading them separately costs two model invocations to save almost no
        // area.
        let regions = coalesce(
            &[rect(100, 100, 40, 20), rect(150, 100, 40, 20)],
            &frame(800, 600),
            MARGIN,
        );
        assert_eq!(regions.len(), 1, "{regions:?}");
        assert!(regions[0].width >= 130);
    }

    #[test]
    fn distant_damage_stays_separate() {
        let regions = coalesce(
            &[rect(10, 10, 20, 20), rect(700, 500, 20, 20)],
            &frame(800, 600),
            MARGIN,
        );
        assert_eq!(regions.len(), 2, "{regions:?}");
    }

    #[test]
    fn a_region_is_grown_but_never_past_the_frame() {
        let regions = coalesce(&[rect(0, 0, 10, 10)], &frame(800, 600), MARGIN);
        assert_eq!(regions[0].x, 0, "cannot grow past the left edge");
        assert_eq!(regions[0].y, 0);
        // Grown on the sides that have room.
        assert!(regions[0].width > 10 && regions[0].height > 10);

        let corner = coalesce(&[rect(790, 590, 10, 10)], &frame(800, 600), MARGIN);
        assert!(corner[0].x + corner[0].width as i32 <= 800);
        assert!(corner[0].y + corner[0].height as i32 <= 600);
    }

    #[test]
    fn the_margin_is_wide_enough_to_contain_a_clipped_glyph() {
        // Damage arrives for the pixels that changed. A caret blinking inside a
        // word reports a sliver; re-reading only that sliver would crop the word
        // in half and recognise a fragment.
        let sliver = rect(400, 300, 2, 18);
        let grown = coalesce(&[sliver], &frame(1920, 1080), MARGIN)[0];
        assert!(
            grown.width >= 40,
            "a one-glyph read needs its neighbours: {grown:?}"
        );
    }

    #[test]
    fn scattered_damage_over_most_of_the_screen_reads_the_whole_frame() {
        // The guard that stops thirty overlapping crops being slower than one
        // honest full read.
        let screen = frame(1000, 1000);
        let scattered: Vec<Rect> = (0..8)
            .flat_map(|row| (0..8).map(move |col| rect(col * 125, row * 125, 100, 100)))
            .collect();
        let regions = coalesce(&scattered, &screen, MARGIN);
        let covered: u64 = regions
            .iter()
            .map(|r| u64::from(r.width) * u64::from(r.height))
            .sum();
        assert!(
            covered as f32 / 1_000_000.0 >= FULL_FRAME_SHARE,
            "this should trip the full-frame path, covered {covered}"
        );
    }

    #[test]
    fn boxes_outside_the_damage_are_carried_forward() {
        // The property the whole optimization rests on: what nobody touched is
        // still there, and is not re-read.
        let mut state = Incremental::new();
        state.seen = true;
        state.known = vec![
            TextBox {
                rect: rect(10, 10, 80, 20),
                text: "Untouched".into(),
                confidence: 0.9,
            },
            TextBox {
                rect: rect(10, 400, 80, 20),
                text: "Stale".into(),
                confidence: 0.9,
            },
        ];

        let damaged = coalesce(&[rect(10, 400, 80, 20)], &frame(800, 600), MARGIN);
        state
            .known
            .retain(|found| !damaged.iter().any(|region| overlaps(*region, found.rect)));

        assert_eq!(state.known.len(), 1);
        assert_eq!(state.known[0].text, "Untouched");
    }

    #[test]
    fn the_same_text_found_twice_is_kept_once() {
        let mut boxes = vec![
            TextBox {
                rect: rect(100, 100, 60, 20),
                text: "Save".into(),
                confidence: 0.7,
            },
            TextBox {
                rect: rect(102, 101, 61, 20),
                text: "Save".into(),
                confidence: 0.95,
            },
            TextBox {
                rect: rect(400, 100, 60, 20),
                text: "Open".into(),
                confidence: 0.9,
            },
        ];
        dedupe(&mut boxes);
        assert_eq!(boxes.len(), 2, "{boxes:?}");
        // And the better reading survives.
        let save = boxes.iter().find(|b| b.text == "Save").unwrap();
        assert!((save.confidence - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn a_reset_forces_the_next_look_to_read_everything() {
        let mut state = Incremental::new();
        state.seen = true;
        state.known = vec![TextBox {
            rect: rect(0, 0, 10, 10),
            text: "old".into(),
            confidence: 1.0,
        }];
        state.reset();
        assert!(state.boxes().is_empty());
        assert!(
            !state.seen,
            "a navigation must not carry the old screen forward"
        );
    }
}
