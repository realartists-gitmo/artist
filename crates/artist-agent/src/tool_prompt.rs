//! System-prompt projection of the exact tools registered for a run.

use rig_core::tool::{IntoToolOutput, PortableDynamicTool, PortableTool};
use std::{collections::HashSet, sync::Arc};

/// Erase a typed portable tool into Rig's runtime-authored portable contract.
pub(crate) fn dynamic<T>(tool: T) -> PortableDynamicTool
where
    T: PortableTool + 'static,
{
    let name = T::NAME;
    let description = tool.description();
    let parameters = tool.parameters();
    let tool = Arc::new(tool);
    PortableDynamicTool::new(name, description, parameters, move |arguments| {
        let tool = Arc::clone(&tool);
        Box::pin(async move {
            let arguments = serde_json::from_value(arguments)
                .map_err(rig_core::tool::ToolExecutionError::from_error)?;
            tool.call(arguments)
                .await
                .map_err(|error| tool.map_error(error))?
                .into_tool_output()
        })
    })
}

pub(crate) fn retain_enabled(tools: &mut Vec<PortableDynamicTool>, disabled: &[String]) {
    tools.retain(|tool| !disabled.iter().any(|name| name == tool.name()));
}

pub(crate) fn render(tools: &[PortableDynamicTool]) -> String {
    if tools.is_empty() {
        return "No tools are available for this run.".to_owned();
    }
    let mut output = String::from("Available tools:\n");
    let mut names = HashSet::new();
    for tool in tools {
        let definition = tool.definition();
        names.insert(definition.name.clone());
        output.push_str(&format!(
            "- `{}`: {}\n",
            definition.name,
            one_line(&definition.description)
        ));
    }
    let mut guidance = Vec::new();
    if names.contains("find") {
        guidance.push("Use `find` for project file/path discovery, listings, and glob filtering.");
    }
    if names.contains("grep") {
        guidance.push("Use `grep` for project content searches.");
    }
    if names.contains("read") {
        guidance.push("Use `read` to inspect files before making targeted edits.");
    }
    if names.contains("edit") {
        guidance.push("Use `edit` with mnemonic anchors from the latest `read`; never use line numbers. Re-read after stale or unknown anchors.");
    }
    if names.contains("write") {
        guidance.push("Use `write` only for new files or intentional complete-file replacement.");
    }
    if names.contains("bash") {
        guidance.push("Use `bash` for tests, builds, diagnostics, package commands, and persistent development servers.");
        if names.contains("find") || names.contains("grep") || names.contains("read") {
            guidance.push("Prefer the available `find`, `grep`, and `read` tools over equivalent shell discovery or content-search commands.");
        }
        guidance.push("For independent long-running commands, use background mode, continue useful work, then read or stop the session without polling repeatedly.");
    }
    if names.contains("subagent") {
        guidance.push("Use `subagent` for focused work that benefits from a separate agent. Collect or cancel every background subagent before finishing.");
    }
    if !guidance.is_empty() {
        output.push_str("\nTool-specific guidelines:\n");
        for line in guidance {
            output.push_str("- ");
            output.push_str(line);
            output.push('\n');
        }
    }
    output.trim_end().to_owned()
}

fn one_line(description: &str) -> String {
    description.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct Stub(&'static str);
    #[derive(Debug, thiserror::Error)]
    #[error("stub")]
    struct Error;
    impl PortableTool for Stub {
        const NAME: &'static str = "read";
        type Args = serde_json::Value;
        type Output = String;
        type Error = Error;
        fn description(&self) -> String {
            self.0.into()
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }
        async fn call(&self, _: Self::Args) -> Result<String, Error> {
            Ok(String::new())
        }
    }
    #[test]
    fn renders_and_filters() {
        let mut tools = vec![dynamic(Stub("Inspect\nfiles"))];
        assert!(render(&tools).contains("- `read`: Inspect files"));
        retain_enabled(&mut tools, &["read".into()]);
        assert_eq!(render(&tools), "No tools are available for this run.");
    }
}
