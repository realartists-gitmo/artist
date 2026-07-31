//! The handoff tool.
//!
//! A handoff is terminal for the outgoing agent: there is no stack and no
//! return. The tool records the request in a shared slot, the run loop notices
//! it and ends the run, and the session continues on the target profile with
//! the payload as its opening task — functionally identical to clearing the
//! session and starting a new one on that profile.
//!
//! A profile cannot hand off to itself. Context management within a profile is
//! compaction's job; a self-handoff would be compaction with a worse
//! summarizer and no ability to reuse the prior summary chain.

use crate::profiles::Profiles;
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// What the outgoing agent wrote for its successor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handoff {
    pub to: String,
    pub summary: String,
    pub artifacts: Vec<String>,
    pub decisions: Vec<String>,
    pub open_questions: Vec<String>,
}

impl Handoff {
    /// The opening task for the incoming profile.
    ///
    /// The original user request is carried verbatim rather than summarized —
    /// it is ground truth, it is small, and summarizing the user's own words is
    /// the highest-regret loss in the whole exchange.
    pub fn seed(
        &self,
        from: &str,
        user_request: &str,
        chain: &[String],
        todos: &str,
    ) -> String {
        let mut seed = format!(
            "<handoff from=\"{from}\">\n<summary>\n{}\n</summary>",
            self.summary.trim()
        );
        for (tag, items) in [
            ("artifacts", &self.artifacts),
            ("decisions", &self.decisions),
            ("open-questions", &self.open_questions),
        ] {
            if items.is_empty() {
                continue;
            }
            seed.push_str(&format!("\n<{tag}>\n{}\n</{tag}>", items.join("\n")));
        }
        if chain.len() > 1 {
            // A handoff cannot hand back, but it can hand onward into a cycle.
            // Each hop is a fresh instance that would not otherwise see it.
            seed.push_str(&format!("\n<chain>{}</chain>", chain.join(" -> ")));
        }
        if !user_request.trim().is_empty() {
            seed.push_str(&format!(
                "\n<original-request>\n{}\n</original-request>",
                user_request.trim()
            ));
        }
        if !todos.is_empty() {
            seed.push_str(&format!("\n{todos}"));
        }
        seed.push_str("\n</handoff>");
        seed
    }
}

/// The slot a fired handoff lands in, mirroring how a stream rule stashes its
/// firing for the run loop to pick up.
#[derive(Clone, Default)]
pub(crate) struct HandoffShared(Arc<Mutex<Option<Handoff>>>);

impl HandoffShared {
    fn set(&self, handoff: Handoff) {
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = Some(handoff);
    }

    pub fn take(&self) -> Option<Handoff> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }
}

#[derive(Clone)]
pub(crate) struct HandoffTool {
    pending: HandoffShared,
    profiles: Profiles,
    current: String,
}

