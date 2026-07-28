use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Paragraph, Widget},
};
use std::time::Instant;

#[derive(Default)]
pub(crate) struct SubagentStatuses {
    active: Vec<SubagentStatus>,
}

struct SubagentStatus {
    id: String,
    role: String,
    phase: &'static str,
    started: Instant,
}

impl SubagentStatuses {
    pub(crate) fn start(&mut self, id: String, role: String) {
        self.active.retain(|status| status.id != id);
        self.active.push(SubagentStatus {
            id,
            role,
            phase: "thinking",
            started: Instant::now(),
        });
    }

    pub(crate) fn event(&mut self, id: &str, event: &artist_agent::PromptEvent) {
        let Some(phase) = phase_for(event) else {
            return;
        };
        if let Some(status) = self.active.iter_mut().find(|status| status.id == id) {
            status.phase = phase;
        }
    }

    pub(crate) fn finish(&mut self, id: &str) {
        self.active.retain(|status| status.id != id);
    }

    pub(crate) fn height(&self) -> u16 {
        self.active.len().try_into().unwrap_or(u16::MAX)
    }

    pub(crate) fn render(&self, buffer: &mut Buffer, area: Rect, animation_frame: usize) {
        for (status, y) in self.active.iter().zip(area.y..area.bottom()) {
            let row = Rect::new(area.x, y, area.width, 1);
            buffer.set_style(row, Style::default().bg(crate::theme::PANEL_BACKGROUND));
            let activity = crate::activity_indicator::activity_status(
                status.phase,
                status.started.elapsed(),
                animation_frame,
            );
            let text = crate::chat_ui::truncate_display_line(
                &format!("    {} · {} · {activity}", status.id, status.role),
                usize::from(row.width.max(1)),
            );
            Paragraph::new(text)
                .style(Style::default().fg(Color::DarkGray))
                .render(row, buffer);
            crate::chat_ui::fill_panel_background(buffer, row);
        }
    }
}

fn phase_for(event: &artist_agent::PromptEvent) -> Option<&'static str> {
    match event {
        artist_agent::PromptEvent::ReasoningSummaryDelta(_)
        | artist_agent::PromptEvent::ToolResult { .. }
        | artist_agent::PromptEvent::RuleFired { .. } => Some("thinking"),
        artist_agent::PromptEvent::TextDelta(_) => Some("responding"),
        artist_agent::PromptEvent::ToolCall { .. }
        | artist_agent::PromptEvent::ToolExecutionStart { .. }
        | artist_agent::PromptEvent::SubagentStarted { .. }
        | artist_agent::PromptEvent::SubagentFinished { .. } => Some("working"),
        artist_agent::PromptEvent::SubagentEvent { event, .. } => phase_for(event),
        artist_agent::PromptEvent::CompletionUsage { .. } => None,
    }
}
