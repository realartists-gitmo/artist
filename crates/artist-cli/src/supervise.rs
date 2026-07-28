use crate::{theme, tool_ui};
use anyhow::Result;
use artist_session::SuperviseTool;
use ratatui::{
    TerminalOptions, Viewport,
    crossterm::{
        cursor::{Hide, Show},
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen},
    },
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget},
};
use std::collections::HashMap;
use unicode_width::UnicodeWidthChar;

pub(crate) fn run(
    terminal: &mut ratatui::DefaultTerminal,
    tools: Vec<SuperviseTool>,
    custom_icons: &HashMap<String, String>,
    inline_height: u16,
) -> Result<()> {
    if tools.is_empty() {
        return Ok(());
    }
    super::chat_ui::finish_inline(terminal)?;
    execute!(std::io::stdout(), EnterAlternateScreen, Hide)?;
    *terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Fullscreen,
    });
    let result = modal(terminal, tools, custom_icons);
    execute!(std::io::stdout(), LeaveAlternateScreen, Show)?;
    *terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(inline_height),
    });
    terminal.show_cursor()?;
    result
}

fn modal(
    terminal: &mut ratatui::DefaultTerminal,
    tools: Vec<SuperviseTool>,
    custom_icons: &HashMap<String, String>,
) -> Result<()> {
    let records = tools
        .into_iter()
        .map(|tool| tool_ui::ToolRecord::new(tool.id, tool.name, tool.arguments, tool.result))
        .collect::<Vec<_>>();
    let mut selected = records.len() - 1;
    loop {
        terminal.draw(|frame| render(frame, &records, selected, custom_icons))?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => break,
                KeyCode::Up => selected = (selected + records.len() - 1) % records.len(),
                KeyCode::Down => selected = (selected + 1) % records.len(),
                _ => {}
            },
            Event::Resize(_, _) => terminal.autoresize()?,
            _ => {}
        }
    }
    Ok(())
}

fn render(
    frame: &mut ratatui::Frame,
    records: &[tool_ui::ToolRecord],
    selected: usize,
    custom_icons: &HashMap<String, String>,
) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(frame.area());
    Paragraph::new(Line::from(vec![
        Span::styled(
            "  󰒓 supervise",
            Style::default()
                .fg(theme::PASTEL_MINT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} tool uses", records.len()),
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .render(header, frame.buffer_mut());

    let (lines, selected_start) = rows(records, selected, custom_icons, body.width);
    let scroll = selected_start.saturating_sub(1) as u16;
    Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .render(body, frame.buffer_mut());

    Paragraph::new("  ↑/↓ select and expand  ·  esc close")
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::TOP))
        .render(footer, frame.buffer_mut());
}

fn rows(
    records: &[tool_ui::ToolRecord],
    selected: usize,
    custom_icons: &HashMap<String, String>,
    width: u16,
) -> (Vec<Line<'static>>, usize) {
    let mut lines = Vec::new();
    let mut selected_start = 0;
    for (index, record) in records.iter().enumerate() {
        if index == selected {
            selected_start = lines.len();
        }
        let icon = tool_ui::icon_for(&record.name, custom_icons).unwrap_or(tool_ui::FALLBACK_ICON);
        let marker = if index == selected { "›" } else { " " };
        let title_style = if index == selected {
            Style::default()
                .fg(theme::PASTEL_WHITE)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let heading = format!(" {marker} {icon}  {}", record.title());
        let icon_start = heading.find(icon).unwrap_or(0);
        let icon_end = icon_start + icon.len();
        lines.push(Line::from(vec![
            Span::styled(
                heading[..icon_start].to_owned(),
                if index == selected {
                    Style::default().fg(theme::PASTEL_MINT)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
            Span::styled(
                heading[icon_start..icon_end].to_owned(),
                Style::default()
                    .fg(crate::chat_ui::tool_icon_color(icon))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(heading[icon_end..].to_owned(), title_style),
        ]));
        if index == selected {
            let expanded = tool_ui::expanded_lines(record);
            if expanded.is_empty() {
                lines.push(Line::styled(
                    "     │ no output",
                    Style::default().fg(Color::DarkGray),
                ));
            } else {
                lines.extend(
                    expanded.into_iter().flat_map(|line| {
                        expanded_row(&line.text, line.is_diff, usize::from(width))
                    }),
                );
            }
        }
    }
    (lines, selected_start)
}

fn expanded_row(content: &str, is_diff: bool, width: usize) -> Vec<Line<'static>> {
    let available = width.saturating_sub(7).max(1);
    content
        .lines()
        .flat_map(|line| wrap_display(&line.replace('\t', "    "), available))
        .map(|line| {
            let diff_content = line
                .split_once("│ ")
                .map_or(line.as_str(), |(_, content)| content);
            let color = if is_diff && diff_content.starts_with('+') {
                theme::PASTEL_MINT
            } else if is_diff && diff_content.starts_with('-') {
                theme::PASTEL_PINK
            } else {
                theme::PASTEL_WHITE
            };
            Line::from(vec![
                Span::styled("     │ ", Style::default().fg(Color::DarkGray)),
                Span::styled(line, Style::default().fg(color)),
            ])
        })
        .collect()
}

fn wrap_display(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut rows = vec![String::new()];
    let mut used = 0;
    for character in line.chars() {
        let character_width = character.width().unwrap_or(0);
        if used > 0 && used + character_width > width {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut().expect("at least one row").push(character);
        used += character_width;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_tool_expands_while_earlier_tools_stay_collapsed() {
        let records = vec![
            tool_ui::ToolRecord::new(
                "find",
                "find",
                serde_json::json!({"query":"config"}),
                "src/config.rs",
            ),
            tool_ui::ToolRecord::new(
                "read",
                "read",
                serde_json::json!({"path":"src/config.rs"}),
                "abc: first\ndef: second",
            ),
        ];

        let (rows, selected_start) = rows(&records, 1, &HashMap::new(), 80);
        let text = rows.iter().map(Line::to_string).collect::<Vec<_>>();

        assert_eq!(selected_start, 1);
        assert_eq!(text.len(), 4);
        assert!(text[0].contains("Searched files"));
        assert!(text[1].contains("Read src/config.rs"));
        assert_eq!(text[2], "     │ abc: first");
        assert_eq!(text[3], "     │ def: second");
    }

    #[test]
    fn expanded_output_wraps_without_losing_wide_characters() {
        assert_eq!(
            wrap_display("ab界cd", 4),
            vec!["ab界".to_owned(), "cd".to_owned()]
        );
    }
}
