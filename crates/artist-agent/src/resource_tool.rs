use artist_kernel::Kernel;
use rig_agent::tool::{DynamicTool, IntoToolOutput, ToolExecutionError};

/// Build the model-facing named tools from the live kernel catalog.
/// `tools://` remains the source/configuration namespace; these dynamic tools
/// are the only execution surface advertised to the model.
pub async fn named_tools(kernel: Kernel) -> Vec<DynamicTool> {
    kernel
        .tool_definitions()
        .await
        .into_iter()
        .map(|definition| {
            let name = definition.name.clone();
            let callback_kernel = kernel.clone();
            DynamicTool::new(
                definition.name,
                definition.description,
                definition.parameters,
                move |_, arguments| {
                    let kernel = callback_kernel.clone();
                    let name = name.clone();
                    Box::pin(async move {
                        kernel
                            .execute_tool(&name, arguments)
                            .await
                            .map_err(|error| ToolExecutionError::other(error.to_string()))?
                            .into_tool_output()
                    })
                },
            )
        })
        .collect()
}
