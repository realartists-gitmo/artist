use artist_kernel::Kernel;
use rig_agent::tool::{DynamicTool, IntoToolOutput, ToolExecutionError};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

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
                definition_parameters(definition.parameters),
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
                        let arguments =
                            dynamic_from_json(arguments).map_err(ToolExecutionError::other)?;
                        let output = kernel
                            .execute_tool_with_scope(&name, arguments, scope)
                            .await
                            .map_err(|error| ToolExecutionError::other(error.to_string()))?;
                        dynamic_to_json(output).into_tool_output()
                    })
                },
            )
        })
        .collect()
}

fn definition_parameters(parameters: artist_kernel::DynamicValue) -> Value {
    dynamic_to_json(parameters)
}

fn dynamic_from_json(value: Value) -> Result<artist_kernel::DynamicValue, String> {
    Ok(match value {
        Value::Null => artist_kernel::DynamicValue::Option(None),
        Value::Bool(value) => artist_kernel::DynamicValue::Bool(value),
        Value::Number(value) => artist_kernel::DynamicValue::U64(
            value
                .as_u64()
                .ok_or_else(|| "model number must be a non-negative integer".to_owned())?,
        ),
        Value::String(value) => artist_kernel::DynamicValue::String(value),
        Value::Array(values) => artist_kernel::DynamicValue::List(
            values
                .into_iter()
                .map(dynamic_from_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Object(values) => artist_kernel::DynamicValue::Record(
            values
                .into_iter()
                .map(|(name, value)| Ok((name, dynamic_from_json(value)?)))
                .collect::<Result<BTreeMap<_, _>, String>>()?,
        ),
    })
}

fn dynamic_to_json(value: artist_kernel::DynamicValue) -> Value {
    use artist_kernel::DynamicValue;
    match value {
        DynamicValue::Bool(value) => Value::Bool(value),
        DynamicValue::S8(value) => Value::from(value),
        DynamicValue::S16(value) => Value::from(value),
        DynamicValue::S32(value) => Value::from(value),
        DynamicValue::S64(value) => Value::from(value),
        DynamicValue::U8(value) => Value::from(value),
        DynamicValue::U16(value) => Value::from(value),
        DynamicValue::U32(value) => Value::from(value),
        DynamicValue::U64(value) => Value::from(value),
        DynamicValue::F32(value) => Value::from(value),
        DynamicValue::F64(value) => Value::from(value),
        DynamicValue::Char(value) => Value::from(value.to_string()),
        DynamicValue::String(value) => Value::String(value),
        DynamicValue::ResourceUri(value) => Value::String(value.to_string()),
        DynamicValue::List(values) | DynamicValue::Tuple(values) => {
            Value::Array(values.into_iter().map(dynamic_to_json).collect())
        }
        DynamicValue::Record(values) => Value::Object(
            values
                .into_iter()
                .map(|(name, value)| (name, dynamic_to_json(value)))
                .collect::<Map<_, _>>(),
        ),
        DynamicValue::Option(None) => Value::Null,
        DynamicValue::Option(Some(value)) => dynamic_to_json(*value),
        DynamicValue::Result(Ok(value)) => dynamic_to_json(*value),
        DynamicValue::Result(Err(value)) => dynamic_to_json(*value),
        DynamicValue::Enum(value) => Value::String(value),
        DynamicValue::Variant(name, None) => Value::String(name),
        DynamicValue::Variant(name, Some(value)) => {
            Value::Object(Map::from_iter([(name, dynamic_to_json(*value))]))
        }
        DynamicValue::Flags(values) => {
            Value::Array(values.into_iter().map(Value::String).collect())
        }
    }
}
