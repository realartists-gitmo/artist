use ansi_to_tui::IntoText;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};

mod gradient;

pub(crate) const HEIGHT: u16 = 7;

const ART: &str = concat!(
    "\x1b[0;37;40m     \x1b[0;97;1;40m▄█▄\x1b[0;37;40m            \x1b[0;97;1;40m██\x1b[0;37;40m    \x1b[0;97;1;40m██\x1b[0;37;40m          \x1b[0;97;1;40m██\x1b[0;37;40m  \x1b[0m\n",
    "\x1b[0;37;40m    \x1b[0;97;1;40m██\x1b[0;37;40m \x1b[0;97;1;40m█▄\x1b[0;37;40m   \x1b[0;97;1;40m▄\x1b[0;37;40m      \x1b[0;97;1;47m▀\x1b[0;97;1;40m██\x1b[0;97;1;47m▀\x1b[0;37;40m  \x1b[0;97;1;40m▄\x1b[0;97;1;47m▄▄\x1b[0;37;40m   \x1b[0;97;1;40m▄▄▄▄▄\x1b[0;37;40m \x1b[0;97;1;47m▀\x1b[0;97;1;40m██\x1b[0;97;1;47m▀\x1b[0;37;40m \x1b[0m\n",
    "\x1b[0;37;40m   \x1b[0;97;1;40m██\x1b[0;37;40m█ \x1b[0;97;1;47m▀\x1b[0;97;1;40m█\x1b[0;37;40m   \x1b[0;97;1;40m██▄██▄\x1b[0;37;40m  \x1b[0;97;1;40m██\x1b[0;37;40m   ▀\x1b[0;97;1;40m██\x1b[0;37;40m  \x1b[0;97;1;47m▀\x1b[0;97;1;40m██\x1b[0;97;1;47m▄\x1b[0;37;40m▀▀  \x1b[0;97;1;40m██\x1b[0;37;40m  \x1b[0m\n",
    "\x1b[0;37;40m  \x1b[0;97;1;40m██\x1b[0;97;1;47m▀▀▀▀\x1b[0;97;1;40m██\x1b[0;37;40m  \x1b[0;97;1;40m██\x1b[0;37;40m▀\x1b[0;97;1;47m▀▀\x1b[0;37;40m▀  \x1b[0;97;1;40m██\x1b[0;37;40m    \x1b[0;97;1;40m██\x1b[0;37;40m   ▀\x1b[0;97;1;47m▀\x1b[0;97;1;40m██▄\x1b[0;37;40m  \x1b[0;97;1;40m██\x1b[0;37;40m  \x1b[0m\n",
    "\x1b[0;97;1;40m▄██\x1b[0;97;1;47m▄\x1b[0;97;1;40m▄\x1b[0;37;40m \x1b[0;90;1;40m▄\x1b[0;97;1;40m▄██▄\x1b[0;37;40m \x1b[0;97;1;40m██\x1b[0;37;40m      \x1b[0;97;1;47m▀\x1b[0;97;1;40m█▄▄\x1b[0;37;40m \x1b[0;97;1;40m▄██▄\x1b[0;37;40m \x1b[0;97;1;47m▀\x1b[0;97;1;40m▄▄█\x1b[0;97;1;47m▀\x1b[0;37;40m▀  \x1b[0;97;1;47m▀\x1b[0;97;1;40m█▄▄\x1b[0m\n",
    "\x1b[0;37;40m▀▀▀▀▀ \x1b[0;90;1;40m▀\x1b[0;37;40m▀▀▀▀ ▀▀       ▀▀▀ ▀▀▀▀  ▀▀▀     ▀▀▀\x1b[0m\n",
);

fn splash_text(extension_ids: &[String]) -> Text<'static> {
    let mut text = ART
        .into_text()
        .expect("embedded startup splash must contain valid ANSI");
    gradient::apply(&mut text.lines);
    for line in &mut text.lines {
        line.spans.insert(0, Span::raw(" "));
    }

    let mut footer = if extension_ids.is_empty() {
        Line::default()
    } else {
        Line::from(Span::styled(" + ", Style::default().fg(Color::DarkGray)))
    };

    for (index, extension_id) in extension_ids.iter().enumerate() {
        if index > 0 {
            footer.push_span(Span::styled(" • ", Style::default().fg(Color::DarkGray)));
        }
        footer.push_span(Span::styled(
            extension_id.clone(),
            Style::default().fg(crate::theme::cycle_color(index)),
        ));
    }

    text.lines.push(footer);
    text
}

