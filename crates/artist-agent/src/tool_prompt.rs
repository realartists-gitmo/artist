//! Erasure and filtering for the tools registered on a run.
//!
//! Tool definitions reach the model through the API's `tools` field, not the
//! system prompt: that is the provider-native channel, it is the single source
//! of truth for name, description, and schema, and it works identically for
//! built-in, MCP, and extension tools. Per-tool usage guidance therefore lives
//! in each tool's own `description`.

use rig_core::tool::{IntoToolOutput, PortableDynamicTool, PortableTool};
use std::sync::Arc;

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
    fn disabled_tools_are_dropped() {
        let mut tools = vec![dynamic(Stub("Inspect files"))];
        assert_eq!(tools.len(), 1);
        retain_enabled(&mut tools, &["read".into()]);
        assert!(tools.is_empty());
    }
}
