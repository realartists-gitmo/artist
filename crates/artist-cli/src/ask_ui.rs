//! TUI renderer/editor for durable structured human decisions.

use std::collections::BTreeMap;

use artist_session::ask::{Answer, Question, Selection};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::theme;

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Answered(Answer),
    DismissedAll,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Options,
    OptionNote,
    FreeResponse,
}

pub struct AskPicker {
    queue: Vec<Question>,
    cursor: usize,
    chosen: Vec<usize>,
    option_notes: BTreeMap<usize, String>,
    free_response: String,
    focus: Focus,
    total: usize,
}

impl AskPicker {
    pub fn open(questions: Vec<Question>) -> Option<Self> {
        if questions.is_empty() {
            return None;
        }
        let total = questions.len();
        Some(Self {
            queue: questions,
            cursor: 0,
            chosen: Vec::new(),
            option_notes: BTreeMap::new(),
            free_response: String::new(),
            focus: Focus::Options,
            total,
        })
    }

    fn current(&self) -> &Question {
        &self.queue[0]
    }

    pub fn outstanding(&self) -> Vec<String> {
        self.queue
            .iter()
            .map(|question| question.id.clone())
            .collect()
    }

    pub fn is_done(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn height(&self, width: u16) -> u16 {
        if self.queue.is_empty() {
            return 0;
        }
        let inner = width.saturating_sub(4).max(20);
        let question_rows = wrapped_rows(&self.current().question, inner);
        let options = self.current().options.len().max(1) as u16;
        let annotated = self
            .chosen
            .iter()
            .filter(|index| {
                self.option_notes
                    .get(index)
                    .is_some_and(|note| !note.is_empty())
            })
            .count() as u16;
        question_rows + options + annotated + 7
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        if key.kind != KeyEventKind::Press || self.queue.is_empty() {
            return Outcome::Pending;
        }
        let options = self.current().options.len();
        match (key.code, self.focus) {
            (KeyCode::Esc, _) => return Outcome::DismissedAll,
            (KeyCode::Tab, _) => self.advance_focus(options),
            (KeyCode::Up, Focus::Options) if options > 0 => {
                self.cursor = self.cursor.checked_sub(1).unwrap_or(options - 1);
            }
            (KeyCode::Down, Focus::Options) if options > 0 => {
                self.cursor = (self.cursor + 1) % options;
            }
            (KeyCode::Char(' '), Focus::Options) if options > 0 => self.toggle(),
            (KeyCode::Enter, _) => return self.commit(),
            (KeyCode::Backspace, Focus::OptionNote) => {
                self.option_notes.entry(self.cursor).or_default().pop();
            }
            (KeyCode::Backspace, Focus::FreeResponse) => {
                self.free_response.pop();
            }
            (KeyCode::Char(c), Focus::OptionNote)
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.option_notes.entry(self.cursor).or_default().push(c);
            }
            (KeyCode::Char(c), Focus::FreeResponse)
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.free_response.push(c);
            }
            _ => {}
        }
        Outcome::Pending
    }

    fn advance_focus(&mut self, options: usize) {
        self.focus = match self.focus {
            Focus::Options if options == 0 => Focus::FreeResponse,
            Focus::Options if self.chosen.contains(&self.cursor) => Focus::OptionNote,
            Focus::Options => Focus::FreeResponse,
            Focus::OptionNote => Focus::FreeResponse,
            Focus::FreeResponse if options == 0 => Focus::FreeResponse,
            Focus::FreeResponse => Focus::Options,
        };
    }

    fn toggle(&mut self) {
        match self.chosen.iter().position(|index| *index == self.cursor) {
            Some(index) => {
                self.chosen.remove(index);
                self.option_notes.remove(&self.cursor);
            }
            None => self.chosen.push(self.cursor),
        }
        self.chosen.sort_unstable();
    }

    fn commit(&mut self) -> Outcome {
        let question = self.queue.remove(0);
        let chosen = std::mem::take(&mut self.chosen);
        let notes = std::mem::take(&mut self.option_notes);
        let free_response = std::mem::take(&mut self.free_response);

        let mut selections = chosen
            .into_iter()
            .filter_map(|index| {
                question.options.get(index).map(|option| Selection {
                    option_id: Some(option.id.clone()),
                    note: notes
                        .get(&index)
                        .map(|note| note.trim())
                        .filter(|note| !note.is_empty())
                        .map(str::to_owned),
                })
            })
            .collect::<Vec<_>>();
        if !free_response.trim().is_empty() {
            selections.push(Selection {
                option_id: None,
                note: Some(free_response.trim().to_owned()),
            });
        }

        self.cursor = 0;
        self.focus = Focus::Options;
        Outcome::Answered(Answer {
            question_id: question.id,
            selections,
        })
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        if self.queue.is_empty() || area.width < 4 || area.height < 3 {
            return;
        }
        frame.render_widget(Clear, area);
        let question = self.current();
        let answered = self.total.saturating_sub(self.queue.len()) + 1;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Question {answered} of {} ", self.total))
            .border_style(Style::default().fg(theme::cycle_color(0)));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines = vec![
            Line::from(Span::styled(
                question.question.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::default(),
        ];

        if question.options.is_empty() {
            lines.push(Line::from(Span::styled(
                "No fixed options — enter a free response below.",
                Style::default().fg(theme::cycle_color(1)),
            )));
        } else {
            for (index, option) in question.options.iter().enumerate() {
                let selected = self.chosen.contains(&index);
                let cursor = self.focus == Focus::Options && self.cursor == index;
                let marker = if selected { "[x]" } else { "[ ]" };
                let recommended = if option.recommended {
                    " (recommended)"
                } else {
                    ""
                };
                let style = if cursor {
                    Style::default()
                        .fg(theme::cycle_color(0))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(
                    format!("{marker} {}{recommended}", option.label),
                    style,
                )));
                if selected {
                    let active = self.focus == Focus::OptionNote && self.cursor == index;
                    let note = self
                        .option_notes
                        .get(&index)
                        .map(String::as_str)
                        .unwrap_or("");
                    lines.push(Line::from(Span::styled(
                        format!("    note: {note}{}", if active { "▌" } else { "" }),
                        Style::default().fg(theme::cycle_color(1)),
                    )));
                }
            }
        }

        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!(
                "Free response: {}{}",
                self.free_response,
                if self.focus == Focus::FreeResponse {
                    "▌"
                } else {
                    ""
                }
            ),
            Style::default().fg(theme::cycle_color(1)),
        )));
        lines.push(Line::from(Span::styled(
            "Space toggle · Tab note/free response · Enter submit · Esc dismiss",
            Style::default().fg(theme::cycle_color(1)),
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
}

fn wrapped_rows(text: &str, width: u16) -> u16 {
    if width == 0 {
        return 0;
    }
    text.lines()
        .map(|line| ((line.chars().count().max(1) as u16).saturating_add(width - 1)) / width)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_session::ask::QuestionOption;

    fn question(options: &[&str]) -> Question {
        Question {
            id: "q-internal".into(),
            ask_session: "ask:test".into(),
            question: "Choose any that apply".into(),
            options: options
                .iter()
                .enumerate()
                .map(|(index, label)| QuestionOption {
                    id: format!("o-{index}"),
                    label: (*label).into(),
                    recommended: index == 0,
                })
                .collect(),
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn empty_batch_opens_nothing() {
        assert!(AskPicker::open(Vec::new()).is_none());
    }

    #[test]
    fn enter_does_not_invent_a_single_selection() {
        let mut picker = AskPicker::open(vec![question(&["A", "B"])]).unwrap();
        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected answer")
        };
        assert!(answer.selections.is_empty());
    }

    #[test]
    fn multiple_options_are_always_selectable() {
        let mut picker = AskPicker::open(vec![question(&["A", "B"])]).unwrap();
        picker.handle_key(press(KeyCode::Char(' ')));
        picker.handle_key(press(KeyCode::Down));
        picker.handle_key(press(KeyCode::Char(' ')));
        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected answer")
        };
        assert_eq!(
            answer
                .selections
                .iter()
                .filter_map(|s| s.option_id.as_deref())
                .collect::<Vec<_>>(),
            ["o-0", "o-1"]
        );
    }

    #[test]
    fn each_selected_option_keeps_its_own_note() {
        let mut picker = AskPicker::open(vec![question(&["A", "B"])]).unwrap();
        picker.handle_key(press(KeyCode::Char(' ')));
        picker.handle_key(press(KeyCode::Tab));
        for c in "first".chars() {
            picker.handle_key(press(KeyCode::Char(c)));
        }
        picker.handle_key(press(KeyCode::Tab));
        picker.handle_key(press(KeyCode::Tab));
        picker.handle_key(press(KeyCode::Down));
        picker.handle_key(press(KeyCode::Char(' ')));
        picker.handle_key(press(KeyCode::Tab));
        for c in "second".chars() {
            picker.handle_key(press(KeyCode::Char(c)));
        }
        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected answer")
        };
        assert_eq!(answer.selections[0].note.as_deref(), Some("first"));
        assert_eq!(answer.selections[1].note.as_deref(), Some("second"));
    }

    #[test]
    fn zero_options_is_free_response() {
        let mut picker = AskPicker::open(vec![question(&[])]).unwrap();
        picker.handle_key(press(KeyCode::Tab));
        for c in "custom answer".chars() {
            picker.handle_key(press(KeyCode::Char(c)));
        }
        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected answer")
        };
        assert_eq!(answer.selections.len(), 1);
        assert_eq!(answer.selections[0].option_id, None);
        assert_eq!(answer.selections[0].note.as_deref(), Some("custom answer"));
    }
}
