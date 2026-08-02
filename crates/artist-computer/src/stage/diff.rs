//! Recovering *where* a screen changed, when the client would not say.
//!
//! The stage's change signal is compositor damage, and it is the best signal
//! there is — a client that reports damage has told us exactly which rectangles
//! moved, for free, before anything is rendered. Android does not report it.
//! Measured against a live Waydroid container: 709 of 709 commits carried real
//! `wl_surface.damage` rectangles, and every single one covered the whole
//! surface. The rectangles are genuine, so nothing is malfunctioning; they
//! simply carry no location information, because Waydroid's `hwcomposer` hands
//! the compositor one full-surface rect per frame rather than SurfaceFlinger's
//! per-layer dirty region.
//!
//! Everything downstream that reasons about *where* then collapses. The noise
//! filter classifies by rectangle, so a blinking caret becomes indistinguishable
//! from a dialog opening and `settle: quiet` waits out its timeout on any
//! animating screen. Incremental OCR re-reads the whole frame every look, losing
//! the 15× that makes rung 3 affordable at all.
//!
//! So when the compositor cannot say where, this works it out: two captures,
//! compared tile by tile. Capture on this stage is a buffer read rather than a
//! screencast negotiation, which is the only reason this is cheap enough to be
//! worth doing — a full-frame comparison costs a couple of milliseconds against
//! an OCR pass costing hundreds.
//!
//! This is a *fallback*, not a replacement. A client that reports real damage is
//! always believed: it knows what it drew, and it knew before the pixels
//! existed.

use crate::model::{Frame, Rect};

/// The comparison granularity.
///
/// A caret is a few pixels wide, so tiles much larger than this would classify
/// one blinking cursor as a large changed region and put us back where we
/// started. Much smaller and the tile bookkeeping starts to cost more than the
/// comparison it is organizing.
pub const TILE: u32 = 32;

/// Which tiles differ between two frames, merged into rows of rectangles.
///
/// Returns a single full-frame rectangle when the frames are not comparable —
/// different sizes mean a resize, and after a resize nothing about the previous
/// frame's layout is informative.
pub fn changed_regions(previous: &Frame, current: &Frame) -> Vec<Rect> {
    if previous.width != current.width
        || previous.height != current.height
        || previous.rgba.len() != current.rgba.len()
    {
        return vec![Rect {
            x: 0,
            y: 0,
            width: current.width,
            height: current.height,
        }];
    }

    let mut regions = Vec::new();
    let stride = (current.width * 4) as usize;

    let mut top = 0;
    while top < current.height {
        let bottom = (top + TILE).min(current.height);

        // Horizontal runs of changed tiles are merged as they are found. Rows
        // are kept separate deliberately: merging vertically as well would grow
        // one rectangle to enclose two distant changes and re-read everything
        // between them, which is the very thing this exists to avoid.
        let mut run: Option<(u32, u32)> = None;
        let mut left = 0;
        while left < current.width {
            let right = (left + TILE).min(current.width);
            let changed = tile_differs(
                &previous.rgba,
                &current.rgba,
                stride,
                left,
                right,
                top,
                bottom,
            );
            match (&mut run, changed) {
                (Some(open), true) => open.1 = right,
                (Some(open), false) => {
                    regions.push(Rect {
                        x: open.0 as i32,
                        y: top as i32,
                        width: open.1 - open.0,
                        height: bottom - top,
                    });
                    run = None;
                }
                (None, true) => run = Some((left, right)),
                (None, false) => {}
            }
            left = right;
        }
        if let Some(open) = run {
            regions.push(Rect {
                x: open.0 as i32,
                y: top as i32,
                width: open.1 - open.0,
                height: bottom - top,
            });
        }
        top = bottom;
    }
    regions
}

fn tile_differs(
    previous: &[u8],
    current: &[u8],
    stride: usize,
    left: u32,
    right: u32,
    top: u32,
    bottom: u32,
) -> bool {
    let start = (left * 4) as usize;
    let end = (right * 4) as usize;
    for row in top..bottom {
        let offset = row as usize * stride;
        let (Some(before), Some(after)) = (
            previous.get(offset + start..offset + end),
            current.get(offset + start..offset + end),
        ) else {
            return true;
        };
        // A whole-row slice comparison rather than pixel-by-pixel: this is the
        // hot loop, and `[u8]` equality is a memcmp the compiler vectorizes.
        if before != after {
            return true;
        }
    }
    false
}

