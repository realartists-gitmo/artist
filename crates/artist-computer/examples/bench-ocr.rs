//! Measure the rung-3 text localizer, so tuning decisions are made on numbers.
//!
//! ```text
//! cargo run -p artist-computer --example bench-ocr --release --features ocr
//! RTEN_TIMING="sort=time" cargo run -p artist-computer --example bench-ocr --release --features ocr
//! ```
//!
//! Two things it answers:
//!
//! 1. **What does cropping to a damage region buy?** This is the whole argument
//!    for owning the compositor, and it should be a large number.
//! 2. **Where does the time actually go?** With `RTEN_TIMING` set, rten reports
//!    per-operator totals — which is what decides whether a GEMM blocking change
//!    or a Winograd path is worth building, rather than a guess about which
//!    convolutions a model contains.

use std::time::Instant;

use artist_computer::model::{Frame, Rect};
use artist_computer::ocr::{DetectOptions, Ocr};

/// A blocky 5×7 font, enough to put readable text on a synthetic screen.
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

fn render(width: u32, height: u32, items: &[(&str, u32, u32, u32)]) -> Frame {
    let mut rgba = vec![0xFF; (width * height * 4) as usize];
    for (text, origin_x, origin_y, scale) in items {
        let mut cursor = *origin_x;
        for character in text.chars() {
            if let Some((_, rows)) = GLYPHS.iter().find(|(glyph, _)| *glyph == character) {
                for (row, bits) in rows.iter().enumerate() {
                    for column in 0..5u32 {
                        if bits & (1 << (4 - column)) == 0 {
                            continue;
                        }
                        for dy in 0..*scale {
                            for dx in 0..*scale {
                                let x = cursor + column * scale + dx;
                                let y = origin_y + row as u32 * scale + dy;
                                if x < width && y < height {
                                    let offset = ((y * width + x) * 4) as usize;
                                    rgba[offset] = 0;
                                    rgba[offset + 1] = 0;
                                    rgba[offset + 2] = 0;
                                }
                            }
                        }
                    }
                }
            }
            cursor += 6 * scale;
        }
    }
    Frame::new(width, height, rgba)
}

fn median(mut times: Vec<f64>) -> f64 {
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times[times.len() / 2]
}

fn main() {
    let ocr = match Ocr::load_default() {
        Ok(ocr) => ocr,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };

    let mut items: Vec<(&str, u32, u32, u32)> = Vec::new();
    for row in 0..8 {
        items.push(("SEND", 80, 60 + row * 120, 5));
        items.push(("CANCEL", 700, 60 + row * 120, 5));
        items.push(("OK", 1400, 60 + row * 120, 5));
    }
    let frame = render(1920, 1080, &items);
    let options = DetectOptions::default();

    // A damage region the size a button repaint or a caret blink reports.
    let damaged = Rect {
        x: 60,
        y: 40,
        width: 260,
        height: 80,
    };

    // The first call pays for lazy allocation inside the runtime and would
    // otherwise land entirely in whichever measurement ran first.
    let _ = ocr.read(&frame, None, &options).expect("warmup");

    let rounds = std::env::var("BENCH_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(5usize);

    let mut full_times = Vec::new();
    let mut full_boxes = 0;
    for _ in 0..rounds {
        let start = Instant::now();
        let boxes = ocr.read(&frame, None, &options).expect("full read");
        full_times.push(start.elapsed().as_secs_f64());
        full_boxes = boxes.len();
    }

    let mut crop_times = Vec::new();
    let mut crop_boxes = 0;
    for _ in 0..rounds {
        let start = Instant::now();
        let boxes = ocr
            .read(&frame, Some(damaged), &options)
            .expect("cropped read");
        crop_times.push(start.elapsed().as_secs_f64());
        crop_boxes = boxes.len();
    }

    let full = median(full_times);
    let crop = median(crop_times);

    println!("frame          1920x1080, {full_boxes} text boxes");
    println!(
        "damage region  {}x{}, {crop_boxes} text boxes",
        damaged.width, damaged.height
    );
    println!(
        "full read      {:>8.1} ms  (median of {rounds})",
        full * 1e3
    );
    println!(
        "damaged read   {:>8.1} ms  (median of {rounds})",
        crop * 1e3
    );
    println!("speedup        {:>8.1}x", full / crop);
    println!(
        "area ratio     {:>8.1}x",
        (1920.0 * 1080.0) / (damaged.width as f64 * damaged.height as f64)
    );
}
