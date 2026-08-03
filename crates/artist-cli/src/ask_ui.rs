//! The TUI picker for a question the agent asked.
//!
//! Questions arrive mid-turn, while the model is blocked on the answer, so this
//! lives in the live region above the input rather than as a modal that steals
//! the screen — the transcript stays readable, and the user can see *why* they
//! are being asked.
//!
//! Answering is deliberately not the only way out. Escape dismisses, which is a
//! real answer: `Answer::dismissed` carries an empty selection, and the tool
//! reports it as "dismissed without choosing" so the model proceeds on its own
//! judgement rather than treating the turn as broken. A picker you cannot
//! decline is a worse prompt than no picker.
//!
//! State lives here and nothing is written back until the user commits a
//! question, so a half-filled batch that gets cancelled leaves the registry
//! untouched.

use artist_session::ask::{Answer, Question};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::theme;

/// Preview lines shown before truncating. Enough to judge a snippet by,
/// short enough that the card stays a decision rather than a document.
const PREVIEW_ROWS: usize = 6;

/// What the picker wants the caller to do next.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Still collecting; redraw.
    Pending,
    /// The user settled this question. Deliver it and keep going.
    Answered(Answer),
    /// The user dismissed everything still open.
    DismissedAll,
}

/// Where typing goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Options,
    Notes,
}

pub struct AskPicker {
    /// Questions still to answer, in the order they were posted.
    queue: Vec<Question>,
    cursor: usize,
    /// Indices chosen for the current question. A set rather than one index
    /// because multi-select is the same widget with a different commit rule.
    chosen: Vec<usize>,
    notes: String,
    focus: Focus,
    /// How many were in the batch when it opened, so the counter reads
    /// "2 of 3" rather than counting down to nothing.
    total: usize,
}

impl AskPicker {
    /// Open a picker over pending questions, or `None` if there are none.
    pub fn open(questions: Vec<Question>) -> Option<Self> {
        if questions.is_empty() {
            return None;
        }
        let total = questions.len();
        Some(Self {
            queue: questions,
            cursor: 0,
            chosen: Vec::new(),
            notes: String::new(),
            focus: Focus::Options,
            total,
        })
    }

    fn current(&self) -> &Question {
        &self.queue[0]
    }

    /// Ids still unanswered, for dismissing the rest on escape.
    pub fn outstanding(&self) -> Vec<String> {
        self.queue.iter().map(|q| q.id.clone()).collect()
    }

    pub fn is_done(&self) -> bool {
        self.queue.is_empty()
    }

    /// Rows this needs, so the caller can reserve them before drawing.
    pub fn height(&self, width: u16) -> u16 {
        if self.queue.is_empty() {
            return 0;
        }
        let question = self.current();
        let inner = width.saturating_sub(4).max(20);
        // Question text wraps; everything else is one row apiece.
        let question_rows = wrapped_rows(&question.question, inner);
        let option_rows = question.options.len() as u16;
        // The preview costs its own lines plus the blank row above it — not
        // two, which left a dangling empty row against the bottom border.
        let preview_rows = self
            .preview()
            .map(|preview| preview.lines().count().min(PREVIEW_ROWS) as u16 + 1)
            .unwrap_or(0);
        // question + blank + options + blank + notes + borders/hint
        question_rows + option_rows + preview_rows + 5
    }

