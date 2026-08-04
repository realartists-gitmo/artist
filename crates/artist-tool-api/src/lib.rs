//! Artist-owned tool contracts layered over Rig's portable execution API.
//!
//! Rig deliberately models only provider-facing input definitions and canonical
//! model content. Artist also needs output schemas, routing metadata, safety
//! annotations, and lossless structured results for MCP and canvas surfaces.

use std::sync::Arc;

use rig_core::tool::{
    IntoToolOutput, PortableDynamicTool, PortableTool, ToolExecutionError, ToolOutput,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    Shell,
    Files,
    Code,
    Memory,
    Computer,
    Canvas,
    Agents,
    UserInteraction,
    Administration,
    External,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArtistToolAnnotations {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

impl ArtistToolAnnotations {
    pub const fn read_only() -> Self {
        Self {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        }
    }

    pub const fn mutating() -> Self {
        Self {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: false,
        }
    }

    pub const fn external_read() -> Self {
        Self {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ArtistToolDefinition {
    pub name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub category: ToolCategory,
    pub annotations: ArtistToolAnnotations,
}

#[derive(Clone, Debug)]
pub struct ArtistToolOutput {
    pub presentation: ToolOutput,
    pub structured: Value,
}

impl ArtistToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            presentation: ToolOutput::text(text.clone()),
            structured: json!({"text": text}),
        }
    }

    pub fn from_tool_output(output: ToolOutput) -> Result<Self, ToolExecutionError> {
        let structured = match output.as_json() {
            Some(Value::Object(_)) => output.as_json().cloned().expect("checked"),
            Some(value) => json!({"value": value}),
            None => json!({"text": output.render()}),
        };
        Ok(Self {
            presentation: output,
            structured,
        })
    }
}

trait Callback:
    Fn(
        Value,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<
        'static,
        Result<ArtistToolOutput, ToolExecutionError>,
    > + rig_core::wasm_compat::WasmCompatSend
    + rig_core::wasm_compat::WasmCompatSync
{
}

impl<F> Callback for F where
    F: Fn(
            Value,
        ) -> rig_core::wasm_compat::WasmBoxedFuture<
            'static,
            Result<ArtistToolOutput, ToolExecutionError>,
        > + rig_core::wasm_compat::WasmCompatSend
        + rig_core::wasm_compat::WasmCompatSync
{
}
#[derive(Clone)]
pub struct ArtistDynamicTool {
    definition: ArtistToolDefinition,
    callback: Arc<dyn Callback>,
}

impl std::fmt::Debug for ArtistDynamicTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtistDynamicTool")
            .field("definition", &self.definition)
            .finish_non_exhaustive()
    }
}

impl ArtistDynamicTool {
    pub fn new<F>(definition: ArtistToolDefinition, callback: F) -> Self
    where
        F: Fn(
                Value,
            ) -> rig_core::wasm_compat::WasmBoxedFuture<
                'static,
                Result<ArtistToolOutput, ToolExecutionError>,
            > + rig_core::wasm_compat::WasmCompatSend
            + rig_core::wasm_compat::WasmCompatSync
            + 'static,
    {
        Self {
            definition,
            callback: Arc::new(callback),
        }
    }

    pub fn from_portable(
        tool: PortableDynamicTool,
        output_schema: Value,
        category: ToolCategory,
        annotations: ArtistToolAnnotations,
    ) -> Self {
        let portable_definition = tool.definition();
        let definition = ArtistToolDefinition {
            title: humanize(&portable_definition.name),
            name: portable_definition.name,
            description: portable_definition.description,
            input_schema: portable_definition.parameters,
            output_schema,
            category,
            annotations,
        };
        let tool = Arc::new(tool);
        Self::new(definition, move |arguments| {
            let tool = Arc::clone(&tool);
            Box::pin(async move {
                let output = tool.execute(arguments).await?;
                ArtistToolOutput::from_tool_output(output)
            })
        })
    }
    pub fn name(&self) -> &str {
        &self.definition.name
    }
    pub fn definition(&self) -> &ArtistToolDefinition {
        &self.definition
    }

    pub async fn execute(&self, arguments: Value) -> Result<ArtistToolOutput, ToolExecutionError> {
        (self.callback)(arguments).await
    }

    pub fn portable(&self) -> PortableDynamicTool {
        let definition = self.definition.clone();
        let tool = self.clone();
        PortableDynamicTool::new(
            definition.name,
            definition.description,
            definition.input_schema,
            move |arguments| {
                let tool = tool.clone();
                Box::pin(async move {
                    tool.execute(arguments)
                        .await
                        .map(|output| output.presentation)
                })
            },
        )
    }

    pub fn with_callback<F>(&self, callback: F) -> Self
    where
        F: Fn(
                Value,
            ) -> rig_core::wasm_compat::WasmBoxedFuture<
                'static,
                Result<ArtistToolOutput, ToolExecutionError>,
            > + rig_core::wasm_compat::WasmCompatSend
            + rig_core::wasm_compat::WasmCompatSync
            + 'static,
    {
        Self::new(self.definition.clone(), callback)
    }
}

/// Metadata and structured-output conversion co-located with a portable tool.
pub trait ArtistToolContract: PortableTool {
    fn title(&self) -> String {
        humanize(Self::NAME)
    }
    fn category(&self) -> ToolCategory;
    fn annotations(&self) -> ArtistToolAnnotations;

    /// Root-object schema required by MCP outputSchema.
    fn output_schema(&self) -> Value {
        text_output_schema(Self::NAME, &format!("Result returned by {}.", Self::NAME))
    }

    /// Produce structured output from the tool's actual Rust value before Rig
    /// converts it into model presentation. This prevents adapters from
    /// guessing that text happens to contain JSON.
    fn structured_output(&self, output: &Self::Output) -> Result<Value, ToolExecutionError>;
}

pub fn dynamic<T>(tool: T) -> ArtistDynamicTool
where
    T: ArtistToolContract + 'static,
{
    let definition = ArtistToolDefinition {
        name: T::NAME.to_owned(),
        title: tool.title(),
        description: tool.description(),
        input_schema: tool.parameters(),
        output_schema: tool.output_schema(),
        category: tool.category(),
        annotations: tool.annotations(),
    };
    let tool = Arc::new(tool);
    ArtistDynamicTool::new(definition, move |arguments| {
        let tool = Arc::clone(&tool);
        Box::pin(async move {
            let arguments =
                serde_json::from_value(arguments).map_err(ToolExecutionError::from_error)?;
            let raw = tool
                .call(arguments)
                .await
                .map_err(|error| tool.map_error(error))?;
            let structured = tool.structured_output(&raw)?;
            let presentation = raw.into_tool_output()?;
            Ok(ArtistToolOutput {
                presentation,
                structured,
            })
        })
    })
}

pub fn schema_for<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("JSON Schema serializes")
}