impl HandoffTool {
    pub fn new(pending: HandoffShared, profiles: Profiles, current: String) -> Self {
        Self {
            pending,
            profiles,
            current,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HandoffArgs {
    profile: String,
    summary: String,
    #[serde(default)]
    artifacts: Vec<String>,
    #[serde(default)]
    decisions: Vec<String>,
    #[serde(default)]
    open_questions: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("handoff rejected: {0}")]
pub(crate) struct HandoffError(String);

impl PortableTool for HandoffTool {
    const NAME: &'static str = "handoff";
    type Error = HandoffError;
    type Args = HandoffArgs;
    type Output = String;

    fn description(&self) -> String {
        "Hand the session to a different profile and stop. Your context is cleared and the target \
         profile starts fresh with your summary as its task, so write everything it needs — you do \
         not get to continue afterwards and cannot be handed back to."
            .into()
    }

    fn parameters(&self) -> Value {
        let targets: Vec<String> = self
            .profiles
            .names()
            .into_iter()
            .filter(|name| *name != self.current)
            .collect();
        json!({"type":"object","properties":{
            "profile":{"type":"string","enum":targets,"description":"The profile taking over."},
            "summary":{"type":"string","description":"What was done and what remains. This is the only prose the next profile sees."},
            "artifacts":{"type":"array","items":{"type":"string"},"description":"Files, branches, or outputs it should look at."},
            "decisions":{"type":"array","items":{"type":"string"},"description":"Settled choices it should not revisit."},
            "openQuestions":{"type":"array","items":{"type":"string"},"description":"What is still undecided."}
        },"required":["profile","summary"],"additionalProperties":false})
    }

    async fn call(&self, args: HandoffArgs) -> Result<String, HandoffError> {
        if args.profile == self.current {
            return Err(HandoffError(format!(
                "a profile cannot hand off to itself ({}); context management within a profile is compaction's job",
                self.current
            )));
        }
        self.profiles.get(&args.profile).map_err(HandoffError)?;
        if args.summary.trim().is_empty() {
            return Err(HandoffError(
                "summary is required; it is the only context the next profile receives".into(),
            ));
        }
        let target = args.profile.clone();
        self.pending.set(Handoff {
            to: args.profile,
            summary: args.summary,
            artifacts: args.artifacts,
            decisions: args.decisions,
            open_questions: args.open_questions,
        });
        Ok(format!(
            "Handing off to {target}. This run is over; do not continue working."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles() -> Profiles {
        let dir = tempfile::tempdir().unwrap();
        let profiles = Profiles::discover_from(dir.path(), None);
        std::mem::forget(dir);
        profiles
    }

    fn tool(current: &str) -> (HandoffShared, HandoffTool) {
        let pending = HandoffShared::default();
        let tool = HandoffTool::new(pending.clone(), profiles(), current.to_owned());
        (pending, tool)
    }

    fn args(profile: &str, summary: &str) -> HandoffArgs {
        HandoffArgs {
            profile: profile.into(),
            summary: summary.into(),
            artifacts: Vec::new(),
            decisions: Vec::new(),
            open_questions: Vec::new(),
        }
    }

    #[tokio::test]
    async fn a_handoff_stashes_the_payload_for_the_run_loop() {
        let (pending, tool) = tool("planner");
        tool.call(args("worker", "plan is done, implement step 1"))
            .await
            .unwrap();
        let handoff = pending.take().unwrap();
        assert_eq!(handoff.to, "worker");
        assert_eq!(handoff.summary, "plan is done, implement step 1");
    }

    /// Self-handoff would be compaction with a worse summarizer.
    #[tokio::test]
    async fn a_profile_cannot_hand_off_to_itself() {
        let (pending, tool) = tool("worker");
        let error = tool.call(args("worker", "carry on")).await.unwrap_err();
        assert!(error.to_string().contains("cannot hand off to itself"));
        assert!(pending.take().is_none());
    }

    #[tokio::test]
    async fn an_unknown_target_is_rejected_without_ending_the_run() {
        let (pending, tool) = tool("planner");
        let error = tool.call(args("nonexistent", "done")).await.unwrap_err();
        assert!(error.to_string().contains("unknown profile"));
        assert!(pending.take().is_none());
    }

    #[tokio::test]
    async fn an_empty_summary_is_rejected() {
        let (pending, tool) = tool("planner");
        let error = tool.call(args("worker", "   ")).await.unwrap_err();
        assert!(error.to_string().contains("summary is required"));
        assert!(pending.take().is_none());
    }

    /// The target list never offers the current profile.
    #[test]
    fn the_tool_schema_excludes_the_current_profile() {
        let (_, tool) = tool("worker");
        let targets = tool.parameters()["properties"]["profile"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(!targets.contains(&"worker".to_owned()));
        assert!(targets.contains(&"planner".to_owned()));
    }

    #[test]
    fn the_seed_carries_the_user_request_verbatim() {
        let handoff = Handoff {
            to: "worker".into(),
            summary: "planned the migration".into(),
            artifacts: vec!["docs/plan.md".into()],
            decisions: vec!["use serde_yaml".into()],
            open_questions: vec!["which crate owns the registry?".into()],
        };
        let seed = handoff.seed(
            "planner",
            "migrate the config loader to YAML",
            &["planner".into(), "worker".into()],
            "<todos>\n[ ] port the loader\n</todos>",
        );
        assert!(seed.contains("migrate the config loader to YAML"));
        assert!(seed.contains("docs/plan.md"));
        assert!(seed.contains("use serde_yaml"));
        assert!(seed.contains("which crate owns the registry?"));
        assert!(seed.contains("planner -> worker"), "{seed}");
        assert!(seed.contains("port the loader"), "todos cross verbatim: {seed}");
    }

    /// A single-hop chain is not worth the tokens; it says nothing the `from`
    /// attribute does not already say.
    #[test]
    fn a_first_hop_carries_no_chain() {
        let handoff = Handoff {
            to: "worker".into(),
            summary: "done".into(),
            ..Handoff::default()
        };
        let seed = handoff.seed("planner", "do the thing", &["planner".into()], "");
        assert!(!seed.contains("<chain>"), "{seed}");
    }
}
