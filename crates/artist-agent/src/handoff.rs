//! Terminal profile handoff.

use crate::profiles::Profiles;
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handoff {
    pub to: String,
    pub brief: String,
}

impl Handoff {
    pub fn seed(&self, from: &str, user_request: &str, chain: &[String], todos: &str) -> String {
        let mut seed = format!(
            "<handoff from=\"{from}\">\n<brief>\n{}\n</brief>",
            self.brief.trim()
        );
        if chain.len() > 1 {
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
#[serde(deny_unknown_fields)]
pub(crate) struct HandoffArgs {
    profile: String,
    brief: String,
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
        format!(
            "Hand the session to a profile and stop this run. Same-profile handoff is allowed. Current profile: {}.",
            self.current
        )
    }

    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "profile":{"type":"string","enum":self.profiles.names()},
                "brief":{"type":"string","description":"All context the successor needs."}
            },
            "required":["profile","brief"],
            "additionalProperties":false
        })
    }

    async fn call(&self, args: HandoffArgs) -> Result<String, HandoffError> {
        self.profiles.get(&args.profile).map_err(HandoffError)?;
        if args.brief.trim().is_empty() {
            return Err(HandoffError("brief is required".into()));
        }
        let target = args.profile.clone();
        self.pending.set(Handoff {
            to: args.profile,
            brief: args.brief,
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

    #[tokio::test]
    async fn same_profile_is_allowed_and_payload_is_only_brief() {
        let (pending, tool) = tool("default");
        tool.call(HandoffArgs {
            profile: "default".into(),
            brief: "continue here".into(),
        })
        .await
        .unwrap();
        let handoff = pending.take().unwrap();
        assert_eq!(handoff.to, "default");
        assert_eq!(handoff.brief, "continue here");
    }

    #[test]
    fn schema_is_exact() {
        let (_, tool) = tool("default");
        let schema = tool.parameters();
        let props = schema["properties"].as_object().unwrap();
        assert_eq!(props.len(), 2);
        assert!(props.contains_key("profile"));
        assert!(props.contains_key("brief"));
        assert_eq!(schema["required"], json!(["profile", "brief"]));
    }
}