pub(crate) fn render(frame: &mut Frame<'_>, area: Rect, extension_ids: &[String]) {
    let area = area.intersection(frame.area());
    if !area.is_empty() {
        frame.render_widget(Paragraph::new(splash_text(extension_ids)), area);
    }
}

pub(crate) fn render_buffer(buffer: &mut Buffer, extension_ids: &[String]) {
    Paragraph::new(splash_text(extension_ids)).render(buffer.area, buffer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, text::Line};

    #[test]
    fn embeds_six_styled_art_rows() {
        let text = splash_text(&[]);

        assert_eq!(text.lines.len(), HEIGHT as usize);
        assert!(text.lines[..6].iter().all(|line| line.width() == 43));
        assert_eq!(
            text.lines[0].to_string(),
            "      ▄█▄            ██    ██          ██  "
        );
        assert_eq!(
            text.lines[5].to_string(),
            " ▀▀▀▀▀ ▀▀▀▀▀ ▀▀       ▀▀▀ ▀▀▀▀  ▀▀▀     ▀▀▀"
        );
        assert!(
            text.lines[..6]
                .iter()
                .flat_map(|line| &line.spans)
                .all(|span| !matches!(
                    (span.style.fg, span.style.bg),
                    (Some(Color::Yellow | Color::LightYellow), _)
                        | (_, Some(Color::Yellow | Color::LightYellow))
                ))
        );
    }

    #[test]
    fn a_has_centered_counter_with_one_shadow_cell() {
        let text = splash_text(&[]);
        let rows = text.lines[..6]
            .iter()
            .map(|line| line.to_string().chars().skip(1).take(11).collect())
            .collect::<Vec<String>>();

        assert_eq!(
            rows,
            [
                "     ▄█▄   ",
                "    ██ █▄  ",
                "   ███ ▀█  ",
                "  ██▀▀▀▀██ ",
                "▄██▄▄ ▄▄██▄",
                "▀▀▀▀▀ ▀▀▀▀▀",
            ]
        );
        assert!(
            ['▓', '▒', '░']
                .into_iter()
                .all(|character| !ART.contains(character))
        );
        assert!(!ART.contains("\x1b[0;97;1;43m"));
    }

    #[test]
    fn footer_contains_only_ordered_ids_and_real_bullets() {
        let extensions = vec!["alpha".to_owned(), "beta".to_owned()];
        let text = splash_text(&extensions);
        let footer = &text.lines[6];

        assert_eq!(footer.to_string(), " + alpha • beta");
        assert_eq!(footer.spans[0].style.fg, Some(Color::DarkGray));
        assert_eq!(footer.spans[1].style.fg, Some(crate::theme::cycle_color(0)));
        assert_eq!(footer.spans[2].style.fg, Some(Color::DarkGray));
        assert_eq!(footer.spans[3].style.fg, Some(crate::theme::cycle_color(1)));
        assert!(!footer.to_string().contains("active extensions"));
        assert!(!footer.to_string().contains(','));
    }

    #[test]
    fn empty_extensions_leave_footer_blank() {
        let text = splash_text(&[]);

        assert_eq!(text.lines.len(), HEIGHT as usize);
        assert_eq!(text.lines[6], Line::default());
    }

    #[test]
    fn clips_art_and_footer_to_narrow_area() {
        let mut terminal = Terminal::new(TestBackend::new(8, HEIGHT)).unwrap();
        let extensions = vec!["alpha".to_owned(), "beta".to_owned()];
        terminal
            .draw(|frame| render(frame, frame.area(), &extensions))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let footer = (0..8)
            .map(|x| buffer.cell((x, HEIGHT - 1)).unwrap().symbol())
            .collect::<String>();
        assert_eq!(footer, " + alpha");
        assert_eq!(buffer.cell((5, 0)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell((6, 0)).unwrap().symbol(), "▄");
    }
}
