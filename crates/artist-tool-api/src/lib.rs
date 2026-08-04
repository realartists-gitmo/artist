//! Artist-owned tool contracts layered over Rig's portable execution API.
//!
//! Rig deliberately models only provider-facing input definitions and canonical
//! model content. Artist also needs output schemas, routing metadata, safety
//! annotations, and lossless structured results for MCP and canvas surfaces.

use std::sync::Arc;

use rig_core::tool::{
    IntoToolOutput, PortableDynamicTool, PortableTool, ToolErrorKind, ToolExecutionError,
    ToolOutput,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArtistWarning {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NextAction {
    Retry {
        #[serde(skip_serializing_if = "Option::is_none")]
        after_ms: Option<u64>,
    },
    RetryWith {
        tool: String,
        arguments: Value,
        reason: String,
    },
    StartBackground {
        tool: String,
        arguments: Value,
    },
    RecoverOperation {
        key: String,
    },
    ReadPage {
        cursor: String,
    },
    RestartPagination {
        tool: String,
        arguments: Value,
    },
    SelectWorkspace {
        path: String,
    },
    EnableCapability {
        capability: String,
        flag: String,
    },
    UseNewIdempotencyKey,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArtistFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_errors: Vec<FieldError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_data: Option<Value>,
}

impl ArtistFailure {
    pub fn from_tool_error(error: &ToolExecutionError) -> Self {
        Self {
            code: error
                .code()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| error.kind().as_str().to_owned()),
            message: error
                .model_feedback()
                .unwrap_or_else(|| error.message())
                .to_owned(),
            retryable: error.retryable().unwrap_or(matches!(
                error.kind(),
                ToolErrorKind::Timeout | ToolErrorKind::RateLimited | ToolErrorKind::Network
            )),
            retry_after_ms: None,
            field_errors: Vec::new(),
            partial_data: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResultMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub cursor: String,
    pub preview: String,
    pub returned_bytes: usize,
    pub total_bytes: usize,
    pub has_more: bool,
    pub content_type: String,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub progress: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

trait ProgressCallback:
    Fn(ProgressEvent) + rig_core::wasm_compat::WasmCompatSend + rig_core::wasm_compat::WasmCompatSync
{
}
impl<F> ProgressCallback for F where
    F: Fn(ProgressEvent)
        + rig_core::wasm_compat::WasmCompatSend
        + rig_core::wasm_compat::WasmCompatSync
{
}

#[derive(Clone, Default)]
pub struct ProgressReporter {
    callback: Option<Arc<dyn ProgressCallback>>,
}

impl std::fmt::Debug for ProgressReporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressReporter")
            .field("enabled", &self.callback.is_some())
            .finish()
    }
}

impl ProgressReporter {
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn(ProgressEvent)
            + rig_core::wasm_compat::WasmCompatSend
            + rig_core::wasm_compat::WasmCompatSync
            + 'static,
    {
        Self {
            callback: Some(Arc::new(callback)),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.callback.is_some()
    }

    pub fn emit(&self, event: ProgressEvent) {
        if let Some(callback) = &self.callback {
            callback(event);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolCallContext {
    pub operation_id: Option<String>,
    pub progress: ProgressReporter,
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
        ToolCallContext,
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
            ToolCallContext,
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
        Self::new_with_context(definition, move |arguments, _context| callback(arguments))
    }

    pub fn new_with_context<F>(definition: ArtistToolDefinition, callback: F) -> Self
    where
        F: Fn(
                Value,
                ToolCallContext,
            ) -> rig_core::wasm_compat::WasmBoxedFuture<
                'static,
                Result<ArtistToolOutput, ToolExecutionError>,
            > + rig_core::wasm_compat::WasmCompatSend
            + rig_core::wasm_compat::WasmCompatSync
            + 'static,
    {
        Self {
            definition: enveloped_definition(definition),
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
        self.execute_with_context(arguments, ToolCallContext::default())
            .await
    }

    pub async fn execute_with_context(
        &self,
        arguments: Value,
        context: ToolCallContext,
    ) -> Result<ArtistToolOutput, ToolExecutionError> {
        let mut output = (self.callback)(arguments, context).await?;
        if !is_result_envelope(&output.structured) {
            output.structured = success_envelope(output.structured);
        }
        Ok(output)
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
        self.with_context_callback(move |arguments, _context| callback(arguments))
    }

    pub fn with_context_callback<F>(&self, callback: F) -> Self
    where
        F: Fn(
                Value,
                ToolCallContext,
            ) -> rig_core::wasm_compat::WasmBoxedFuture<
                'static,
                Result<ArtistToolOutput, ToolExecutionError>,
            > + rig_core::wasm_compat::WasmCompatSend
            + rig_core::wasm_compat::WasmCompatSync
            + 'static,
    {
        Self {
            definition: self.definition.clone(),
            callback: Arc::new(callback),
        }
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
    ArtistDynamicTool::new_with_context(definition, move |arguments, context| {
        let tool = Arc::clone(&tool);
        Box::pin(async move {
            context.progress.emit(ProgressEvent {
                progress: 0.0,
                total: None,
                phase: "executing".into(),
                message: Some(format!("Executing {}.", T::NAME)),
                unit: Some("phase".into()),
            });
            let arguments =
                serde_json::from_value(arguments).map_err(ToolExecutionError::from_error)?;
            let raw = tool
                .call(arguments)
                .await
                .map_err(|error| tool.map_error(error))?;
            context.progress.emit(ProgressEvent {
                progress: 0.0,
                total: None,
                phase: "serializing".into(),
                message: Some(format!("Preparing {} result.", T::NAME)),
                unit: Some("phase".into()),
            });
            let structured = tool.structured_output(&raw)?;
            let presentation = raw.into_tool_output()?;
            Ok(ArtistToolOutput {
                presentation,
                structured,
            })
        })
    })
}

fn enveloped_definition(mut definition: ArtistToolDefinition) -> ArtistToolDefinition {
    if !is_result_schema(&definition.output_schema) {
        definition.output_schema = result_output_schema(definition.output_schema);
    }
    definition
}

pub fn is_result_envelope(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|object| object.get("ok"))
        .and_then(Value::as_bool)
        .is_some()
}

fn is_result_schema(value: &Value) -> bool {
    value
        .get("x-artist-envelope")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn success_envelope(data: Value) -> Value {
    json!({"ok": true, "data": data})
}

pub fn failure_envelope(
    failure: ArtistFailure,
    next_actions: Vec<NextAction>,
    meta: Option<ResultMeta>,
) -> Value {
    let mut value = json!({
        "ok": false,
        "error": failure,
    });
    let object = value
        .as_object_mut()
        .expect("failure envelope is an object");
    if !next_actions.is_empty() {
        object.insert(
            "nextActions".into(),
            serde_json::to_value(next_actions).expect("next actions serialize"),
        );
    }
    if let Some(meta) = meta {
        object.insert(
            "meta".into(),
            serde_json::to_value(meta).expect("result metadata serializes"),
        );
    }
    value
}

pub fn set_result_meta(value: &mut Value, meta: ResultMeta) {
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "meta".into(),
            serde_json::to_value(meta).expect("result metadata serializes"),
        );
    }
}

pub fn set_page(value: &mut Value, page: PageInfo) {
    if let Some(object) = value.as_object_mut() {
        object.insert("data".into(), Value::Null);
        object.insert(
            "page".into(),
            serde_json::to_value(&page).expect("page metadata serializes"),
        );
        object.insert(
            "nextActions".into(),
            serde_json::to_value(vec![NextAction::ReadPage {
                cursor: page.cursor,
            }])
            .expect("page action serializes"),
        );
    }
}

pub fn result_output_schema(mut data_schema: Value) -> Value {
    let warning_schema = json!({
        "type": "object",
        "properties": {
            "code": {"type": "string"},
            "message": {"type": "string"}
        },
        "required": ["code", "message"],
        "additionalProperties": false
    });
    let field_error_schema = json!({
        "type": "object",
        "properties": {
            "field": {"type": "string"},
            "message": {"type": "string"}
        },
        "required": ["field", "message"],
        "additionalProperties": false
    });
    let next_action_schema = json!({
        "type": "object",
        "required": ["kind"],
        "properties": {"kind": {"type": "string"}},
        "additionalProperties": true
    });
    let meta_schema = json!({
        "type": "object",
        "properties": {
            "operationId": {"type": "string"},
            "durationMs": {"type": "integer", "minimum": 0}
        },
        "additionalProperties": false
    });
    let page_schema = json!({
        "type": "object",
        "properties": {
            "cursor": {"type": "string"},
            "preview": {"type": "string"},
            "returnedBytes": {"type": "integer", "minimum": 0},
            "totalBytes": {"type": "integer", "minimum": 0},
            "hasMore": {"type": "boolean"},
            "contentType": {"type": "string"},
            "summary": {"type": "string"}
        },
        "required": ["cursor", "preview", "returnedBytes", "totalBytes", "hasMore", "contentType", "summary"],
        "additionalProperties": false
    });
    let definitions = data_schema
        .as_object_mut()
        .and_then(|object| object.remove("definitions"));
    let defs = data_schema
        .as_object_mut()
        .and_then(|object| object.remove("$defs"));

    let failure_schema = json!({
        "type": "object",
        "properties": {
            "code": {"type": "string"},
            "message": {"type": "string"},
            "retryable": {"type": "boolean"},
            "retryAfterMs": {"type": "integer", "minimum": 0},
            "fieldErrors": {"type": "array", "items": field_error_schema},
            "partialData": {}
        },
        "required": ["code", "message", "retryable"],
        "additionalProperties": false
    });
    let mut schema = json!({
        "x-artist-envelope": true,
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "ok": {"const": true},
                    "data": {"anyOf": [data_schema, {"type": "null"}]},
                    "warnings": {"type": "array", "items": warning_schema},
                    "nextActions": {"type": "array", "items": next_action_schema},
                    "meta": meta_schema,
                    "page": page_schema
                },
                "required": ["ok", "data"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "ok": {"const": false},
                    "error": failure_schema,
                    "warnings": {"type": "array", "items": warning_schema},
                    "nextActions": {"type": "array", "items": next_action_schema},
                    "meta": meta_schema
                },
                "required": ["ok", "error"],
                "additionalProperties": false
            }
        ]
    });
    let root = schema.as_object_mut().expect("result schema is an object");
    if let Some(defs) = defs {
        root.insert("$defs".into(), defs);
    }
    if let Some(definitions) = definitions {
        root.insert("definitions".into(), definitions);
    }
    schema
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
