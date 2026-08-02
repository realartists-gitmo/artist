//! SVTR: one cropped line in, CTC logits out, greedy-decoded to text.

use rten_tensor::prelude::*;
use rten_tensor::{NdTensor, NdTensorView};

use crate::model::{Frame, Rect};

/// The recogniser's input height is baked into the graph.
const HEIGHT: u32 = 48;
/// Widest line fed to the model. A longer crop is squeezed rather than split:
/// we are matching against a label the model already named, so the leading
/// characters carry the match and the tail rarely decides it.
const MAX_WIDTH: u32 = 640;
/// Below this the crop is upscaled instead, because the model has a fixed
/// receptive field and a two-pixel-wide glyph reads as noise.
const MIN_WIDTH: u32 = 16;

pub(super) fn recognize(
    model: &rten::Model,
    dictionary: &[String],
    frame: &Frame,
    rect: Rect,
) -> Result<String, String> {
    if rect.width == 0 || rect.height == 0 {
        return Ok(String::new());
    }

    let input = crop(frame, rect);
    let output = model
        .run_one(input.view().into(), None)
        .map_err(|error| format!("recognition: {error}"))?;
    let logits: NdTensor<f32, 3> = output
        .try_into()
        .map_err(|error| format!("recognition output: {error}"))?;

    Ok(decode(&logits.slice(0), dictionary))
}

/// Crop the box out of the frame and letterbox it to the model's fixed height.
///
/// Aspect ratio is preserved: squashing a line horizontally is exactly the
/// distortion the recogniser was not trained for, and it turns confident text
/// into confident nonsense.
fn crop(frame: &Frame, rect: Rect) -> NdTensor<f32, 4> {
    let scaled = ((rect.width as f32) * (HEIGHT as f32 / rect.height as f32)).round() as u32;
    let width = scaled.clamp(MIN_WIDTH, MAX_WIDTH);

    let mut input = NdTensor::zeros([1, 3, HEIGHT as usize, width as usize]);
    for y in 0..HEIGHT {
        let source_y = rect.y as u32 + (y as f32 / HEIGHT as f32 * rect.height as f32) as u32;
        for x in 0..width {
            let source_x = rect.x as u32 + (x as f32 / width as f32 * rect.width as f32) as u32;
            let offset = ((source_y.min(frame.height.saturating_sub(1)) * frame.width
                + source_x.min(frame.width.saturating_sub(1)))
                * 4) as usize;
            let Some(pixel) = frame.rgba.get(offset..offset + 3) else {
                continue;
            };
            for channel in 0..3 {
                // The recogniser wants [-1, 1], not the ImageNet statistics the
                // detector wants. Using the detector's normalization here is a
                // quiet way to get plausible-looking garbage out.
                let value = pixel[channel] as f32 / 255.0;
                input[[0, channel, y as usize, x as usize]] = (value - 0.5) / 0.5;
            }
        }
    }
    input
}

/// Greedy CTC decode: argmax per timestep, collapse runs, drop blanks.
///
/// Greedy rather than beam search on purpose. A beam buys accuracy on ambiguous
/// transcription, and we are not transcribing — we are matching against a known
/// candidate under a tolerant comparison, so the beam would cost time to
/// improve a number nobody reads.
fn decode(logits: &NdTensorView<f32, 2>, dictionary: &[String]) -> String {
    let view = logits.view();
    let [steps, classes] = view.shape();

    let mut text = String::new();
    let mut previous = usize::MAX;

    for step in 0..steps {
        let mut best = 0usize;
        let mut best_score = f32::NEG_INFINITY;
        for class in 0..classes {
            let score = view[[step, class]];
            if score > best_score {
                best_score = score;
                best = class;
            }
        }

        // Class 0 is the CTC blank. A repeat of the previous class is the same
        // character held across timesteps, not a doubled letter — which is why
        // "ll" survives only when a blank separates the two.
        if best != 0
            && best != previous
            && let Some(character) = dictionary.get(best - 1)
        {
            text.push_str(character);
        }
        previous = best;
    }
    text.trim().to_owned()
}
