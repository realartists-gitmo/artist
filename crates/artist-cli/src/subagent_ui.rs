use anyhow::Result;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
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
    prompt: String,
    phase: &'static str,
    started: Instant,
}

pub(crate) struct SettledSubagent {
    id: String,
    role: String,
    prompt: String,
    outcome: String,
}

impl SubagentStatuses {
    pub(crate) fn start(&mut self, id: String, role: String) {
        self.start_card(id, role, String::new());
    }

    pub(crate) fn start_card(&mut self, id: String, role: String, prompt: String) {
        self.active.retain(|status| status.id != id);
        self.active.push(SubagentStatus {
            id,
            role,
            prompt,
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

    pub(crate) fn settle(&mut self, id: &str, outcome: String) -> Option<SettledSubagent> {
        let index = self.active.iter().position(|status| status.id == id)?;
        let status = self.active.remove(index);
        Some(SettledSubagent {
            id: status.id,
            role: status.role,
            prompt: status.prompt,
            outcome,
        })
    }

    pub(crate) fn drain_running(&mut self) -> Vec<SettledSubagent> {
        self.active
            .drain(..)
            .map(|status| SettledSubagent {
                id: status.id,
                role: status.role,
                prompt: status.prompt,
                outcome: "running".into(),
            })
            .collect()
    }

    pub(crate) fn height(&self) -> u16 {
        self.active
            .len()
            .saturating_mul(2)
            .try_into()
            .unwrap_or(u16::MAX)
    }

    pub(crate) fn render(&self, buffer: &mut Buffer, area: Rect, animation_frame: usize) {
        for (index, status) in self.active.iter().enumerate() {
            let y = area
                .y
                .saturating_add(u16::try_from(index.saturating_mul(2)).unwrap_or(u16::MAX));
            if y.saturating_add(2) > area.bottom() {
                break;
            }
            let activity = crate::activity_indicator::activity_status(
                status.phase,
                status.started.elapsed(),
                animation_frame,
            );
            render_card(
                buffer,
                Rect::new(area.x, y, area.width, 2),
                &status.role,
                &status.prompt,
                &format!("{} · {} · {activity}", status.id, status.role),
            );
        }
    }
}

pub(crate) fn insert_settled(
    terminal: &mut ratatui::DefaultTerminal,
    status: SettledSubagent,
) -> Result<()> {
    terminal.insert_before(2, |buffer| {
        render_card(
            buffer,
            buffer.area,
            &status.role,
            &status.prompt,
            &format!("{} · {} · {}", status.id, status.role, status.outcome),
        );
    })?;
    Ok(())
}

pub(crate) fn is_launch(arguments: &serde_json::Value) -> bool {
    matches!(
        arguments
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("run"),
        "" | "run" | "start"
    )
}

pub(crate) fn is_running_output(output: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return false;
    };
    value.get("status").and_then(serde_json::Value::as_str) == Some("running")
}

fn render_card(buffer: &mut Buffer, area: Rect, role: &str, prompt: &str, status: &str) {
    buffer.set_style(area, Style::default().bg(crate::theme::PANEL_BACKGROUND));
    Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "",
            Style::default()
                .fg(crate::theme::PASTEL_BLUSH)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  Started {role} subagent: {prompt}"),
            Style::default().fg(crate::theme::PASTEL_WHITE),
        ),
    ]))
    .render(Rect::new(area.x, area.y, area.width, 1), buffer);
    Paragraph::new(format!("    {status}"))
        .style(Style::default().fg(Color::DarkGray))
        .render(
            Rect::new(area.x, area.y.saturating_add(1), area.width, 1),
            buffer,
        );
    crate::chat_ui::fill_panel_background(buffer, area);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_events_follow_activity_phases() {
        let mut statuses = SubagentStatuses::default();
        statuses.start_card("a-green-comet".into(), "explorer".into(), "inspect".into());
        assert_eq!(statuses.active[0].phase, "thinking");

        statuses.event(
            "a-green-comet",
            &artist_agent::PromptEvent::TextDelta("answer".into()),
        );
        assert_eq!(statuses.active[0].phase, "responding");

        statuses.event(
            "a-green-comet",
            &artist_agent::PromptEvent::ToolExecutionStart {
                id: "tool".into(),
                name: "read".into(),
            },
        );
        assert_eq!(statuses.active[0].phase, "working");

        statuses.event(
            "a-green-comet",
            &artist_agent::PromptEvent::ToolResult {
                id: "tool".into(),
                content: "result".into(),
                outcome: None,
                duration_ms: None,
                images: 0,
            },
        );
        assert_eq!(statuses.active[0].phase, "thinking");

        statuses.event(
            "a-green-comet",
            &artist_agent::PromptEvent::CompletionUsage { total_tokens: 42 },
        );
        statuses.event(
            "missing",
            &artist_agent::PromptEvent::TextDelta("ignored".into()),
        );
        assert_eq!(statuses.active[0].phase, "thinking");
    }

    #[test]
    fn status_rows_hide_child_text_and_keep_start_order() {
        let area = Rect::new(0, 0, 90, 4);
        let mut buffer = Buffer::empty(area);
        let mut statuses = SubagentStatuses::default();
        statuses.start_card(
            "a-green-comet".into(),
            "explorer".into(),
            "inspect files".into(),
        );
        statuses.start_card("a-soft-heron".into(), "worker".into(), "fix tests".into());
        statuses.event(
            "a-green-comet",
            &artist_agent::PromptEvent::TextDelta("secret child output".into()),
        );

        statuses.render(&mut buffer, area, 0);

        let rendered = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .replace('\u{2800}', " ");
        assert!(rendered.contains("Started explorer subagent: inspect files"));
        assert!(rendered.contains("a-green-comet · explorer · ⋮·⋮·⋮ responding [00:00 elapsed]"));
        assert!(rendered.contains("Started worker subagent: fix tests"));
        assert!(rendered.contains("a-soft-heron · worker · ⋮·⋮·⋮ thinking [00:00 elapsed]"));
        assert!(!rendered.contains("secret child output"));
        assert!(
            area.positions()
                .all(|position| buffer[position].bg == crate::theme::PANEL_BACKGROUND)
        );
    }

    #[test]
    fn finishing_removes_the_live_row() {
        let mut statuses = SubagentStatuses::default();
        statuses.start_card("a-green-comet".into(), "explorer".into(), "inspect".into());
        let settled = statuses
            .settle("a-green-comet", "completed".into())
            .unwrap();
        assert_eq!(statuses.height(), 0);
        assert_eq!(settled.id, "a-green-comet");
        assert_eq!(settled.outcome, "completed");
    }

    #[test]
    fn launch_and_running_detection_ignore_management_calls() {
        assert!(is_launch(&serde_json::json!({"prompt":"inspect"})));
        assert!(is_launch(&serde_json::json!({"mode":"start"})));
        assert!(!is_launch(&serde_json::json!({"mode":"read"})));
        assert!(is_running_output(
            r#"{"taskId":"a-green-comet","status":"running"}"#
        ));
        assert!(!is_running_output(
            r#"{"taskId":"a-green-comet","status":"completed"}"#
        ));
    }
}
