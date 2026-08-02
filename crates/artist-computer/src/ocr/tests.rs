//! Tests against synthesized frames, so they need no display and no browser.
//!
//! The glyphs are drawn from a small bitmap font defined here rather than by
//! rasterizing a real typeface: the point is to prove the *pipeline* reads what
//! was drawn, and a font dependency would make the fixture depend on which
//! version of which face happened to be installed.

use super::*;
use crate::model::Frame;

/// A deliberately blocky 5×7 font. Only the characters the tests use.
///
/// Blocky is right here: PP-OCR is trained on real rendered text, and hairline
/// strokes at this size are exactly what a detector trained on screenshots
/// would reasonably miss. These are drawn thick, at a size a person could read.
const GLYPHS: &[(char, [u8; 7])] = &[
    (
        'S',
        [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
    ),
    (
        'E',
        [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
    ),
    (
        'N',
        [
            0b10001, 0b11001, 0b11001, 0b10101, 0b10011, 0b10011, 0b10001,
        ],
    ),
    (
        'D',
        [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
    ),
    (
        'C',
        [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
    ),
    (
        'A',
        [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
    ),
    (
        'L',
        [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
    ),
    (
        'O',
        [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
    ),
    (
        'K',
        [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
    ),
];

/// Draw black text on white at a given scale.
fn render(width: u32, height: u32, items: &[(&str, u32, u32, u32)]) -> Frame {
    let mut rgba = vec![0xFF; (width * height * 4) as usize];
    for (text, origin_x, origin_y, scale) in items {
        let mut cursor = *origin_x;
        for character in text.chars() {
            if character == ' ' {
                cursor += 6 * scale;
                continue;
            }
            let Some((_, rows)) = GLYPHS.iter().find(|(glyph, _)| *glyph == character) else {
                continue;
            };
            for (row, bits) in rows.iter().enumerate() {
                for column in 0..5u32 {
                    if bits & (1 << (4 - column)) == 0 {
                        continue;
                    }
                    for dy in 0..*scale {
                        for dx in 0..*scale {
                            let x = cursor + column * scale + dx;
                            let y = origin_y + row as u32 * scale + dy;
                            if x >= width || y >= height {
                                continue;
                            }
                            let offset = ((y * width + x) * 4) as usize;
                            rgba[offset] = 0;
                            rgba[offset + 1] = 0;
                            rgba[offset + 2] = 0;
                        }
                    }
                }
            }
            cursor += 6 * scale;
        }
    }
    Frame::new(width, height, rgba)
}

/// Skip rather than fail when the weights are absent — they are large binaries
/// that a fresh checkout will not have until they are fetched.
fn engine() -> Option<Ocr> {
    match Ocr::load_default() {
        Ok(ocr) => Some(ocr),
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

#[test]
fn a_region_outside_the_frame_reads_nothing_rather_than_panicking() {
    // No model needed: this is the clamp, and it is the path a caller hits when
    // a damage rectangle arrives for a window that has since been resized.
    let frame = render(64, 64, &[]);
    let outside = Rect {
        x: 500,
        y: 500,
        width: 100,
        height: 100,
    };
    assert_eq!(clamp_region(&frame, Some(outside)).width, 0);

    let overhanging = Rect {
        x: 32,
        y: 32,
        width: 999,
        height: 999,
    };
    let clamped = clamp_region(&frame, Some(overhanging));
    assert_eq!((clamped.width, clamped.height), (32, 32));
}

#[test]
fn no_region_means_the_whole_frame() {
    let frame = render(128, 64, &[]);
    let full = clamp_region(&frame, None);
    assert_eq!((full.x, full.y, full.width, full.height), (0, 0, 128, 64));
}

/// An engine whose models are never run. `locate` touches no weights, so these
/// tests have no business loading 20 MB to exercise a string comparison.
fn bare() -> Ocr {
    Ocr {
        inner: Arc::new(Engine {
            detection: unreachable_model(),
            recognition: unreachable_model(),
            dictionary: Vec::new(),
        }),
    }
}

#[test]
fn locate_refuses_to_choose_between_two_equal_matches() {
    // The whole point of the rung: two "Delete"s must be a miss, not a guess.
    let ocr = bare();
    let boxes = vec![
        TextBox {
            rect: Rect::default(),
            text: "Delete".into(),
            confidence: 1.0,
        },
        TextBox {
            rect: Rect {
                x: 0,
                y: 40,
                width: 10,
                height: 10,
            },
            text: "Delete".into(),
            confidence: 1.0,
        },
    ];
    assert!(ocr.locate(&boxes, "Delete").is_none());

    let single = vec![boxes[0].clone()];
    assert!(ocr.locate(&single, "Delete").is_some());
}

#[test]
fn locate_tolerates_the_same_drift_a_live_step_does() {
    let ocr = bare();
    let boxes = vec![TextBox {
        rect: Rect::default(),
        text: "Send message".into(),
        confidence: 1.0,
    }];
    // Containment, same as `check_label`.
    assert!(ocr.locate(&boxes, "Send").is_some());
    // And the same refusal: nothing shorter than three characters may stand in.
    assert!(ocr.locate(&boxes, "S").is_none());
    assert!(ocr.locate(&boxes, "Cancel").is_none());
}

#[test]
fn a_misread_glyph_still_matches_but_a_different_word_does_not() {
    // Measured, not hypothetical: our own fixture reads SEND back as SENO.
    // Containment cannot absorb that, which is why this rung is the one place
    // edit distance is allowed.
    let ocr = bare();
    let misread = vec![TextBox {
        rect: Rect::default(),
        text: "SENO".into(),
        confidence: 0.9,
    }];
    assert!(
        ocr.locate(&misread, "Send").is_some(),
        "a single substituted glyph must not lose the match"
    );

    // The confusion `check_label` exists to prevent stays prevented. These are
    // five edits over seven characters; the threshold is one in four.
    let confirm = vec![TextBox {
        rect: Rect::default(),
        text: "Confirm".into(),
        confidence: 0.9,
    }];
    assert!(ocr.locate(&confirm, "Cancel").is_none());
    assert!(ocr.locate(&confirm, "Delete").is_none());

    // Two equally-bad reads of different things is a miss, not a coin flip.
    let ambiguous = vec![
        TextBox {
            rect: Rect::default(),
            text: "SENO".into(),
            confidence: 0.9,
        },
        TextBox {
            rect: Rect {
                x: 0,
                y: 50,
                width: 8,
                height: 8,
            },
            text: "SEN0".into(),
            confidence: 0.9,
        },
    ];
    assert!(ocr.locate(&ambiguous, "Send").is_none());
}

#[test]
fn the_distance_metric_separates_noise_from_different_words() {
    // The numbers the threshold was chosen against, pinned so a later tweak
    // has to confront them.
    assert!(normalized_distance("seno", "send") <= MAX_DISTANCE);
    assert!(normalized_distance("delete", "delcte") <= MAX_DISTANCE);
    assert!(normalized_distance("cancel", "confirm") > MAX_DISTANCE);
    assert!(normalized_distance("save", "load") > MAX_DISTANCE);
    assert!(normalized_distance("delete", "cancel") > MAX_DISTANCE);
    assert_eq!(normalized_distance("same", "same"), 0.0);
}

/// A loadable graph with nothing in it.
fn unreachable_model() -> rten::Model {
    // An empty graph is a valid model; it simply has nothing to run.
    rten::Model::load(minimal_rten_model()).expect("an empty model is loadable")
}

/// The smallest valid `.rten` file: a header and an empty graph.
fn minimal_rten_model() -> Vec<u8> {
    // Built once and cached, because constructing it costs a flatbuffer write.
    static MODEL: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    MODEL
        .get_or_init(|| {
            // rten can load ONNX, and an ONNX graph with no nodes is far easier
            // to hand-write than a flatbuffer: it is a protobuf with a single
            // empty `graph` field and an ir_version.
            //
            // field 1 (ir_version, varint) = 7; field 7 (graph, message) = empty
            vec![0x08, 0x07, 0x3a, 0x00]
        })
        .clone()
}

#[test]
fn text_drawn_on_a_frame_is_found_and_read() {
    let Some(ocr) = engine() else { return };

    let frame = render(480, 160, &[("SEND", 40, 40, 8)]);
    let boxes = ocr
        .read(&frame, None, &DetectOptions::default())
        .expect("detection should run");

    assert!(
        !boxes.is_empty(),
        "nothing was detected on a frame with text on it"
    );

    // The box must actually cover the drawn glyphs. The text starts at x=40 and
    // each glyph is 6*8 wide, so "SEND" spans roughly x 40..232, y 40..96.
    let covers = boxes.iter().any(|found| {
        found.rect.x <= 60 && found.rect.y <= 60 && found.rect.x + found.rect.width as i32 >= 200
    });
    assert!(covers, "no box covered the drawn text: {boxes:?}");

    let read: String = boxes.iter().map(|found| found.text.as_str()).collect();
    eprintln!("read back: {read:?}");
}

#[test]
fn cropping_to_a_region_finds_only_what_is_in_it() {
    let Some(ocr) = engine() else { return };

    // Two well-separated words. Cropping to the second must not return the
    // first — this is the property item 1 depends on.
    let frame = render(640, 320, &[("SEND", 30, 30, 6), ("CANCEL", 30, 200, 6)]);
    let lower = Rect {
        x: 0,
        y: 170,
        width: 640,
        height: 150,
    };

    let boxes = ocr
        .read(&frame, Some(lower), &DetectOptions::default())
        .expect("detection should run");

    for found in &boxes {
        assert!(
            found.rect.y >= 150,
            "a box from outside the region leaked in: {found:?}"
        );
    }
    assert!(
        boxes
            .iter()
            .all(|found| found.rect.y + found.rect.height as i32 <= 320),
        "coordinates must be in frame space, not region space: {boxes:?}"
    );
}

/// What item 1 actually buys, measured rather than asserted in a comment.
///
/// Not a strict performance gate — a loaded machine would make that flaky — but
/// the ratio is printed, and the direction is checked. Detection cost scales
/// with area, so a small crop of a large screen must not be slower.
#[test]
fn reading_a_damaged_region_is_far_cheaper_than_the_whole_screen() {
    let Some(ocr) = engine() else { return };
    use std::time::Instant;

    // A realistic screen: text scattered over 1920x1080.
    let mut items: Vec<(&str, u32, u32, u32)> = Vec::new();
    for row in 0..8 {
        items.push(("SEND", 80, 60 + row * 120, 5));
        items.push(("CANCEL", 700, 60 + row * 120, 5));
    }
    let frame = render(1920, 1080, &items);
    let options = DetectOptions::default();

    // Warm: first call pays for lazy allocation inside the runtime.
    let _ = ocr.read(&frame, None, &options).expect("warmup");

    let start = Instant::now();
    let full = ocr.read(&frame, None, &options).expect("full read");
    let full_time = start.elapsed();

    // One damaged region, the size a caret or a button repaint would report.
    let damaged = Rect {
        x: 60,
        y: 40,
        width: 260,
        height: 80,
    };
    let start = Instant::now();
    let cropped = ocr
        .read(&frame, Some(damaged), &options)
        .expect("cropped read");
    let cropped_time = start.elapsed();

    let ratio = full_time.as_secs_f64() / cropped_time.as_secs_f64().max(1e-9);
    eprintln!(
        "full {:?} ({} boxes) vs damaged {:?} ({} boxes) — {ratio:.1}x",
        full_time,
        full.len(),
        cropped_time,
        cropped.len()
    );

    assert!(
        cropped_time < full_time,
        "cropping must not cost more than reading everything: {cropped_time:?} vs {full_time:?}"
    );
    assert!(
        cropped.len() < full.len(),
        "the crop should see less than the whole screen"
    );
}
