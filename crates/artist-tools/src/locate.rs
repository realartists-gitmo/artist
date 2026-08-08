//! Turning `(file, line)` into `(path, anchor)`.
//!
//! `artist-ast` reports locations the way a compiler does — a path and a line
//! number. Artist addresses lines by semantic occurrence anchor, because a line
//! number is invalidated by any insert above it while an anchor survives. Every
//! cross-file answer therefore has to be translated before the model sees it,
//! or it hands back coordinates in a system nothing else here speaks.
//!
//! Translation means *issuing* anchors for files the model has not read. That is
//! deliberate: naming a line to the model is an observation of that file, and an
//! observation it cannot act on is worse than useless — it invites an `edit`
//! against a handle that was never issued. Issuing costs slots in the allocator's
//! one-word deck, which the cross-file preference then spreads; that preference
//! is an ergonomics heuristic, not a correctness constraint, so pressure on it
//! degrades ranking rather than breaking anything.
//!
//! A single anchor is meaningless without its path — the ledger is keyed by
//! `(path, handle)` and the same handle legitimately means different lines in
//! different files. So cross-file output always renders both.

use crate::{ToolError, Workspace};
use hashline_tools::ReadFileRequest;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A location the model can act on directly.
pub struct Located {
    /// Workspace-relative path, as `edit` expects it.
    pub path: String,
    /// Semantic anchor for the line, when one could be issued.
    pub anchor: Option<String>,
    /// The original line, kept for the cases an anchor cannot cover: a line
    /// past the end of the file, or a file that could not be read.
    pub line: u32,
}

impl Located {
    /// `path@anchor`, falling back to `path:line`.
    ///
    /// `@` rather than `:` so the two forms cannot be confused. A reader seeing
    /// `:` knows the number is a line and will drift; seeing `@` knows the token
    /// is an anchor and will not.
    pub fn render(&self) -> String {
        match &self.anchor {
            Some(anchor) => format!("{}@{anchor}", self.path),
            None => format!("{}:{}", self.path, self.line),
        }
    }
}

/// Resolves locations to anchors, reconciling each file at most once.
///
/// One graph answer routinely names the same file many times — thirty callers
/// across six files — and each reconcile walks and hashes the whole file. The
/// cache makes that per-file rather than per-entry.
pub struct Locator<'a> {
    workspace: &'a Workspace,
    by_file: HashMap<PathBuf, HashMap<u32, String>>,
}

impl<'a> Locator<'a> {
    pub fn new(workspace: &'a Workspace) -> Self {
        Self {
            workspace,
            by_file: HashMap::new(),
        }
    }

    /// Resolve one location, issuing anchors for `file` if this is the first
    /// time it has been named.
    pub async fn locate(&mut self, file: &Path, line: u32) -> Located {
        let path = self.workspace.display(file);
        if !self.by_file.contains_key(file) {
            let issued = self.issue(&path).await.unwrap_or_default();
            self.by_file.insert(file.to_path_buf(), issued);
        }
        let anchor = self.by_file.get(file).and_then(|m| m.get(&line)).cloned();
        Located { path, anchor, line }
    }

    /// Reconcile the whole file and keep its line → anchor map.
    ///
    /// Errors are swallowed into an empty map on purpose: a graph answer that
    /// mentions an unreadable or since-deleted file should still render, with
    /// that one entry falling back to `path:line`, rather than failing whole.
    async fn issue(&self, path: &str) -> Result<HashMap<u32, String>, ToolError> {
        let read = self
            .workspace
            .files
            .read_file(
                &self.workspace.actor,
                ReadFileRequest {
                    path: path.to_string(),
                    start_line: 1,
                    max_lines: None,
                },
            )
            .await?;
        Ok(read
            .result
            .lines
            .iter()
            .map(|l| (l.line_number as u32, l.anchor.clone()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_an_anchor_when_one_was_issued() {
        let located = Located {
            path: "src/lib.rs".into(),
            anchor: Some("time".into()),
            line: 12,
        };
        assert_eq!(located.render(), "src/lib.rs@time");
    }

    /// A line with no anchor still has to render — a deleted file or an
    /// out-of-range line must degrade to something navigable, not vanish.
    #[test]
    fn falls_back_to_a_line_number_when_no_anchor_exists() {
        let located = Located {
            path: "src/lib.rs".into(),
            anchor: None,
            line: 12,
        };
        assert_eq!(located.render(), "src/lib.rs:12");
    }

    /// The separator has to distinguish the two, because they behave
    /// differently: a line number drifts under edits, an anchor does not.
    #[test]
    fn the_two_forms_are_visually_distinct() {
        let anchored = Located {
            path: "a.rs".into(),
            anchor: Some("time".into()),
            line: 1,
        };
        let numbered = Located {
            path: "a.rs".into(),
            anchor: None,
            line: 1,
        };
        assert!(anchored.render().contains('@'));
        assert!(!numbered.render().contains('@'));
    }

    /// Two-word handles contain a space; the rendered form must still parse as
    /// one location rather than looking like a path followed by a stray word.
    #[test]
    fn two_word_anchors_render_intact() {
        let located = Located {
            path: "src/lib.rs".into(),
            anchor: Some("time beta".into()),
            line: 3,
        };
        assert_eq!(located.render(), "src/lib.rs@time beta");
    }
}
