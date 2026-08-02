//! Reading text off pixels, so rung 3 can be acted on rather than only looked at.
//!
//! Rung 3 used to be observation-only, and the reasoning was sound as far as it
//! went: minting anchors from a pixel grid is coordinates wearing a hat. But the
//! conclusion was wrong. It is not the *pixels* that are unsafe to act on, it is
//! letting the **model** name a position. If the harness finds the text, mints an
//! anchor for it, and the model says "click the anchor called Send" exactly as it
//! does at every other rung, then nothing about the contract has changed — the
//! model still names things, and a name that no longer resolves is still an
//! error rather than a click on whatever moved into that spot.
//!
//! So this module answers one question, and deliberately not the general one:
//! **given a name the model already used, where is it on screen?** That is much
//! easier than transcribing a screen: we are matching against a known candidate,
//! so detection quality is what carries the result and recognition only has to
//! get close.
//!
//! "Close" needs its own tolerance, though, and finding that out cost a test.
//! The first version reused the ordinary label check, on the assumption that
//! containment would absorb a misread glyph. It does not — our own fixture reads
//! `SEND` back as `SENO`, which neither equals nor contains the label. See
//! [`Ocr::locate`] for the bounded edit distance that lives here and nowhere
//! else in the crate, and why it is safe here specifically.
//!
//! Two models, both PP-OCRv5 mobile, both ONNX, both run on [`rten`] — the same
//! pure-Rust runtime the memory subsystem already ships. No C++ toolchain, no
//! conversion step, no second round with `CXXFLAGS`.
//!
//! * **Detection** (DBNet) — a `[1,3,H,W]` image in, a `[1,1,H,W]` probability
//!   map out. Where the map is hot, there is text.
//! * **Recognition** (SVTR) — one cropped line in at a fixed height of 48, CTC
//!   logits out, greedy-decoded against a character dictionary.

use std::path::Path;
use std::sync::Arc;

use crate::model::{Frame, Rect};

mod detect;
mod incremental;
mod recognize;

#[cfg(test)]
mod tests;

pub use detect::DetectOptions;
pub use incremental::Incremental;

/// One run of text found on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBox {
    /// Where it is, in frame coordinates.
    pub rect: Rect,
    /// What it says. Empty when detection found a region that recognition could
    /// not read — still useful, because a box with no text is a candidate the
    /// set-of-mark overlay can offer the model.
    pub text: String,
    /// Mean detector confidence over the region, 0..1.
    pub confidence: f32,
}

/// The two models, loaded once and kept resident.
///
/// Cloning is cheap — the models are behind an `Arc`, so every surface can hold
/// a handle without a second copy of 20 MB of weights.
#[derive(Clone)]
pub struct Ocr {
    inner: Arc<Engine>,
}

struct Engine {
    detection: rten::Model,
    recognition: rten::Model,
    /// Index → character. CTC index 0 is the blank, so character `i` of this
    /// list is class `i + 1`.
    dictionary: Vec<String>,
}

/// Where the shipped models live, relative to the crate.
///
/// Overridable because a packaged build puts them somewhere else, and because a
/// test wants to point at a fixture without reaching outside the workspace.
pub const MODEL_DIR_VAR: &str = "ARTIST_OCR_MODELS";