    /// The preview of whatever option the cursor is on, if it has one.
    fn preview(&self) -> Option<&str> {
        self.current()
            .options
            .get(self.cursor)
            .and_then(|option| option.preview.as_deref())
            .filter(|preview| !preview.trim().is_empty())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        if key.kind != KeyEventKind::Press || self.queue.is_empty() {
            return Outcome::Pending;
        }
        let options = self.current().options.len();

        match (key.code, self.focus) {
            (KeyCode::Esc, _) => return Outcome::DismissedAll,

            // Tab moves between choosing and annotating rather than cycling
            // options: the notes field is where the real answer often is when
            // none of the options quite fit, so it needs to be one keystroke
            // away rather than buried.
            (KeyCode::Tab, _) => {
                self.focus = match self.focus {
                    Focus::Options => Focus::Notes,
                    Focus::Notes => Focus::Options,
                };
            }

            (KeyCode::Up, Focus::Options) => {
                self.cursor = self.cursor.checked_sub(1).unwrap_or(options - 1);
            }
            (KeyCode::Down, Focus::Options) => {
                self.cursor = (self.cursor + 1) % options.max(1);
            }
            (KeyCode::Char(' '), Focus::Options) if self.current().multi_select => {
                self.toggle();
            }
            // Single-select: space is a shortcut for "this one", matching what
            // a checkbox-shaped list trains the hand to do.
            (KeyCode::Char(' '), Focus::Options) => {
                self.chosen = vec![self.cursor];
            }

            (KeyCode::Enter, _) => return self.commit(),

            (KeyCode::Backspace, Focus::Notes) => {
                self.notes.pop();
            }
            (KeyCode::Char(c), Focus::Notes) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.notes.push(c);
            }
            _ => {}
        }
        Outcome::Pending
    }

    fn toggle(&mut self) {
        match self.chosen.iter().position(|index| *index == self.cursor) {
            Some(at) => {
                self.chosen.remove(at);
            }
            None => self.chosen.push(self.cursor),
        }
        self.chosen.sort_unstable();
    }

    /// Settle the current question and advance.
    ///
    /// On a single-select question the cursor *is* the answer even if the user
    /// never pressed space — moving to an option and hitting enter is what a
    /// list like this trains, and requiring a separate select would read as the
    /// picker ignoring them.
    fn commit(&mut self) -> Outcome {
        let question = self.queue.remove(0);
        let mut chosen = std::mem::take(&mut self.chosen);
        if chosen.is_empty() && !question.multi_select {
            chosen.push(self.cursor);
        }
        let selected = chosen
            .iter()
            .filter_map(|index| question.options.get(*index))
            .map(|option| option.label.clone())
            .collect();
        let notes = std::mem::take(&mut self.notes);

        self.cursor = 0;
        self.focus = Focus::Options;
        Outcome::Answered(Answer {
            question_id: question.id,
            selected,
            notes: (!notes.trim().is_empty()).then(|| notes.trim().to_owned()),
        })
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        if self.queue.is_empty() || area.height == 0 {
            return;
        }
        let question = self.current();
        // The transcript scrolls underneath, and a Paragraph does not clear its
        // own background — without this the previous frame bleeds through the
        // picker's empty cells.
        frame.render_widget(Clear, area);

        let answered = self.total - self.queue.len();
        let counter = if self.total > 1 {
            format!(" {} of {} ", answered + 1, self.total)
        } else {
            String::new()
        };
        let title = if question.header.trim().is_empty() {
            " artist is asking ".to_owned()
        } else {
            format!(" {} ", question.header.trim())
        };

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::styled(
            format!(" {}", question.question),
            Style::default()
                .fg(theme::PASTEL_WHITE)
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::raw(""));

        for (index, option) in question.options.iter().enumerate() {
            let on_cursor = index == self.cursor && self.focus == Focus::Options;
            let picked = self.chosen.contains(&index)
                || (self.chosen.is_empty() && !question.multi_select && index == self.cursor);
            // Radio for one-of, checkbox for many-of: the glyph tells the user
            // whether picking a second option will replace or add, before they
            // try it.
            let mark = match (question.multi_select, picked) {
                (true, true) => "[x]",
                (true, false) => "[ ]",
                (false, true) => "(•)",
                (false, false) => "( )",
            };
            let mut spans = vec![
                Span::styled(
                    if on_cursor { " ❯ " } else { "   " },
                    Style::default().fg(theme::PASTEL_PINK),
                ),
                Span::styled(
                    format!("{mark} "),
                    Style::default().fg(if picked {
                        theme::PASTEL_MINT
                    } else {
                        theme::PASTEL_BLUE
                    }),
                ),
                Span::styled(
                    option.label.clone(),
                    if on_cursor {
                        Style::default()
                            .fg(theme::PASTEL_WHITE)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme::PASTEL_WHITE)
                    },
                ),
            ];
            if !option.description.trim().is_empty() {
                spans.push(Span::styled(
                    format!("  {}", option.description.trim()),
                    Style::default().fg(theme::PASTEL_BLUE),
                ));
            }
            lines.push(Line::from(spans));
        }

        if let Some(preview) = self.preview() {
            lines.push(Line::raw(""));
            for line in preview.lines().take(PREVIEW_ROWS) {
                lines.push(Line::styled(
                    format!("   │ {line}"),
                    Style::default().fg(theme::PASTEL_YELLOW),
                ));
            }
        }

        lines.push(Line::raw(""));
        let notes_focused = self.focus == Focus::Notes;
        lines.push(Line::from(vec![
            Span::styled(
                "   notes ",
                Style::default().fg(if notes_focused {
                    theme::PASTEL_PINK
                } else {
                    theme::PASTEL_BLUE
                }),
            ),
            Span::styled(
                if self.notes.is_empty() && !notes_focused {
                    "(tab to add your own answer)".to_owned()
                } else {
                    format!("{}{}", self.notes, if notes_focused { "▏" } else { "" })
                },
                Style::default().fg(if self.notes.is_empty() && !notes_focused {
                    theme::PASTEL_BLUE
                } else {
                    theme::PASTEL_WHITE
                }),
            ),
        ]));

        let hint = if question.multi_select {
            " ↑↓ move · space toggle · tab notes · enter confirm · esc skip "
        } else {
            " ↑↓ move · tab notes · enter confirm · esc skip "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::PASTEL_PINK))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(theme::PASTEL_PINK)
                    .add_modifier(Modifier::BOLD),
            ))
            .title_top(Line::from(counter).right_aligned())
            .title_bottom(Line::styled(hint, Style::default().fg(theme::PASTEL_BLUE)));

        frame.render_widget(
            Paragraph::new(lines)
                .block(block)
                .style(Style::default().bg(theme::PANEL_BACKGROUND))
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

fn wrapped_rows(text: &str, width: u16) -> u16 {
    let width = width.max(1) as usize;
    text.lines()
        .map(|line| (line.chars().count().div_ceil(width)).max(1) as u16)
        .sum::<u16>()
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_session::ask::QuestionOption;

    fn question(id: &str, multi: bool, options: &[&str]) -> Question {
        Question {
            id: id.into(),
            header: "Store".into(),
            question: "Which store?".into(),
            multi_select: multi,
            options: options
                .iter()
                .map(|label| QuestionOption {
                    label: (*label).into(),
                    description: String::new(),
                    preview: None,
                })
                .collect(),
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn an_empty_batch_opens_nothing() {
        assert!(AskPicker::open(Vec::new()).is_none());
    }

    /// Arrow to an option and press enter — the thing a list like this trains
    /// the hand to do. Requiring a separate "select" keystroke first would read
    /// as the picker ignoring the user.
    #[test]
    fn moving_the_cursor_and_confirming_answers_with_that_option() {
        let mut picker = AskPicker::open(vec![question("q1", false, &["Sled", "Redb"])]).unwrap();
        picker.handle_key(press(KeyCode::Down));

        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("enter should settle the question");
        };
        assert_eq!(answer.selected, ["Redb"]);
        assert!(picker.is_done());
    }

    /// Single-select replaces rather than accumulating, or the "one of" promise
    /// the radio glyph makes would be a lie.
    #[test]
    fn single_select_keeps_exactly_one_choice() {
        let mut picker =
            AskPicker::open(vec![question("q1", false, &["Sled", "Redb", "Fjall"])]).unwrap();
        picker.handle_key(press(KeyCode::Char(' ')));
        picker.handle_key(press(KeyCode::Down));
        picker.handle_key(press(KeyCode::Char(' ')));

        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected an answer");
        };
        assert_eq!(answer.selected, ["Redb"]);
    }

    #[test]
    fn multi_select_accumulates_and_toggles_off() {
        let mut picker =
            AskPicker::open(vec![question("q1", true, &["Sled", "Redb", "Fjall"])]).unwrap();
        picker.handle_key(press(KeyCode::Char(' ')));
        picker.handle_key(press(KeyCode::Down));
        picker.handle_key(press(KeyCode::Char(' ')));
        picker.handle_key(press(KeyCode::Down));
        picker.handle_key(press(KeyCode::Char(' ')));
        // Back to the second and untick it.
        picker.handle_key(press(KeyCode::Up));
        picker.handle_key(press(KeyCode::Char(' ')));

        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected an answer");
        };
        assert_eq!(answer.selected, ["Sled", "Fjall"]);
    }

    /// A multi-select question the user ticked nothing on is genuinely "none of
    /// these", and must not be silently turned into the cursor's option.
    #[test]
    fn multi_select_can_answer_with_nothing_chosen() {
        let mut picker = AskPicker::open(vec![question("q1", true, &["Sled", "Redb"])]).unwrap();
        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected an answer");
        };
        assert!(answer.selected.is_empty());
    }

    /// Notes are where the real answer often is when none of the options fit,
    /// so they ride along with the choice rather than replacing it.
    #[test]
    fn notes_are_captured_alongside_the_choice() {
        let mut picker = AskPicker::open(vec![question("q1", false, &["Sled", "Redb"])]).unwrap();
        picker.handle_key(press(KeyCode::Tab));
        for c in "only if it embeds".chars() {
            picker.handle_key(press(KeyCode::Char(c)));
        }
        picker.handle_key(press(KeyCode::Backspace));

        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected an answer");
        };
        assert_eq!(answer.selected, ["Sled"]);
        assert_eq!(answer.notes.as_deref(), Some("only if it embed"));
    }

    /// Whitespace-only notes are not notes.
    #[test]
    fn blank_notes_are_dropped() {
        let mut picker = AskPicker::open(vec![question("q1", false, &["Sled", "Redb"])]).unwrap();
        picker.handle_key(press(KeyCode::Tab));
        picker.handle_key(press(KeyCode::Char(' ')));

        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected an answer");
        };
        assert!(answer.notes.is_none());
    }

    /// Space types a space while the notes field has focus rather than
    /// toggling the option behind it.
    #[test]
    fn space_types_into_notes_rather_than_toggling() {
        let mut picker = AskPicker::open(vec![question("q1", true, &["Sled", "Redb"])]).unwrap();
        picker.handle_key(press(KeyCode::Tab));
        for c in "a b".chars() {
            picker.handle_key(press(KeyCode::Char(c)));
        }

        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected an answer");
        };
        assert!(
            answer.selected.is_empty(),
            "nothing should have been ticked"
        );
        assert_eq!(answer.notes.as_deref(), Some("a b"));
    }

    /// A batch is answered one question at a time, and each answer carries its
    /// own question's id.
    #[test]
    fn a_batch_advances_and_each_answer_names_its_question() {
        let mut picker = AskPicker::open(vec![
            question("q1", false, &["Sled", "Redb"]),
            question("q2", false, &["Tokio", "Smol"]),
        ])
        .unwrap();

        let Outcome::Answered(first) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected the first answer");
        };
        assert_eq!(first.question_id, "q1");
        assert!(!picker.is_done(), "the second question is still open");

        // Cursor and notes reset between questions, so a choice does not carry
        // over into a question it was never made about.
        picker.handle_key(press(KeyCode::Down));
        let Outcome::Answered(second) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected the second answer");
        };
        assert_eq!(second.question_id, "q2");
        assert_eq!(second.selected, ["Smol"]);
        assert!(picker.is_done());
    }

    /// Declining is a real answer, and it must reach every question still open
    /// rather than only the one on screen.
    #[test]
    fn escape_dismisses_the_whole_batch() {
        let mut picker = AskPicker::open(vec![
            question("q1", false, &["Sled", "Redb"]),
            question("q2", false, &["Tokio", "Smol"]),
        ])
        .unwrap();
        assert_eq!(
            picker.handle_key(press(KeyCode::Esc)),
            Outcome::DismissedAll
        );
        assert_eq!(picker.outstanding(), ["q1", "q2"]);
    }

    /// The caller reserves rows before drawing, so a height of zero would draw
    /// the picker into nothing.
    #[test]
    fn height_accounts_for_options_and_preview() {
        let plain = AskPicker::open(vec![question("q1", false, &["Sled", "Redb"])]).unwrap();
        let bare = plain.height(80);
        assert!(bare >= 7, "got {bare}");

        let mut with_preview = question("q1", false, &["Sled", "Redb"]);
        with_preview.options[0].preview = Some("one\ntwo\nthree".into());
        let previewed = AskPicker::open(vec![with_preview]).unwrap();
        assert!(
            previewed.height(80) > bare,
            "a preview needs more room than none"
        );
    }

    /// Key releases arrive on some terminals and must not double-apply.
    #[test]
    fn only_key_presses_are_acted_on() {
        let mut picker = AskPicker::open(vec![question("q1", false, &["Sled", "Redb"])]).unwrap();
        let release =
            KeyEvent::new_with_kind(KeyCode::Down, KeyModifiers::NONE, KeyEventKind::Release);
        assert_eq!(picker.handle_key(release), Outcome::Pending);

        let Outcome::Answered(answer) = picker.handle_key(press(KeyCode::Enter)) else {
            panic!("expected an answer");
        };
        assert_eq!(answer.selected, ["Sled"], "the release must not have moved");
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use artist_session::ask::QuestionOption;
    use ratatui::{Terminal, backend::TestBackend};

    fn draw(picker: &AskPicker, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| picker.render(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn question(multi: bool) -> Question {
        Question {
            id: "q1".into(),
            header: "Store".into(),
            question: "Which store should we use?".into(),
            multi_select: multi,
            options: vec![
                QuestionOption {
                    label: "Sled".into(),
                    description: "embedded, no server".into(),
                    preview: Some("sled::open(\"data\")?".into()),
                },
                QuestionOption {
                    label: "Redb".into(),
                    description: "single file, MVCC".into(),
                    preview: None,
                },
            ],
        }
    }

    /// Everything the user needs to decide has to be on screen: the question,
    /// every option with its cost, and the keys that operate it.
    #[test]
    fn the_card_shows_the_question_options_and_controls() {
        let picker = AskPicker::open(vec![question(false)]).unwrap();
        let rendered = draw(&picker, 72, picker.height(72));

        assert!(rendered.contains("Store"), "header missing:\n{rendered}");
        assert!(
            rendered.contains("Which store should we use?"),
            "{rendered}"
        );
        assert!(rendered.contains("Sled"), "{rendered}");
        assert!(rendered.contains("embedded, no server"), "{rendered}");
        assert!(rendered.contains("Redb"), "{rendered}");
        assert!(rendered.contains("enter confirm"), "controls:\n{rendered}");
        assert!(rendered.contains("esc skip"), "{rendered}");
    }

    /// The glyph tells the user whether a second pick replaces or adds, before
    /// they try it — a radio for one-of, a checkbox for many-of.
    #[test]
    fn the_marker_distinguishes_one_of_from_many_of() {
        let single = AskPicker::open(vec![question(false)]).unwrap();
        let single = draw(&single, 72, 14);
        assert!(single.contains("(•)"), "single-select radio:\n{single}");
        assert!(!single.contains("[x]"), "{single}");
        assert!(!single.contains("space toggle"), "{single}");

        let multi = AskPicker::open(vec![question(true)]).unwrap();
        let multi = draw(&multi, 72, 14);
        assert!(multi.contains("[ ]"), "multi-select checkbox:\n{multi}");
        assert!(multi.contains("space toggle"), "hint should mention space");
    }

    /// The preview is the whole point of offering one: it has to be visible
    /// beside the option it belongs to.
    #[test]
    fn the_cursors_preview_is_shown() {
        let picker = AskPicker::open(vec![question(false)]).unwrap();
        let rendered = draw(&picker, 72, picker.height(72));
        assert!(rendered.contains("sled::open"), "{rendered}");
    }

    /// A batch tells the user how far through they are, so answering three
    /// questions does not feel unbounded.
    #[test]
    fn a_batch_shows_its_position() {
        let picker = AskPicker::open(vec![question(false), question(false)]).unwrap();
        let rendered = draw(&picker, 72, picker.height(72));
        assert!(rendered.contains("1 of 2"), "{rendered}");
    }

    /// Notes advertise themselves when empty; once focused they show what has
    /// been typed and where the caret is.
    #[test]
    fn the_notes_field_invites_then_echoes() {
        let mut picker = AskPicker::open(vec![question(false)]).unwrap();
        let empty = draw(&picker, 72, picker.height(72));
        assert!(empty.contains("tab to add your own answer"), "{empty}");

        picker.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        for c in "needs to embed".chars() {
            picker.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let typed = draw(&picker, 72, picker.height(72));
        assert!(typed.contains("needs to embed"), "{typed}");
        assert!(typed.contains('▏'), "caret should show where typing lands");
    }

    /// A narrow terminal must still produce a readable card rather than
    /// panicking or drawing nothing.
    #[test]
    fn a_narrow_terminal_still_renders() {
        let picker = AskPicker::open(vec![question(true)]).unwrap();
        let rendered = draw(&picker, 28, picker.height(28));
        assert!(rendered.contains("Sled"), "{rendered}");
    }
}
