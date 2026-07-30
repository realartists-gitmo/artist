use ratatui::style::Color;

pub(crate) const PASTEL_PINK: Color = Color::Rgb(0xFF, 0xCC, 0xE1);
pub(crate) const PASTEL_WHITE: Color = Color::Rgb(0xF2, 0xF1, 0xED);
pub(crate) const PASTEL_MINT: Color = Color::Rgb(0xCD, 0xE5, 0xD9);
pub(crate) const PASTEL_YELLOW: Color = Color::Rgb(0xF2, 0xEB, 0xCC);
pub(crate) const PASTEL_BLUE: Color = Color::Rgb(0xC6, 0xE2, 0xE7);
pub(crate) const PASTEL_BLUSH: Color = Color::Rgb(0xF7, 0xDD, 0xE8);
pub(crate) const PANEL_BACKGROUND: Color = Color::Rgb(32, 32, 32);

pub(crate) const PASTEL_CYCLE: [Color; 6] = [
    PASTEL_PINK,
    PASTEL_WHITE,
    PASTEL_MINT,
    PASTEL_YELLOW,
    PASTEL_BLUE,
    PASTEL_BLUSH,
];

pub(crate) fn cycle_color(index: usize) -> Color {
    PASTEL_CYCLE[index % PASTEL_CYCLE.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_follows_palette_order() {
        for (index, expected) in PASTEL_CYCLE.into_iter().enumerate() {
            assert_eq!(cycle_color(index), expected);
        }
    }

    #[test]
    fn cycle_wraps_at_palette_boundary() {
        assert_eq!(cycle_color(PASTEL_CYCLE.len()), PASTEL_PINK);
        assert_eq!(cycle_color(PASTEL_CYCLE.len() + 1), PASTEL_WHITE);
        assert_eq!(
            cycle_color(usize::MAX),
            PASTEL_CYCLE[usize::MAX % PASTEL_CYCLE.len()]
        );
    }
}