/// Whether reported damage is too coarse to be worth believing.
///
/// True when the rectangles amount to the whole surface — either because the
/// client said so or because the compositor had to assume it. That is the
/// condition under which differencing earns its cost.
pub fn is_coarse(damage: &[Rect], frame: &Frame) -> bool {
    if damage.is_empty() {
        return false;
    }
    let area = u64::from(frame.width) * u64::from(frame.height);
    if area == 0 {
        return false;
    }
    let largest = damage
        .iter()
        .map(|rect| u64::from(rect.width) * u64::from(rect.height))
        .max()
        .unwrap_or(0);
    // Nine tenths rather than all of it: a client that damages everything but a
    // one-pixel border is telling us nothing more than one that damages the lot.
    largest * 10 >= area * 9
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, fill: u8) -> Frame {
        Frame::new(width, height, vec![fill; (width * height * 4) as usize])
    }

    fn set_pixel(frame: &mut Frame, x: u32, y: u32, value: u8) {
        let offset = ((y * frame.width + x) * 4) as usize;
        frame.rgba[offset..offset + 4].copy_from_slice(&[value; 4]);
    }

    #[test]
    fn identical_frames_have_no_changed_regions() {
        let before = frame(128, 64, 0);
        let after = frame(128, 64, 0);
        assert!(changed_regions(&before, &after).is_empty());
    }

    #[test]
    fn one_changed_pixel_yields_one_tile() {
        let before = frame(128, 64, 0);
        let mut after = frame(128, 64, 0);
        set_pixel(&mut after, 40, 40, 255);

        let regions = changed_regions(&before, &after);
        assert_eq!(regions.len(), 1);
        let region = regions[0];
        assert_eq!((region.x, region.y), (32, 32));
        assert_eq!((region.width, region.height), (TILE, TILE));
    }

    #[test]
    fn adjacent_tiles_in_a_row_merge() {
        let before = frame(128, 64, 0);
        let mut after = frame(128, 64, 0);
        set_pixel(&mut after, 5, 5, 255);
        set_pixel(&mut after, 40, 5, 255);

        let regions = changed_regions(&before, &after);
        assert_eq!(regions.len(), 1, "{regions:?}");
        assert_eq!(regions[0].width, 64);
    }

    #[test]
    fn distant_changes_stay_separate() {
        // The point of the whole exercise: two small changes far apart must not
        // become one rectangle spanning the screen, or the re-read is a full
        // re-read wearing a disguise.
        let before = frame(256, 64, 0);
        let mut after = frame(256, 64, 0);
        set_pixel(&mut after, 5, 5, 255);
        set_pixel(&mut after, 250, 5, 255);

        let regions = changed_regions(&before, &after);
        assert_eq!(regions.len(), 2, "{regions:?}");
    }

    #[test]
    fn changes_on_different_rows_stay_separate() {
        let before = frame(64, 128, 0);
        let mut after = frame(64, 128, 0);
        set_pixel(&mut after, 5, 5, 255);
        set_pixel(&mut after, 5, 100, 255);

        let regions = changed_regions(&before, &after);
        assert_eq!(regions.len(), 2, "{regions:?}");
    }

    #[test]
    fn a_resize_is_reported_as_everything() {
        let before = frame(64, 64, 0);
        let after = frame(128, 64, 0);
        let regions = changed_regions(&before, &after);
        assert_eq!(regions.len(), 1);
        assert_eq!((regions[0].width, regions[0].height), (128, 64));
    }

    #[test]
    fn edge_tiles_are_clipped_to_the_frame() {
        // A frame whose size is not a multiple of the tile: the last column and
        // row are narrower, and a region running past the edge would send the
        // OCR reading outside the buffer.
        let before = frame(50, 50, 0);
        let mut after = frame(50, 50, 0);
        set_pixel(&mut after, 49, 49, 255);

        let regions = changed_regions(&before, &after);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].x + regions[0].width as i32, 50);
        assert_eq!(regions[0].y + regions[0].height as i32, 50);
    }

    #[test]
    fn full_surface_damage_is_coarse() {
        let frame = frame(100, 100, 0);
        let whole = vec![Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        }];
        assert!(is_coarse(&whole, &frame));
    }

    #[test]
    fn a_small_rectangle_is_not_coarse() {
        let frame = frame(100, 100, 0);
        let small = vec![Rect {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        }];
        assert!(!is_coarse(&small, &frame));
    }

    #[test]
    fn no_damage_is_not_coarse() {
        // Absent damage means "nothing changed", which is a *stronger* statement
        // than a coarse rectangle. Differencing on it would be pure waste.
        let frame = frame(100, 100, 0);
        assert!(!is_coarse(&[], &frame));
    }
}
