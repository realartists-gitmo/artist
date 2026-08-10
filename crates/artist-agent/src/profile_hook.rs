//! Non-interrupting propagation of a changed resolved profile.
//!
//! Profiles are deliberately re-resolved from disk at model-call boundaries.
//! A bad edit leaves the last valid profile in force; it never tears down an
//! active run or replaces its context with an error page.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use rig_agent::agent::{
    AgentHook, CompletionCallAction, CompletionCallEvent, HookContext, RequestPatch, StepEventKind,
    ToolCall, ToolCallAction,
};
use rig_core::completion::Document;
use sha2::{Digest, Sha256};

use crate::profiles::{Profile, Profiles};

#[derive(Clone)]
pub(crate) struct ProfileUpdateHook {
    project: PathBuf,
    name: String,
    active_revision: Arc<Mutex<String>>,
    active_profile: Arc<Mutex<Profile>>,
}

impl ProfileUpdateHook {
    pub(crate) fn new(project: PathBuf, profile: &Profile) -> Self {
        Self {
            project,
            name: profile.name.clone(),
            active_revision: Arc::new(Mutex::new(revision(profile))),
            active_profile: Arc::new(Mutex::new(profile.clone())),
        }
    }

    fn updated_profile(&self) -> Option<Profile> {
        let profile = Profiles::discover(&self.project).get(&self.name).ok()?;
        let current = revision(&profile);
        let mut active = self
            .active_revision
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *active == current {
            return None;
        }
        *active = current;
        *self
            .active_profile
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = profile.clone();
        Some(profile)
    }

    /// Resolve a profile again for an execution boundary. A malformed edit
    /// cannot widen authority: the last successfully resolved profile stays
    /// authoritative until a valid replacement is available. This does not
    /// advance `active_revision`: a policy check between completion calls must
    /// not consume the instruction-context patch due at the next call.
    fn permits_current(&self, tool_name: &str) -> bool {
        let profile = Profiles::discover(&self.project)
            .get(&self.name)
            .ok()
            .unwrap_or_else(|| {
                self.active_profile
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone()
            });
        profile.permits(tool_name)
    }
}

fn revision(profile: &Profile) -> String {
    if let Some(source) = &profile.source
        && let Ok(bytes) = std::fs::read(source)
    {
        return digest(&bytes);
    }
    digest(
        format!(
            "{}\n{}\n{}\n{:?}\n{:?}",
            profile.name,
            profile.description,
            profile.instructions,
            profile.candidates,
            profile.yield_schema,
        )
        .as_bytes(),
    )
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn document(profile: &Profile) -> Document {
    Document {
        id: "artist-profile-update".into(),
        text: format!(
            "<artist_profile_update name=\"{}\">\nThe resolved profile changed while this run remained active. Follow these replacement instructions for subsequent work units; previously completed work remains valid.\n\n{}\n</artist_profile_update>",
            profile.name, profile.instructions
        ),
        additional_props: Default::default(),
    }
}

impl AgentHook for ProfileUpdateHook {
    fn observes(&self, kind: StepEventKind) -> bool {
        matches!(
            kind,
            StepEventKind::CompletionCall | StepEventKind::ToolCall
        )
    }

    async fn on_completion_call(
        &self,
        _context: &HookContext,
        _event: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        let Some(profile) = self.updated_profile() else {
            return CompletionCallAction::continue_run();
        };
        CompletionCallAction::patch(RequestPatch::new().extra_context([document(&profile)]))
    }

    async fn on_tool_call(&self, _context: &HookContext, event: ToolCall<'_>) -> ToolCallAction {
        if self.permits_current(event.tool_name) {
            ToolCallAction::run()
        } else {
            ToolCallAction::skip(format!(
                "tool `{}` is denied by the current resolved profile; choose a permitted tool",
                event.tool_name
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_document_carries_only_the_new_profile_instructions() {
        let profile = Profiles::discover_from(std::path::Path::new("/missing"), None)
            .get("default")
            .unwrap();
        let rendered = document(&profile).text;
        assert!(rendered.contains("artist_profile_update"));
        assert!(rendered.contains("replacement instructions"));
    }

    #[test]
    fn changed_profile_source_is_detected_at_the_next_completion_boundary() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".artist/profiles/live.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "---\ndescription: live\n---\nfirst instructions\n").unwrap();
        let initial = Profiles::discover(root.path()).get("live").unwrap();
        let hook = ProfileUpdateHook::new(root.path().to_path_buf(), &initial);
        assert!(hook.updated_profile().is_none());
        std::fs::write(
            &path,
            "---\ndescription: live\n---\nreplacement instructions\n",
        )
        .unwrap();
        let changed = hook.updated_profile().expect("profile update");
        assert_eq!(changed.instructions, "replacement instructions");
        assert!(
            hook.updated_profile().is_none(),
            "an unchanged revision re-patches nothing"
        );
    }

    #[test]
    fn a_live_policy_edit_revokes_a_previously_registered_tool() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".artist/profiles/live.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "---\ndescription: live\ntools:\n  allow: [read, bash]\n---\nfirst instructions\n",
        )
        .unwrap();
        let initial = Profiles::discover(root.path()).get("live").unwrap();
        let hook = ProfileUpdateHook::new(root.path().to_path_buf(), &initial);
        assert!(hook.permits_current("bash"));
        std::fs::write(
            &path,
            "---\ndescription: live\ntools:\n  allow: [read]\n---\nreplacement instructions\n",
        )
        .unwrap();
        assert!(!hook.permits_current("bash"));
        assert!(hook.permits_current("read"));
        assert_eq!(
            hook.updated_profile()
                .expect("the replacement instructions remain pending")
                .instructions,
            "replacement instructions"
        );
    }

    #[test]
    fn an_invalid_live_policy_edit_keeps_the_last_valid_policy() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".artist/profiles/live.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "---\ndescription: live\ntools:\n  allow: [read]\n---\nfirst instructions\n",
        )
        .unwrap();
        let initial = Profiles::discover(root.path()).get("live").unwrap();
        let hook = ProfileUpdateHook::new(root.path().to_path_buf(), &initial);
        assert!(!hook.permits_current("bash"));
        std::fs::write(&path, "---\ntools: [not-an-object]\n---\nbroken\n").unwrap();
        assert!(!hook.permits_current("bash"));
        assert!(hook.permits_current("read"));
    }
}
