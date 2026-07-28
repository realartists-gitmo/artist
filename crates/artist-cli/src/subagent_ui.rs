use crate::tool_ui::ToolUi;
use anyhow::Result;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};
use std::collections::HashMap;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) struct NestedChat {
    visible: String,
    reasoning: String,
    response_renderer: crate::response_output::Renderer,
    tools: ToolUi,
}

impl NestedChat {
    pub(crate) fn new(custom_icons: HashMap<String, String>) -> Self {
        Self {
            visible: String::new(),
            reasoning: String::new(),
            response_renderer: crate::response_output::Renderer::default(),
            tools: ToolUi::with_icons(custom_icons),
        }
    }

    pub(crate) fn event(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        event: artist_agent::PromptEvent,
    ) -> Result<()> {
        match event {
            artist_agent::PromptEvent::TextDelta(delta) => {
                self.flush_reasoning(terminal)?;
                self.visible.push_str(&delta);
                let width = usize::from(terminal.size()?.width.saturating_sub(8).max(1));
                while let Some(line) = take_visible_line(&mut self.visible, width) {
                    insert_response(terminal, &line, &mut self.response_renderer)?;
                }
            }
            artist_agent::PromptEvent::ReasoningSummaryDelta(delta) => {
                self.reasoning.push_str(&delta);
            }
            artist_agent::PromptEvent::ToolCall {
                id,
                name,
                arguments,
            } => {
                self.flush_response(terminal)?;
                self.flush_reasoning(terminal)?;
                if let Some(title) = self.tools.start(id, &name, &arguments) {
                    insert_tool_line(
                        terminal,
                        &title.text,
                        title.first,
                        title.is_diff,
                        title.icon.as_deref(),
                    )?;
                }
            }
            artist_agent::PromptEvent::ToolResult {
                id,
                content,
                images,
                ..
            } => {
                for line in self.tools.output(&id, &content).lines {
                    insert_tool_line(
                        terminal,
                        &line.text,
                        line.first,
                        line.is_diff,
                        line.icon.as_deref(),
                    )?;
                }
                if images > 0 {
                    insert_tool_line(
                        terminal,
                        &format!("[{images} image result(s) not shown]"),
                        false,
                        false,
                        None,
                    )?;
                }
            }
            artist_agent::PromptEvent::RuleFired { rule, matched } => {
                self.visible.clear();
                self.reasoning.clear();
                self.response_renderer.reset();
                let excerpt = matched.chars().take(60).collect::<String>();
                insert_status(
                    terminal,
                    &format!("rule {rule} fired on “{excerpt}” · retried"),
                )?;
            }
            artist_agent::PromptEvent::ToolExecutionStart { .. }
            | artist_agent::PromptEvent::CompletionUsage { .. } => {}
            artist_agent::PromptEvent::SubagentStarted { prompt, .. } => {
                insert_prompt(terminal, &prompt)?;
            }
            artist_agent::PromptEvent::SubagentEvent { event, .. } => {
                self.event(terminal, *event)?;
            }
            artist_agent::PromptEvent::SubagentFinished { outcome, .. } => {
                insert_status(terminal, &outcome)?;
            }
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        self.flush_reasoning(terminal)?;
        self.flush_response(terminal)
    }

    fn flush_reasoning(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        if self.reasoning.is_empty() {
            return Ok(());
        }
        insert_reasoning(terminal, &self.reasoning)?;
        self.reasoning.clear();
        Ok(())
    }

    fn flush_response(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        if self.visible.is_empty() {
            return Ok(());
        }
        insert_response(terminal, &self.visible, &mut self.response_renderer)?;
        self.visible.clear();
        Ok(())
    }
}

pub(crate) fn task_id(output: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(output)
        .ok()?
        .get("taskId")?
        .as_str()
        .map(str::to_owned)
}

pub(crate) fn is_foreground(arguments: &serde_json::Value) -> bool {
    let mode = arguments
        .get("mode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("run");
    matches!(mode, "run" | "")
        && arguments
            .get("background")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
}

const INDENT: u16 = 4;

pub(crate) fn insert_prompt(terminal: &mut ratatui::DefaultTerminal, text: &str) -> Result<()> {
    let width = terminal.size()?.width.saturating_sub(INDENT);
    let height = crate::message_box::frame_height(text, width);
    terminal.insert_before(height.saturating_add(1), |buffer| {
        crate::message_box::render(buffer, nested_area(buffer.area, height), text);
    })?;
    Ok(())
}

pub(crate) fn insert_status(terminal: &mut ratatui::DefaultTerminal, status: &str) -> Result<()> {
    terminal.insert_before(1, |buffer| {
        Paragraph::new(format!("  {status}"))
            .style(Style::default().fg(Color::DarkGray))
            .render(nested_area(buffer.area, 1), buffer);
    })?;
    Ok(())
}

fn insert_response(
    terminal: &mut ratatui::DefaultTerminal,
    output: &str,
    renderer: &mut crate::response_output::Renderer,
) -> Result<()> {
    let width = terminal.size()?.width.saturating_sub(INDENT).max(1);
    let text = renderer.render(output, usize::from(width));
    let height = text.lines.len().max(1) as u16;
    terminal.insert_before(height, |buffer| {
        Paragraph::new(text).render(nested_area(buffer.area, height), buffer);
    })?;
    Ok(())
}

fn insert_reasoning(terminal: &mut ratatui::DefaultTerminal, reasoning: &str) -> Result<()> {
    let text = crate::chat_ui::reasoning_chunk_text(reasoning, true);
    let height = text.lines.len().max(1) as u16;
    terminal.insert_before(height, |buffer| {
        Paragraph::new(text).render(nested_area(buffer.area, height), buffer);
    })?;
    Ok(())
}

fn insert_tool_line(
    terminal: &mut ratatui::DefaultTerminal,
    content: &str,
    first: bool,
    is_diff: bool,
    icon: Option<&str>,
) -> Result<()> {
    let terminal_width = terminal.size()?.width.saturating_sub(INDENT);
    let width = usize::from(terminal_width.max(1));
    let rows = content
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let line = line.replace('\t', "    ");
            let row_icon = icon.unwrap_or("󰒓");
            let prefix_width = if index == 0 && first {
                4 + row_icon.width()
            } else {
                4
            };
            let line = crate::chat_ui::truncate_display_line(
                &line,
                width.saturating_sub(prefix_width).max(1),
            );
            let diff_content = line
                .split_once("│ ")
                .map_or(line.as_str(), |(_, content)| content);
            let color = if first {
                crate::theme::PASTEL_WHITE
            } else if is_diff && diff_content.starts_with('+') {
                crate::theme::PASTEL_MINT
            } else if is_diff && diff_content.starts_with('-') {
                crate::theme::PASTEL_PINK
            } else {
                Color::Rgb(175, 175, 175)
            };
            if index == 0 && first {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        row_icon.to_owned(),
                        Style::default()
                            .fg(crate::chat_ui::tool_icon_color(row_icon))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {line}"), Style::default().fg(color)),
                ])
            } else {
                Line::styled(format!("    {line}"), Style::default().fg(color))
            }
        })
        .collect::<Vec<_>>();
    let height = rows.len().max(1) as u16;
    terminal.insert_before(height, |buffer| {
        Paragraph::new(Text::from(rows)).render(nested_area(buffer.area, height), buffer);
    })?;
    Ok(())
}

fn nested_area(area: Rect, height: u16) -> Rect {
    let indent = INDENT.min(area.width.saturating_sub(1));
    Rect::new(
        area.x.saturating_add(indent),
        area.y,
        area.width.saturating_sub(indent),
        height,
    )
}

fn take_visible_line(pending: &mut String, width: usize) -> Option<String> {
    let split = pending.find('\n').map(|index| index + 1).or_else(|| {
        let mut columns = 0;
        pending.char_indices().find_map(|(index, character)| {
            columns += character.width().unwrap_or(0);
            (columns > width).then_some(index)
        })
    })?;
    Some(pending.drain(..split).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_detection_excludes_background_lifecycle_calls() {
        assert!(is_foreground(&serde_json::json!({"prompt": "inspect"})));
        assert!(!is_foreground(&serde_json::json!({
            "mode": "start",
            "prompt": "inspect"
        })));
        assert!(!is_foreground(&serde_json::json!({
            "mode": "run",
            "background": true,
            "prompt": "inspect"
        })));
    }
}
