//! DBNet: an image in, a probability map out, boxes derived from where it is hot.

use rten_tensor::NdTensor;
use rten_tensor::prelude::*;

use super::TextBox;
use crate::model::{Frame, Rect};

/// Knobs worth having, with defaults taken from PP-OCR's own inference config.
#[derive(Clone, Debug)]
pub struct DetectOptions {
    /// Probability above which a pixel counts as text.
    pub threshold: f32,
    /// Mean probability a whole box must reach to be kept.
    pub box_threshold: f32,
    /// How far to grow each box. DBNet is trained to predict a *shrunk* region,
    /// so a box used as-is clips the glyphs it is supposed to contain.
    pub unclip_ratio: f32,
    /// Longest side the image is scaled to before detection.
    ///
    /// The model is fully convolutional and takes any size, so this trades
    /// accuracy on small text against time. It applies after cropping, so a
    /// damaged region rarely hits it at all.
    pub max_side: u32,
    /// Smallest box worth returning, in pixels.
    pub min_size: u32,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self {
            threshold: 0.3,
            box_threshold: 0.6,
            unclip_ratio: 1.5,
            max_side: 960,
            min_size: 3,
        }
    }
}

/// The network wants both dimensions to be a multiple of this.
const STRIDE: u32 = 32;

/// ImageNet normalization, which is what PP-OCR's detector was trained with.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

pub(super) fn detect(
    model: &rten::Model,
    frame: &Frame,
    region: Rect,
    options: &DetectOptions,
) -> Result<Vec<TextBox>, String> {
    let (input, scale_x, scale_y) = preprocess(frame, region, options.max_side);
    let (_, _, height, width) = input.shape().into();

    let output = model
        .run_one(input.view().into(), None)
        .map_err(|error| format!("detection: {error}"))?;
    let probabilities: NdTensor<f32, 4> = output
        .try_into()
        .map_err(|error| format!("detection output: {error}"))?;

    let map = probabilities.slice([0, 0]);
    let mut found = boxes_from_map(map.to_slice().as_ref(), width, height, options);

    // Back to frame coordinates: undo the resize, then undo the crop. Doing it
    // here rather than at the call site is what lets a caller crop freely
    // without having to remember it did.
    for text in &mut found {
        text.rect = Rect {
            x: region.x + (text.rect.x as f32 * scale_x).round() as i32,
            y: region.y + (text.rect.y as f32 * scale_y).round() as i32,
            width: (text.rect.width as f32 * scale_x).round() as u32,
            height: (text.rect.height as f32 * scale_y).round() as u32,
        };
    }
    found
        .retain(|text| text.rect.width >= options.min_size && text.rect.height >= options.min_size);
    Ok(found)
}

/// Crop, resize, normalize, and lay out as NCHW.
///
/// Returns the scale factors needed to map detections back, which are not
/// simply `1 / ratio` — rounding each axis up to a multiple of the stride makes
/// the two axes scale slightly differently.
fn preprocess(frame: &Frame, region: Rect, max_side: u32) -> (NdTensor<f32, 4>, f32, f32) {
    let ratio = {
        let longest = region.width.max(region.height) as f32;
        if longest > max_side as f32 {
            max_side as f32 / longest
        } else {
            1.0
        }
    };
    let width = round_up((region.width as f32 * ratio).round() as u32);
    let height = round_up((region.height as f32 * ratio).round() as u32);

    let mut input = NdTensor::zeros([1, 3, height as usize, width as usize]);
    for y in 0..height {
        // Nearest-neighbour. Bilinear would be marginally kinder to small text,
        // but the dominant term by far is *how much* we resize, and cropping to
        // a damaged region usually means not resizing at all.
        let source_y = region.y as u32 + (y as f32 / height as f32 * region.height as f32) as u32;
        for x in 0..width {
            let source_x = region.x as u32 + (x as f32 / width as f32 * region.width as f32) as u32;
            let offset = ((source_y.min(frame.height - 1) * frame.width
                + source_x.min(frame.width - 1))
                * 4) as usize;
            let Some(pixel) = frame.rgba.get(offset..offset + 3) else {
                continue;
            };
            for channel in 0..3 {
                let value = pixel[channel] as f32 / 255.0;
                input[[0, channel, y as usize, x as usize]] =
                    (value - MEAN[channel]) / STD[channel];
            }
        }
    }

    let scale_x = region.width as f32 / width as f32;
    let scale_y = region.height as f32 / height as f32;
    (input, scale_x, scale_y)
}

fn round_up(value: u32) -> u32 {
    value.max(STRIDE).div_ceil(STRIDE) * STRIDE
}

/// Turn the probability map into boxes.
///
/// PP-OCR's own postprocessing finds contours and fits a minimum-area rotated
/// rectangle. We deliberately do not: **GUI text is axis-aligned.** Rotated text
/// is a document-scanning concern, and a rotated box would only have to be
/// un-rotated again before we could crop it for recognition. Connected
/// components plus an axis-aligned bounding box is the same answer for our
/// inputs, with far less to get wrong.
fn boxes_from_map(
    map: &[f32],
    width: usize,
    height: usize,
    options: &DetectOptions,
) -> Vec<TextBox> {
    let mut visited = vec![false; width * height];
    let mut found = Vec::new();
    let mut queue: Vec<usize> = Vec::new();

    for start in 0..width * height {
        if visited[start] || map[start] < options.threshold {
            continue;
        }
        // Iterative flood fill. Recursion here would blow the stack on a large
        // connected region — a full-width heading is tens of thousands of
        // pixels.
        queue.clear();
        queue.push(start);
        visited[start] = true;

        let (mut min_x, mut min_y) = (usize::MAX, usize::MAX);
        let (mut max_x, mut max_y) = (0usize, 0usize);
        let mut total = 0.0f32;
        let mut area = 0usize;

        while let Some(index) = queue.pop() {
            let (x, y) = (index % width, index / width);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            total += map[index];
            area += 1;

            // Four-connected. Eight would bridge diagonally-touching glyphs into
            // one blob, which merges adjacent words that happen to lean.
            for (dx, dy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i64 + dx;
                let ny = y as i64 + dy;
                if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                    continue;
                }
                let neighbour = ny as usize * width + nx as usize;
                if !visited[neighbour] && map[neighbour] >= options.threshold {
                    visited[neighbour] = true;
                    queue.push(neighbour);
                }
            }
        }

        let confidence = total / area as f32;
        if confidence < options.box_threshold {
            continue;
        }

        let (box_width, box_height) = (max_x - min_x + 1, max_y - min_y + 1);
        // DBNet predicts a deliberately shrunk region, so a box used as-is cuts
        // the glyphs off. The standard expansion: grow by area × ratio ÷
        // perimeter, which widens a long thin line less than a chunky one.
        let perimeter = 2 * (box_width + box_height);
        let offset = if perimeter > 0 {
            (area as f32 * options.unclip_ratio / perimeter as f32).round() as i64
        } else {
            0
        };

        let left = (min_x as i64 - offset).max(0);
        let top = (min_y as i64 - offset).max(0);
        let right = (max_x as i64 + offset).min(width as i64 - 1);
        let bottom = (max_y as i64 + offset).min(height as i64 - 1);

        found.push(TextBox {
            rect: Rect {
                x: left as i32,
                y: top as i32,
                width: (right - left + 1) as u32,
                height: (bottom - top + 1) as u32,
            },
            text: String::new(),
            confidence,
        });
    }

    // Reading order, so a caller listing candidates gets them the way a person
    // would scan: down the screen, then across.
    found.sort_by_key(|text| (text.rect.y, text.rect.x));
    found
}
