use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Block, Borders, Widget},
};

use crate::theme::PASTEL_CYCLE;

pub(crate) fn render(buffer: &mut Buffer, area: Rect) {
    let area = area.intersection(*buffer.area());
    if area.is_empty() {
        return;
    }

    Block::default().borders(Borders::ALL).render(area, buffer);

    let perimeter = perimeter(area);
    let perimeter_len = perimeter.len();
    for (index, position) in perimeter.into_iter().enumerate() {
        let palette_index = index * PASTEL_CYCLE.len() / perimeter_len;
        buffer[position].set_fg(PASTEL_CYCLE[palette_index]);
    }
}

fn perimeter(area: Rect) -> Vec<(u16, u16)> {
    let (left, top, right, bottom) = (area.left(), area.top(), area.right(), area.bottom());
    match (area.width, area.height) {
        (0, _) | (_, 0) => Vec::new(),
        (1, 1) => vec![(left, top)],
        (_, 1) => (left..right).map(|x| (x, top)).collect(),
        (1, _) => (top..bottom).map(|y| (left, y)).collect(),
        _ => {
            let mut cells =
                Vec::with_capacity((usize::from(area.width) + usize::from(area.height)) * 2 - 4);
            cells.extend((left..right).map(|x| (x, top)));
            cells.extend((top + 1..bottom).map(|y| (right - 1, y)));
            cells.extend((left..right - 1).rev().map(|x| (x, bottom - 1)));
            cells.extend((top + 1..bottom - 1).rev().map(|y| (left, y)));
            cells
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use ratatui::style::Color;

    use super::*;

    #[test]
    fn renders_default_sharp_border_glyphs() {
        let area = Rect::new(0, 0, 4, 3);
        let mut buffer = Buffer::empty(area);
        render(&mut buffer, area);
        let symbols: Vec<_> = perimeter(area)
            .into_iter()
            .map(|position| buffer[position].symbol())
            .collect();
        assert_eq!(symbols, ["┌", "─", "─", "┐", "│", "┘", "─", "─", "└", "│"]);
    }

    #[test]
    fn four_by_three_uses_broad_palette_sections() {
        let area = Rect::new(0, 0, 4, 3);
        let mut buffer = Buffer::empty(area);
        render(&mut buffer, area);
        let colors: Vec<_> = perimeter(area)
            .into_iter()
            .map(|position| buffer[position].fg)
            .collect();
        assert_eq!(
            colors,
            [
                PASTEL_CYCLE[0],
                PASTEL_CYCLE[0],
                PASTEL_CYCLE[1],
                PASTEL_CYCLE[1],
                PASTEL_CYCLE[2],
                PASTEL_CYCLE[3],
                PASTEL_CYCLE[3],
                PASTEL_CYCLE[4],
                PASTEL_CYCLE[4],
                PASTEL_CYCLE[5],
            ]
        );
    }

    #[test]
    fn degenerate_perimeters_are_unique_and_safe() {
        for area in [
            Rect::new(0, 0, 0, 0),
            Rect::new(0, 0, 1, 1),
            Rect::new(0, 0, 1, 4),
            Rect::new(0, 0, 4, 1),
            Rect::new(0, 0, 2, 2),
        ] {
            let cells = perimeter(area);
            assert_eq!(cells.len(), cells.iter().collect::<HashSet<_>>().len());
            let mut buffer = Buffer::empty(area);
            render(&mut buffer, area);
        }
    }

    #[test]
    fn preserves_backgrounds_and_interior_cells() {
        let area = Rect::new(0, 0, 3, 3);
        let mut buffer = Buffer::empty(area);
        buffer.set_style(area, ratatui::style::Style::new().bg(Color::Blue));
        buffer[(1, 1)].set_symbol("x").set_fg(Color::Green);
        render(&mut buffer, area);
        assert_eq!(buffer[(0, 0)].bg, Color::Blue);
        assert_eq!(buffer[(1, 1)].symbol(), "x");
        assert_eq!(buffer[(1, 1)].fg, Color::Green);
        assert_eq!(buffer[(1, 1)].bg, Color::Blue);
    }

    #[test]
    fn clips_to_buffer_without_spilling() {
        let buffer_area = Rect::new(0, 0, 5, 5);
        let mut buffer = Buffer::filled(buffer_area, ratatui::buffer::Cell::new("x"));
        let before = buffer.clone();
        render(&mut buffer, Rect::new(3, 3, 4, 4));
        let changed: HashSet<_> = buffer_area
            .positions()
            .filter(|&position| buffer[position] != before[position])
            .map(|position| (position.x, position.y))
            .collect();
        assert_eq!(changed, HashSet::from([(3, 3), (4, 3), (4, 4), (3, 4)]));
    }
}