pub fn text_output_schema(title: &str, description: &str) -> Value {
    json!({
        "type": "object",
        "title": humanize(title),
        "description": description,
        "properties": {
            "text": {"type": "string", "description": description}
        },
        "required": ["text"],
        "additionalProperties": false
    })
}

pub fn humanize(name: &str) -> String {
    let mut words = name.split('_').filter(|word| !word.is_empty());
    let Some(first) = words.next() else {
        return String::new();
    };
    let mut title = capitalize(first);
    for word in words {
        title.push(' ');
        title.push_str(word);
    }
    title
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Implement the common text-result contract beside a tool implementation.
#[macro_export]
macro_rules! impl_text_tool_contract {
    ($ty:ty, $category:expr, $annotations:expr, $description:expr) => {
        impl $crate::ArtistToolContract for $ty {
            fn category(&self) -> $crate::ToolCategory {
                $category
            }
            fn annotations(&self) -> $crate::ArtistToolAnnotations {
                $annotations
            }
            fn output_schema(&self) -> serde_json::Value {
                $crate::text_output_schema(
                    <Self as rig_core::tool::PortableTool>::NAME,
                    $description,
                )
            }
            fn structured_output(
                &self,
                output: &<Self as rig_core::tool::PortableTool>::Output,
            ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
                Ok(serde_json::json!({"text": output}))
            }
        }
    };
}

/// Implement a contract for tools whose portable output is already canonical
/// [`ToolOutput`]. Native JSON remains structured and text remains text.
#[macro_export]
macro_rules! impl_tool_output_contract {
    ($ty:ty, $category:expr, $annotations:expr, $description:expr) => {
        impl $crate::ArtistToolContract for $ty {
            fn category(&self) -> $crate::ToolCategory { $category }
            fn annotations(&self) -> $crate::ArtistToolAnnotations { $annotations }
            fn output_schema(&self) -> serde_json::Value {
                $crate::text_output_schema(
                    <Self as rig_core::tool::PortableTool>::NAME,
                    $description,
                )
            }
            fn structured_output(
                &self,
                output: &<Self as rig_core::tool::PortableTool>::Output,
            ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
                match output.as_json() {
                    Some(serde_json::Value::Object(map)) => {
                        Ok(serde_json::Value::Object(map.clone()))
                    }
                    Some(value) => Ok(serde_json::json!({"value": value})),
                    None => Ok(serde_json::json!({"text": output.render()})),
                }
            }
        }
    };
}

/// Implement a contract whose portable output is a serializable DTO.
#[macro_export]
macro_rules! impl_serialized_tool_contract {
    ($ty:ty, $category:expr, $annotations:expr) => {
        impl $crate::ArtistToolContract for $ty {
            fn category(&self) -> $crate::ToolCategory {
                $category
            }
            fn annotations(&self) -> $crate::ArtistToolAnnotations {
                $annotations
            }
            fn output_schema(&self) -> serde_json::Value {
                $crate::schema_for::<<Self as rig_core::tool::PortableTool>::Output>()
            }
            fn structured_output(
                &self,
                output: &<Self as rig_core::tool::PortableTool>::Output,
            ) -> Result<serde_json::Value, rig_core::tool::ToolExecutionError> {
                serde_json::to_value(output).map_err(rig_core::tool::ToolExecutionError::from_error)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_never_reparsed_as_json() {
        let output = ToolOutput::text(r#"{"delivered":2}"#);
        let structured = ArtistToolOutput::from_tool_output(output)
            .unwrap()
            .structured;
        assert_eq!(structured, json!({"text": r#"{"delivered":2}"#}));
    }

    #[test]
    fn native_json_is_preserved() {
        let output = ToolOutput::json(json!({"delivered": 2}));
        let structured = ArtistToolOutput::from_tool_output(output)
            .unwrap()
            .structured;
        assert_eq!(structured, json!({"delivered": 2}));
    }
}