impl Ocr {
    /// Load from the directory holding the three shipped files.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let detection = load_model(&dir.join("ppocrv5-mobile-det.onnx"))?;
        let recognition = load_model(&dir.join("ppocrv5-mobile-rec.onnx"))?;
        let dictionary = load_dictionary(&dir.join("ppocrv5_dict.txt"))?;
        Ok(Self {
            inner: Arc::new(Engine {
                detection,
                recognition,
                dictionary,
            }),
        })
    }

    /// Load from `$ARTIST_OCR_MODELS`, or the crate's own `models/` directory.
    pub fn load_default() -> Result<Self, String> {
        let dir = std::env::var_os(MODEL_DIR_VAR)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("models"));
        Self::load(&dir)
    }

    /// Find text in a frame, optionally restricted to one region.
    ///
    /// **`region` is the single highest-value optimization in this module**, and
    /// it is available to us and to almost nobody else: we own the compositor,
    /// so we know exactly which rectangles changed since the last look. Running
    /// detection over a 200×80 damaged region instead of a 1920×1080 screen is
    /// two orders of magnitude less work, and no amount of kernel tuning
    /// competes with not doing the arithmetic at all.
    ///
    /// Returned rectangles are always in **frame** coordinates, not region
    /// coordinates, so a caller that cropped does not have to remember it did.
    pub fn read(
        &self,
        frame: &Frame,
        region: Option<Rect>,
        options: &DetectOptions,
    ) -> Result<Vec<TextBox>, String> {
        let region = clamp_region(frame, region);
        if region.width == 0 || region.height == 0 {
            return Ok(Vec::new());
        }

        let mut boxes = detect::detect(&self.inner.detection, frame, region, options)?;
        for found in &mut boxes {
            found.text = recognize::recognize(
                &self.inner.recognition,
                &self.inner.dictionary,
                frame,
                found.rect,
            )
            .unwrap_or_default();
        }
        Ok(boxes)
    }

    /// Find the box whose text matches a label the model named.
    ///
    /// **This is the one place in the crate that tolerates edit distance**, and
    /// the exception is deliberate rather than a weakening of the rule.
    /// Everywhere else, a label is compared against a name the toolkit *told*
    /// us, and the only drift is cosmetic — `&Save` against `Save…`. Here it is
    /// compared against a name we *guessed* from pixels, and recognition
    /// substitutes visually similar glyphs: measured on our own fixture,
    /// `SEND` comes back as `SENO`. Containment cannot absorb that, and refusing
    /// it would make the rung unusable for the reason it exists.
    ///
    /// The bound is tight enough that the confusion `check_label` was written to
    /// prevent stays prevented. `cancel` against `confirm` is five edits over
    /// seven characters — 0.71, nowhere near the threshold — while `seno`
    /// against `send` is one over four. The gap between "the camera misread a
    /// letter" and "these are different words" is wide, and this sits in it.
    ///
    /// Ambiguity is still a miss. Two boxes matching equally well returns
    /// nothing, because guessing between two `Delete`s is the exact failure this
    /// design exists to prevent — and it is *more* likely here, not less, since
    /// noisy reads collide more readily than exact names do.
    pub fn locate<'a>(&self, boxes: &'a [TextBox], label: &str) -> Option<&'a TextBox> {
        let wanted = crate::program::normalize(label);
        if wanted.chars().count() < MIN_MATCH {
            return None;
        }

        let mut best: Option<(f32, &TextBox)> = None;
        let mut tied = false;

        for found in boxes {
            let text = crate::program::normalize(&found.text);
            if text.chars().count() < MIN_MATCH {
                continue;
            }
            // Containment counts as a perfect match, same as equality: the
            // reading may be a whole line while the model named one button on
            // it, which is not an error of any kind.
            let score = if text == wanted || text.contains(&wanted) || wanted.contains(&text) {
                0.0
            } else {
                normalized_distance(&text, &wanted)
            };
            if score > MAX_DISTANCE {
                continue;
            }
            match best {
                Some((best_score, _)) if score < best_score => {
                    best = Some((score, found));
                    tied = false;
                }
                Some((best_score, _)) if (score - best_score).abs() < f32::EPSILON => tied = true,
                Some(_) => {}
                None => best = Some((score, found)),
            }
        }

        if tied {
            return None;
        }
        best.map(|(_, found)| found)
    }
}

/// Shortest string that may take part in a match, either side.
///
/// Same threshold and same reasoning as [`crate::program::check_label`]: below
/// three characters, a match carries no information. It matters more here — a
/// two-character misread lands within edit distance of almost anything.
const MIN_MATCH: usize = 3;

/// Largest share of a label that may be wrong and still count as the same word.
///
/// One character in four. Chosen so that single-glyph substitution — what
/// recognition actually gets wrong — survives, while genuinely different words
/// do not come close.
const MAX_DISTANCE: f32 = 0.25;

/// Levenshtein distance over characters, divided by the longer length.
///
/// Two rows rather than a full matrix: these are button labels, so the strings
/// are short, but a long line of recognised text against a short label would
/// otherwise allocate a rectangle for nothing.
fn normalized_distance(left: &str, right: &str) -> f32 {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let longest = left.len().max(right.len());
    if longest == 0 {
        return 0.0;
    }

    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];

    for (i, a) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let substitution = previous[j] + usize::from(a != b);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()] as f32 / longest as f32
}

fn load_model(path: &Path) -> Result<rten::Model, String> {
    if !path.exists() {
        return Err(format!(
            "{} is missing. The OCR models are large binaries kept out of git; \
             fetch them with `just ocr-models` or set {MODEL_DIR_VAR}.",
            path.display()
        ));
    }
    rten::Model::load_file(path).map_err(|error| format!("load {}: {error}", path.display()))
}

/// Read the character dictionary, one character per line.
///
/// Trailing whitespace is *not* trimmed: one legitimate entry in PP-OCR's
/// dictionary is the space character, and trimming turns it into an empty
/// string that silently drops spaces out of every recognised phrase.
fn load_dictionary(path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect())
}

/// Clip a requested region to the frame, defaulting to the whole thing.
fn clamp_region(frame: &Frame, region: Option<Rect>) -> Rect {
    let full = Rect {
        x: 0,
        y: 0,
        width: frame.width,
        height: frame.height,
    };
    let Some(region) = region else { return full };

    let left = region.x.max(0);
    let top = region.y.max(0);
    let right = (region.x + region.width as i32).min(frame.width as i32);
    let bottom = (region.y + region.height as i32).min(frame.height as i32);
    Rect {
        x: left,
        y: top,
        width: (right - left).max(0) as u32,
        height: (bottom - top).max(0) as u32,
    }
}
