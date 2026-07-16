#[derive(Default)]
pub(crate) struct PromptHistory {
    entries: Vec<String>,
    selected: Option<usize>,
    draft: String,
}

impl PromptHistory {
    pub fn from_prompts(entries: Vec<String>) -> Self {
        Self {
            entries,
            ..Self::default()
        }
    }
    pub fn push(&mut self, prompt: String) {
        self.entries.push(prompt);
        self.selected = None;
        self.draft.clear();
    }
    pub fn navigate(&mut self, up: bool, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        if self.selected.is_none() {
            self.draft = current.to_owned();
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

#[derive(Default)]
pub(crate) struct SteeringQueue {
    queued: Vec<String>,
    selected: Option<usize>,
    draft: String,
}

impl SteeringQueue {
    pub fn entries(&self) -> &[String] {
        &self.queued
    }
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }
    pub fn navigate(&mut self, up: bool, current: &str) -> Option<String> {
        if self.queued.is_empty() {
            return None;
        }
        if self.selected.is_none() {
            self.draft = current.to_owned();
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
    pub fn submit(&mut self, value: String) {
        if let Some(index) = self.selected.take() {
            self.queued[index] = value;
        } else {
            self.queued.push(value);
        }
        self.draft.clear();
    }
    pub fn remove_selected(&mut self) -> bool {
        let Some(index) = self.selected.take() else {
            return false;
        };
        self.queued.remove(index);
        self.draft.clear();
        true
    }
    pub fn mark_delivered(&mut self, message: &str) {
        if let Some(index) = self.queued.iter().position(|queued| queued == message) {
            self.queued.remove(index);
            self.selected = self.selected.and_then(|selected| {
                if selected == index {
                    None
                } else if selected > index {
                    Some(selected - 1)
                } else {
                    Some(selected)
                }
            });
        }
    }
    pub fn take(self) -> Vec<String> {
        self.queued
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recalls_prompts_and_restores_draft() {
        let mut history = PromptHistory::from_prompts(vec!["one".into(), "two".into()]);
        assert_eq!(history.navigate(true, "draft").as_deref(), Some("two"));
        assert_eq!(history.navigate(true, "two").as_deref(), Some("one"));
        assert_eq!(history.navigate(false, "one").as_deref(), Some("two"));
        assert_eq!(history.navigate(false, "two").as_deref(), Some("draft"));
    }
    #[test]
    fn edits_and_removes_queued_steering() {
        let mut queue = SteeringQueue::default();
        queue.submit("one".into());
        queue.submit("two".into());
        assert_eq!(queue.navigate(true, "draft").as_deref(), Some("two"));
        queue.submit("changed".into());
        assert_eq!(queue.entries(), &["one", "changed"]);
        queue.navigate(true, "");
        assert!(queue.remove_selected());
        assert_eq!(queue.entries(), &["one"]);
    }
}
