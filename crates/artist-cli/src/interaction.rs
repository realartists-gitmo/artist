use crate::{input_atoms::InputAtoms, input_images::ImagePaste};

#[derive(Clone, Default)]
pub(crate) struct PromptEntry {
    pub display: String,
    pub atoms: InputAtoms,
}

#[derive(Default)]
pub(crate) struct PromptHistory {
    entries: Vec<PromptEntry>,
    selected: Option<usize>,
    draft: PromptEntry,
}

impl PromptHistory {
    pub fn from_prompts(entries: Vec<String>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|content| PromptEntry {
                    display: content.clone(),
                    atoms: InputAtoms::default(),
                })
                .collect(),
            ..Self::default()
        }
    }
    pub fn push(&mut self, display: String, atoms: InputAtoms) {
        self.entries.push(PromptEntry { display, atoms });
        self.selected = None;
        self.draft = PromptEntry::default();
    }
    pub fn navigate(
        &mut self,
        up: bool,
        current_display: &str,
        current_atoms: &InputAtoms,
    ) -> Option<PromptEntry> {
        if self.entries.is_empty() {
            return None;
        }
        if self.selected.is_none() {
            self.draft = PromptEntry {
                display: current_display.to_owned(),
                atoms: current_atoms.clone(),
            };
        }
        self.selected = if up {
            Some(
                self.selected
                    .map_or(self.entries.len() - 1, |index| index.saturating_sub(1)),
            )
        } else {
            match self.selected {
                Some(index) if index + 1 < self.entries.len() => Some(index + 1),
                Some(_) => None,
                None => return None,
            }
        };
        Some(
            self.selected
                .map_or_else(|| self.draft.clone(), |index| self.entries[index].clone()),
        )
    }
}

#[derive(Clone, Default)]
pub(crate) struct SteeringEntry {
    pub display: String,
    pub content: String,
    pub images: Vec<ImagePaste>,
    pub atoms: InputAtoms,
}

#[derive(Default)]
pub(crate) struct SteeringQueue {
    queued: Vec<SteeringEntry>,
    selected: Option<usize>,
    draft: SteeringEntry,
}

impl SteeringQueue {
    pub fn displays(&self) -> impl Iterator<Item = &str> {
        self.queued.iter().map(|entry| entry.display.as_str())
    }
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }
    pub fn navigate(
        &mut self,
        up: bool,
        current: &str,
        current_atoms: &InputAtoms,
    ) -> Option<SteeringEntry> {
        if self.queued.is_empty() {
            return None;
        }
        if self.selected.is_none() {
            self.draft = SteeringEntry {
                display: current.to_owned(),
                atoms: current_atoms.clone(),
                ..SteeringEntry::default()
            };
        }
        self.selected = if up {
            Some(
                self.selected
                    .map_or(self.queued.len() - 1, |index| index.saturating_sub(1)),
            )
        } else {
            match self.selected {
                Some(index) if index + 1 < self.queued.len() => Some(index + 1),
                Some(_) => None,
                None => return None,
            }
        };
        Some(
            self.selected
                .map_or_else(|| self.draft.clone(), |index| self.queued[index].clone()),
        )
    }
    pub fn submit(
        &mut self,
        display: String,
        content: String,
        images: Vec<ImagePaste>,
        atoms: InputAtoms,
    ) {
        let entry = SteeringEntry {
            display,
            content,
            images,
            atoms,
        };
        if let Some(index) = self.selected.take() {
            self.queued[index] = entry;
        } else {
            self.queued.push(entry);
        }
        self.draft = SteeringEntry::default();
    }
    pub fn remove_selected(&mut self) -> bool {
        let Some(index) = self.selected.take() else {
            return false;
        };
        self.queued.remove(index);
        self.draft = SteeringEntry::default();
        true
    }
    pub fn mark_delivered(&mut self, message: &str) -> Option<String> {
        // The steering handle delivers FIFO and this queue mirrors that
        // order, so the delivered entry is the front one — matching by
        // position keeps duplicate-content entries from desyncing the
        // mirror. Content search is only a defensive fallback.
        let front_matches = self
            .queued
            .first()
            .is_some_and(|entry| entry.content == message);
        if let Some(index) = if front_matches {
            Some(0)
        } else {
            self.queued
                .iter()
                .position(|queued| queued.content == message)
        } {
            let display = self.queued.remove(index).display;
            self.selected = self.selected.and_then(|selected| {
                if selected == index {
                    None
                } else if selected > index {
                    Some(selected - 1)
                } else {
                    Some(selected)
                }
            });
            return Some(display);
        }
        None
    }
    pub fn take(self) -> Vec<SteeringEntry> {
        self.queued
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recalls_prompts_and_restores_draft() {
        let mut history = PromptHistory::from_prompts(vec!["one".into(), "two".into()]);
        assert_eq!(
            history
                .navigate(true, "draft", &InputAtoms::default())
                .unwrap()
                .display,
            "two"
        );
        assert_eq!(
            history
                .navigate(true, "two", &InputAtoms::default())
                .unwrap()
                .display,
            "one"
        );
        assert_eq!(
            history
                .navigate(false, "one", &InputAtoms::default())
                .unwrap()
                .display,
            "two"
        );
        assert_eq!(
            history
                .navigate(false, "two", &InputAtoms::default())
                .unwrap()
                .display,
            "draft"
        );
    }
    #[test]
    fn edits_and_removes_queued_steering() {
        let mut queue = SteeringQueue::default();
        queue.submit("one".into(), "one".into(), vec![], InputAtoms::default());
        queue.submit("two".into(), "two".into(), vec![], InputAtoms::default());
        assert_eq!(
            queue
                .navigate(true, "draft", &InputAtoms::default())
                .unwrap()
                .display,
            "two"
        );
        queue.submit(
            "changed".into(),
            "changed".into(),
            vec![],
            InputAtoms::default(),
        );
        assert_eq!(queue.displays().collect::<Vec<_>>(), ["one", "changed"]);
        queue.navigate(true, "", &InputAtoms::default());
        assert!(queue.remove_selected());
        assert_eq!(queue.displays().collect::<Vec<_>>(), ["one"]);
    }
}
