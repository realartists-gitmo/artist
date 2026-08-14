use artist_kernel::Kernel;
use rig_agent::tool::{DynamicTool, IntoToolOutput, ToolExecutionError};

/// Optional invocation metadata injected by an embedding runtime into
/// `rig_agent::tool::ToolContext`. The model-facing adapter remains portable,
/// while hosts that have cancellation/deadline/correlation data can carry it
/// into the kernel instead of silently dropping it.
#[derive(Clone)]
pub struct ArtistToolContext(pub artist_kernel::InvocationContext);

/// Build the model-facing named tools from the live kernel catalog.
/// `tools://` remains the source/configuration namespace; these dynamic tools
/// are the only execution surface advertised to the model.
pub async fn named_tools(
    kernel: Kernel,
    context: artist_kernel::InvocationContext,
    cancellation: tokio_util::sync::CancellationToken,
) -> Vec<DynamicTool> {
    kernel
        .tool_definitions()
        .await
        .into_iter()
        .map(|definition| {
            let name = definition.name.clone();
            let callback_kernel = kernel.clone();
            let callback_context = context.clone();
            let callback_cancellation = cancellation.clone();
            DynamicTool::new(
                definition.name,
                definition.description,
                definition.parameters,
                move |tool_context, arguments| {
                    let kernel = callback_kernel.clone();
                    let name = name.clone();
                    let cancellation = callback_cancellation.clone();
                    let context = tool_context
                        .get::<ArtistToolContext>()
                        .map(|value| value.0.clone())
                        .unwrap_or_else(|| callback_context.clone());
                    Box::pin(async move {
                        let scope = artist_kernel::InvocationScope::with_cancellation(
                            context,
                            cancellation,
                        );
                        kernel
                            .execute_tool_with_scope(&name, arguments, scope)
                            .await
                            .map_err(|error| ToolExecutionError::other(error.to_string()))?
                            .into_tool_output()
                    })
                },
            )
        })
        .collect()
}
