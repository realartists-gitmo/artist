use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::theme::PASTEL_CYCLE;

const WIDTH: usize = 42;
const DIM_PERCENT: u16 = 65;
const SHADOW_PERCENT: u16 = 40;

pub(super) fn apply(lines: &mut [Line<'static>]) {
    for line in lines {
        let source_spans = std::mem::take(&mut line.spans);
        let mut column = 0;

        for span in source_spans {
            for character in span.content.chars() {
                let color = gradient_color(column);
                line.spans.push(Span::styled(
                    character.to_string(),
                    recolor(span.style, color),
                ));
                column += 1;
            }
        }
    }
}

fn recolor(mut style: Style, color: Color) -> Style {
    style.fg = match style.fg {
        Some(Color::White) => Some(color),
        Some(Color::Gray) => Some(shade(color, DIM_PERCENT)),
        Some(Color::DarkGray) => Some(shade(color, SHADOW_PERCENT)),
        other => other,
    };
    style.bg = match style.bg {
        Some(Color::Gray | Color::Yellow) => Some(shade(color, DIM_PERCENT)),
        Some(Color::Black) => None,
        other => other,
    };
    style
}

fn gradient_color(column: usize) -> Color {
    let denominator = WIDTH - 1;
    let scaled = column.min(denominator) * (PASTEL_CYCLE.len() - 1);
    let left_index = scaled / denominator;
    let remainder = scaled % denominator;
    let right_index = (left_index + 1).min(PASTEL_CYCLE.len() - 1);

    interpolate(
        PASTEL_CYCLE[left_index],
        PASTEL_CYCLE[right_index],
        remainder,
        denominator,
    )
}

fn interpolate(left: Color, right: Color, numerator: usize, denominator: usize) -> Color {
    let (left_red, left_green, left_blue) = rgb(left);
    let (right_red, right_green, right_blue) = rgb(right);

    Color::Rgb(
        interpolate_channel(left_red, right_red, numerator, denominator),
        interpolate_channel(left_green, right_green, numerator, denominator),
        interpolate_channel(left_blue, right_blue, numerator, denominator),
    )
}

fn interpolate_channel(left: u8, right: u8, numerator: usize, denominator: usize) -> u8 {
    let inverse = denominator - numerator;
    ((usize::from(left) * inverse + usize::from(right) * numerator + denominator / 2) / denominator)
        as u8
}

fn shade(color: Color, percent: u16) -> Color {
    let (red, green, blue) = rgb(color);
    let dim = |channel| ((u16::from(channel) * percent + 50) / 100) as u8;
    Color::Rgb(dim(red), dim(green), dim(blue))
}

fn rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(red, green, blue) => (red, green, blue),
        _ => unreachable!("the splash palette only contains RGB colors"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{PASTEL_BLUSH, PASTEL_PINK};

    #[test]
    fn gradient_reaches_both_palette_endpoints() {
        assert_eq!(gradient_color(0), PASTEL_PINK);
        assert_eq!(gradient_color(WIDTH - 1), PASTEL_BLUSH);
    }

    #[test]
    fn gradient_interpolates_between_palette_stops() {
        assert_eq!(gradient_color(20), Color::Rgb(221, 232, 211));
    }

    #[test]
    fn shading_scales_each_rgb_channel() {
        assert_eq!(
            shade(Color::Rgb(100, 200, 250), DIM_PERCENT),
            Color::Rgb(65, 130, 163)
        );
    }
}
