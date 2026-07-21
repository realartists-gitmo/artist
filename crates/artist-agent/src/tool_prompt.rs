//! System-prompt projection of the exact tools registered for a run.

use std::collections::HashSet;

use rig_core::tool::ToolDyn;

/// Apply the same deny-list to provider registration and prompt projection.
pub(crate) fn retain_enabled(tools: &mut Vec<Box<dyn ToolDyn>>, disabled: &[String]) {
    tools.retain(|tool| !disabled.iter().any(|name| name == &tool.name()));
}

/// Render model-facing descriptions and conditional usage guidance from the
/// final registered list. MCP and extension tools naturally participate via
/// `ToolDyn::description`; guidance never names a disabled built-in.
pub(crate) fn render(tools: &[Box<dyn ToolDyn>]) -> String {
    if tools.is_empty() {
        return "No tools are available for this run.".to_owned();
    }
    let mut output = String::from("Available tools:\n");
    let mut names = HashSet::new();
    for tool in tools {
        let name = tool.name();
        names.insert(name.clone());
        output.push_str(&format!("- `{name}`: {}\n", one_line(&tool.description())));
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
    if names.contains("delegate") {
        guidance.push("Use `delegate` for focused work that benefits from a subagent. Collect or cancel every background delegate before finishing.");
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
    use rig_core::tool::{ToolError, ToolExecutionResult};

    struct Stub(&'static str, &'static str);

    impl ToolDyn for Stub {
        fn name(&self) -> String {
            self.0.into()
        }
        fn description(&self) -> String {
            self.1.into()
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }
        fn call<'a>(
            &'a self,
            _args: String,
        ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<String, ToolError>> {
            Box::pin(async { Ok(String::new()) })
        }
        fn call_structured<'a>(
            &'a self,
            _args: String,
            _extensions: &'a rig_core::tool::ToolCallExtensions,
        ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, ToolExecutionResult> {
            Box::pin(async { ToolExecutionResult::success(String::new()) })
        }
    }

    fn tools(values: &[(&'static str, &'static str)]) -> Vec<Box<dyn ToolDyn>> {
        values
            .iter()
            .map(|(name, description)| Box::new(Stub(name, description)) as Box<dyn ToolDyn>)
            .collect()
    }

    #[test]
    fn renders_dynamic_tools_and_only_applicable_guidance() {
        let tools = tools(&[("read", "Inspect\nfiles"), ("custom", "Extension action")]);
        let prompt = render(&tools);
        assert!(prompt.contains("- `read`: Inspect files"));
        assert!(prompt.contains("- `custom`: Extension action"));
        assert!(prompt.contains("Use `read`"));
        assert!(!prompt.contains("Use `grep`"));
        assert!(!prompt.contains("Use `bash`"));
    }

    #[test]
    fn filtering_drives_the_same_projected_list() {
        let mut tools = tools(&[("read", "Read"), ("custom", "Custom")]);
        retain_enabled(&mut tools, &["read".into()]);
        let prompt = render(&tools);
        assert!(!prompt.contains("`read`"));
        assert!(prompt.contains("`custom`"));
    }
}
