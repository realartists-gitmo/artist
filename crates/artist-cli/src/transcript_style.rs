use ratatui::style::Color;

pub(crate) struct TurnStyle {
    accent: Color,
}

impl TurnStyle {
    pub(crate) fn for_index(index: usize) -> Self {
        Self {
            accent: crate::theme::cycle_color(index),
        }
    }

    pub(crate) fn accent(&self) -> Color {
        self.accent
    }

    pub(crate) fn apply_markdown(&self, config: &mut glamour::StyleConfig, light: bool) {
        let accent = color_hex(self.accent);

        config.code.style.color = Some(accent.clone());
        config.code.style.background_color =
            Some(if light { "#E8E8E8" } else { "#202020" }.to_owned());

        for heading in [
            &mut config.heading,
            &mut config.h1,
            &mut config.h2,
            &mut config.h3,
            &mut config.h4,
            &mut config.h5,
            &mut config.h6,
        ] {
            heading.style.color = Some(accent.clone());
        }

        config.h1.style.background_color = None;
        config.h1.style.prefix.clear();
        config.h1.style.suffix.clear();
        config.h1.style.bold = Some(true);

        config.link.color = Some(accent.clone());
        config.link_text.color = Some(accent);
    }
}

fn color_hex(color: Color) -> String {
    let Color::Rgb(red, green, blue) = color else {
        unreachable!("the transcript palette contains only RGB colors");
    };
    format!("#{red:02X}{green:02X}{blue:02X}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{PASTEL_CYCLE, PASTEL_PINK};

    #[test]
    fn turn_accents_follow_and_wrap_the_palette() {
        for (index, expected) in PASTEL_CYCLE.into_iter().enumerate() {
            assert_eq!(TurnStyle::for_index(index).accent(), expected);
        }
        assert_eq!(
            TurnStyle::for_index(PASTEL_CYCLE.len()).accent(),
            PASTEL_PINK
        );
    }

    #[test]
    fn markdown_uses_accented_inline_code_without_an_h1_pill() {
        let mut config = glamour::Style::Dark.config();
        TurnStyle::for_index(0).apply_markdown(&mut config, false);

        assert_eq!(config.code.style.color.as_deref(), Some("#FFCCE1"));
        assert_eq!(
            config.code.style.background_color.as_deref(),
            Some("#202020")
        );
        assert_eq!(config.heading.style.color.as_deref(), Some("#FFCCE1"));
        assert_eq!(config.link.color.as_deref(), Some("#FFCCE1"));
        assert_eq!(config.h1.style.background_color, None);
        assert!(config.h1.style.prefix.is_empty());
        assert!(config.h1.style.suffix.is_empty());
    }
}
