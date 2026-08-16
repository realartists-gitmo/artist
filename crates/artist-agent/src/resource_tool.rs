use artist_kernel::{DynamicType, DynamicValue, Kernel};
use artist_session::ToolOutcomeRecord;
use futures::{StreamExt, future::join_all};
use rig_agent::{
    agent::{
        hook::InvalidToolCallAction,
        run::{AgentRun, AgentRunStep, ModelTurn},
    },
    tool::{DynamicTool, IntoToolOutput, ToolExecutionError},
};
use rig_core::{
    OneOrMany,
    completion::{CompletionModel, Message},
    message::{AssistantContent, ReasoningContent, ToolResult, ToolResultContent, UserContent},
    streaming::StreamedAssistantContent,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

#[derive(Debug)]
pub struct BatchedRunError {
    pub message: String,
    pub messages: Vec<Message>,
}

impl std::fmt::Display for BatchedRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl From<String> for BatchedRunError {
    fn from(message: String) -> Self {
        Self {
            message,
            messages: Vec::new(),
        }
    }
}

/// Optional invocation metadata injected by an embedding runtime into
/// `rig_agent::tool::ToolContext`. The model-facing adapter remains portable,
/// while hosts that have cancellation/deadline/correlation data can carry it
/// into the kernel instead of silently dropping it.
#[derive(Clone)]
pub struct ArtistToolContext(pub artist_kernel::InvocationContext);

#[derive(Clone, Debug)]
pub struct SiblingToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug)]
pub(crate) enum BatchedRunEvent {
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
    ToolExecutionStart {
        id: String,
        name: String,
    },
    ToolResult {
        id: String,
        content: String,
        outcome: ToolOutcomeRecord,
        duration_ms: u64,
    },
    Reasoning(String),
    Text(String),
    CompletionUsage(u64),
}

/// Artist-owned sans-IO Rig driver. `AgentRun` exposes the complete
/// `CallTools` set before any tool is dispatched; this driver is the concrete
/// integration point that preserves that set, performs one grouped kernel
/// batch per tool identity, and returns results in Rig's original call order.
pub async fn run_batched_agent<M>(
    model: M,
    prompt: Message,
    history: Vec<Message>,
    preamble: Option<String>,
    tools: Vec<DynamicTool>,
    kernel: Kernel,
    context: artist_kernel::InvocationContext,
    cancellation: tokio_util::sync::CancellationToken,
    additional_params: Option<Value>,
    mut take_steering: impl FnMut() -> Vec<String>,
    mut on_event: impl FnMut(BatchedRunEvent) -> Result<(), String>,
) -> Result<(String, Vec<Message>), BatchedRunError>
where
    M: CompletionModel + 'static,
{
    let definitions = tools
        .iter()
        .map(DynamicTool::definition)
        .collect::<Vec<_>>();
    let executable_names: BTreeSet<String> = definitions
        .iter()
        .map(|definition| definition.name.clone())
        .collect();
    let mut run = AgentRun::new(prompt)
        .with_history(history)
        .max_turns(usize::MAX);
    let mut tool_starts = BTreeMap::<String, Instant>::new();
    loop {
        if cancellation.is_cancelled() {
            return Err(BatchedRunError {
                message: "agent run aborted".to_owned(),
                messages: run.messages().to_vec(),
            });
        }
        match run.next_step().map_err(|error| error.to_string())? {
            AgentRunStep::CallModel {
                prompt, history, ..
            } => {
                let mut request = model
                    .completion_request(prompt)
                    .messages(history)
                    .tools(definitions.clone());
                if let Some(preamble) = preamble.clone() {
                    request = request.preamble(preamble);
                }
                request = request.additional_params_opt(additional_params.clone());
                let mut stream = model
                    .stream(request.build())
                    .await
                    .map_err(|error| error.to_string())?;
                let mut emitted_text = false;
                let mut emitted_reasoning = false;
                while let Some(item) = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        return Err(BatchedRunError {
                            message: "agent run aborted".to_owned(),
                            messages: run.messages().to_vec(),
                        });
                    },
                    item = stream.next() => item,
                } {
                    match item.map_err(|error| error.to_string())? {
                        StreamedAssistantContent::Text(text) => {
                            emitted_text = true;
                            on_event(BatchedRunEvent::Text(text.text))?;
                        }
                        StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                            emitted_reasoning = true;
                            on_event(BatchedRunEvent::Reasoning(reasoning))?;
                        }
                        _ => {}
                    }
                }
                let response: rig_core::completion::CompletionResponse<
                    Option<M::StreamingResponse>,
                > = stream.into();
                // Some providers expose the complete assistant choice only in
                // the terminal response rather than as stream deltas. Replay
                // that content exactly once so the custom driver has the same
                // observable text/reasoning lifecycle as Rig's normal stream
                // runner. Tool calls are replayed by AgentRun's CallTools
                // boundary below and must not be emitted twice here.
                for content in response.choice.iter() {
                    match content {
                        AssistantContent::Text(text) if !emitted_text => {
                            on_event(BatchedRunEvent::Text(text.text.clone()))?;
                        }
                        AssistantContent::Reasoning(reasoning) if !emitted_reasoning => {
                            let text = reasoning
                                .content
                                .iter()
                                .map(|part| match part {
                                    ReasoningContent::Text { text, .. }
                                    | ReasoningContent::Summary(text) => text.clone(),
                                    ReasoningContent::Encrypted(value)
                                    | ReasoningContent::Redacted { data: value } => value.clone(),
                                    _ => String::new(),
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            if !text.is_empty() {
                                on_event(BatchedRunEvent::Reasoning(text))?;
                            }
                        }
                        AssistantContent::Text(_) | AssistantContent::Reasoning(_) => {}
                        AssistantContent::ToolCall(_) | AssistantContent::Image(_) => {}
                    }
                }
                on_event(BatchedRunEvent::CompletionUsage(
                    response.usage.total_tokens,
                ))?;
                let outcome = run
                    .model_response(ModelTurn::new(
                        response.message_id,
                        response.choice,
                        response.usage,
                        executable_names.clone(),
                        executable_names.clone(),
                    ))
                    .map_err(|error| error.to_string())?;
                if matches!(
                    outcome,
                    rig_agent::agent::run::ModelTurnOutcome::NeedsResolution(_)
                ) {
                    run.resolve_invalid_tool_call(InvalidToolCallAction::skip(
                        "tool call was not present in the active typed tool catalog",
                    ))
                    .map_err(|error| error.to_string())?;
                }
            }
            AgentRunStep::CallTools { calls } => {
                let sibling_calls = calls
                    .iter()
                    .map(|call| SiblingToolCall {
                        id: call.tool_call.id.clone(),
                        name: call.tool_call.function.name.clone(),
                        arguments: call.tool_call.function.arguments.clone(),
                    })
                    .collect::<Vec<_>>();
                for call in &sibling_calls {
                    on_event(BatchedRunEvent::ToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    })?;
                    on_event(BatchedRunEvent::ToolExecutionStart {
                        id: call.id.clone(),
                        name: call.name.clone(),
                    })?;
                    tool_starts.insert(call.id.clone(), Instant::now());
                }
                let results = execute_sibling_calls(
                    kernel.clone(),
                    sibling_calls,
                    context.clone(),
                    cancellation.clone(),
                )
                .await;
                let tool_results = results
                    .into_iter()
                    .map(|(id, result)| -> Result<UserContent, String> {
                        let (content, outcome) = match result {
                            Ok((value, semantic_error)) => {
                                let outcome = if semantic_error {
                                    ToolOutcomeRecord::Error {
                                        kind: None,
                                        message: "tool returned a semantic error".to_owned(),
                                    }
                                } else {
                                    ToolOutcomeRecord::Success
                                };
                                (ToolResultContent::json(value), outcome)
                            }
                            Err(error) => (
                                ToolResultContent::json(
                                    serde_json::json!({ "error": error.clone() }),
                                ),
                                ToolOutcomeRecord::Error {
                                    kind: None,
                                    message: error,
                                },
                            ),
                        };
                        let content_text = match &content {
                            ToolResultContent::Json { value } => value.to_string(),
                            ToolResultContent::Text(text) => text.text.clone(),
                            ToolResultContent::Image(_) => "[image]".to_owned(),
                        };
                        let steering = take_steering();
                        let content_text = if steering.is_empty() {
                            content_text
                        } else {
                            format!(
                                "{}\n\n{}",
                                content_text,
                                steering
                                    .iter()
                                    .map(|message| format!(
                                        "<user_steering>\n{message}\n</user_steering>"
                                    ))
                                    .collect::<Vec<_>>()
                                    .join("\n\n")
                            )
                        };
                        on_event(BatchedRunEvent::ToolResult {
                            id: id.clone(),
                            content: content_text.clone(),
                            outcome,
                            duration_ms: tool_starts
                                .remove(&id)
                                .map(|start| start.elapsed().as_millis() as u64)
                                .unwrap_or(0),
                        })?;
                        Ok(UserContent::ToolResult(ToolResult {
                            id,
                            call_id: None,
                            content: OneOrMany::one(ToolResultContent::text(content_text)),
                        }))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                run.tool_results(tool_results)
                    .map_err(|error| error.to_string())?;
            }
            AgentRunStep::Done(response) => {
                return Ok((response.output, run.messages().to_vec()));
            }
        }
    }
}

/// The sans-I/O batch boundary used by an agent driver: normalize each flat
/// model call, group only by exact tool identity, execute each group once, and
/// restore source order. Item failures stay attached to their own call.
pub async fn execute_sibling_calls(
    kernel: Kernel,
    calls: Vec<SiblingToolCall>,
    context: artist_kernel::InvocationContext,
    cancellation: tokio_util::sync::CancellationToken,
) -> Vec<(String, Result<(Value, bool), String>)> {
    let definitions = kernel
        .tool_definitions()
        .await
        .into_iter()
        .map(|definition| (definition.name.clone(), definition.input_type))
        .collect::<BTreeMap<_, _>>();
    let mut grouped = BTreeMap::<String, Vec<(usize, DynamicValue)>>::new();
    let mut results = calls
        .iter()
        .map(|call| (call.id.clone(), None))
        .collect::<Vec<_>>();
    for (index, call) in calls.iter().enumerate() {
        let Some(Some(expected)) = definitions.get(&call.name) else {
            results[index].1 = Some(Err(format!("tool {} has no active input type", call.name)));
            continue;
        };
        match normalize_json(call.arguments.clone(), expected) {
            Ok(value) => grouped
                .entry(call.name.clone())
                .or_default()
                .push((index, value)),
            Err(error) => results[index].1 = Some(Err(error)),
        }
    }
    let scope = artist_kernel::InvocationScope::with_cancellation(context, cancellation);

    // Same-resource mutations are one transaction, regardless of tool name.
    // This is deliberately resolved before the ordinary per-tool groups so
    // write+edit and edit+insert cannot execute in hidden map order.
    let mut mutation_groups = BTreeMap::<String, Vec<(usize, String, DynamicValue)>>::new();
    for (index, call) in calls.iter().enumerate() {
        if !matches!(call.name.as_str(), "write" | "edit" | "insert") || results[index].1.is_some()
        {
            continue;
        }
        let Some(value) = grouped
            .get(&call.name)
            .and_then(|items| items.iter().find(|(item, _)| *item == index))
            .map(|(_, value)| value.clone())
        else {
            continue;
        };
        let Some(uri) = value_uri(&value) else {
            continue;
        };
        mutation_groups
            .entry(uri.clone())
            .or_default()
            .push((index, call.name.clone(), value));
    }
    let mut handled = BTreeSet::new();
    for (uri_text, items) in mutation_groups
        .into_iter()
        .filter(|(_, items)| items.len() > 1)
    {
        let _uri = match artist_kernel::ResourceUri::parse(&uri_text) {
            Ok(uri) => uri,
            Err(error) => {
                for (index, _, _) in items {
                    results[index].1 = Some(Err(error.to_string()));
                }
                continue;
            }
        };
        let transaction = kernel.new_mutation_transaction(items.len());
        let outputs = join_all(items.iter().map(|(_, name, value)| {
            let kernel = kernel.clone();
            let transaction = transaction.clone();
            let scope = scope.child().with_mutation_transaction(transaction.clone());
            let name = name.clone();
            let value = value.clone();
            async move {
                let result = kernel
                    .execute_tool_models_for_model(name.as_str(), vec![value], scope)
                    .await
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| {
                        Err(artist_kernel::KernelError::Handler {
                            message: "transactional tool returned no result".to_owned(),
                        })
                    });
                if let Err(error) = &result {
                    transaction.fail(error.clone());
                }
                if let Ok(value) = &result {
                    if let Err(error) = &value.stdout {
                        transaction.fail(error.clone());
                    }
                }
                result
            }
        }))
        .await;
        for ((index, _, _), output) in items.into_iter().zip(outputs) {
            handled.insert(index);
            results[index].1 = Some(
                output
                    .map(|value| (model_result_json(&value), value.stdout.is_err()))
                    .map_err(|error| error.to_string()),
            );
        }
    }
    let group_results = join_all(grouped.into_iter().filter_map(|(name, items)| {
        let items = items
            .into_iter()
            .filter(|(index, _)| !handled.contains(index))
            .collect::<Vec<_>>();
        (!items.is_empty()).then(|| {
            let kernel = kernel.clone();
            let scope = scope.child();
            async move {
                let values = kernel
                    .execute_tool_models_for_model(
                        name.as_str(),
                        items.iter().map(|(_, value)| value.clone()).collect(),
                        scope,
                    )
                    .await;
                (items, values)
            }
        })
    }))
    .await;
    for (items, values) in group_results {
        for ((index, _), value) in items.into_iter().zip(values) {
            results[index].1 = Some(
                value
                    .map(|value| (model_result_json(&value), value.stdout.is_err()))
                    .map_err(|error| error.to_string()),
            );
        }
    }
    results
        .into_iter()
        .map(|(id, result)| {
            (
                id,
                result.unwrap_or_else(|| Err("tool call was not executed".to_owned())),
            )
        })
        .collect()
}

/// Model-facing tool content is the authoritative stdout projection.  Keep
/// its lossless DynamicValue encoding intact, including Result/Variant/Option
/// constructors and numeric widths; the ordinary JSON helper is intentionally
/// only for typed WIT ingress/egress where the expected type is known.
fn model_result_json(value: &artist_kernel::ToolModelResult) -> Value {
    serde_json::from_str(&value.stdobs).unwrap_or_else(|_| Value::String(value.stdobs.clone()))
}

fn value_uri(value: &DynamicValue) -> Option<String> {
    let DynamicValue::Record(fields) = value else {
        return None;
    };
    match fields.get("uri").or_else(|| fields.get("root")) {
        Some(DynamicValue::ResourceUri(uri)) => Some(uri.to_string()),
        Some(DynamicValue::String(uri)) => Some(uri.clone()),
        _ => None,
    }
}

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
            let expected = definition.input_type.clone();
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
                    let expected = expected.clone();
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
                        let expected = expected.as_ref().ok_or_else(|| {
                            ToolExecutionError::other(format!(
                                "tool {name} has no active input type"
                            ))
                        })?;
                        let arguments = normalize_json(arguments, expected)
                            .map_err(ToolExecutionError::other)?;
                        let output = kernel
                            .execute_tool_for_model(&name, arguments, scope)
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

pub fn normalize_json(value: Value, expected: &DynamicType) -> Result<DynamicValue, String> {
    fn normalized(value: &str) -> String {
        value
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }
    fn field<'a>(fields: &'a Map<String, Value>, name: &str) -> Result<Option<&'a Value>, String> {
        let wanted = normalized(name);
        let matches = fields
            .iter()
            .filter(|(key, _)| normalized(key) == wanted)
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(format!("multiple fields normalize to {name}"));
        }
        Ok(matches.first().map(|(_, value)| *value))
    }
    match expected {
        DynamicType::Bool => value
            .as_bool()
            .map(DynamicValue::Bool)
            .ok_or_else(|| "expected boolean".into()),
        DynamicType::S8 | DynamicType::S16 | DynamicType::S32 | DynamicType::S64 => {
            let number = value
                .as_i64()
                .ok_or_else(|| "expected signed integer".to_owned())?;
            match expected {
                DynamicType::S8 if i8::try_from(number).is_ok() => {
                    Ok(DynamicValue::S8(number as i8))
                }
                DynamicType::S16 if i16::try_from(number).is_ok() => {
                    Ok(DynamicValue::S16(number as i16))
                }
                DynamicType::S32 if i32::try_from(number).is_ok() => {
                    Ok(DynamicValue::S32(number as i32))
                }
                DynamicType::S64 => Ok(DynamicValue::S64(number)),
                _ => Err("signed integer is out of range".into()),
            }
        }
        DynamicType::U8 | DynamicType::U16 | DynamicType::U32 | DynamicType::U64 => {
            let number = value
                .as_u64()
                .ok_or_else(|| "expected nonnegative integer".to_owned())?;
            match expected {
                DynamicType::U8 if u8::try_from(number).is_ok() => {
                    Ok(DynamicValue::U8(number as u8))
                }
                DynamicType::U16 if u16::try_from(number).is_ok() => {
                    Ok(DynamicValue::U16(number as u16))
                }
                DynamicType::U32 if u32::try_from(number).is_ok() => {
                    Ok(DynamicValue::U32(number as u32))
                }
                DynamicType::U64 => Ok(DynamicValue::U64(number)),
                _ => Err("unsigned integer is out of range".into()),
            }
        }
        DynamicType::F32 => {
            let number = value.as_f64().ok_or_else(|| "expected number".to_owned())? as f32;
            number
                .is_finite()
                .then_some(DynamicValue::F32(number))
                .ok_or_else(|| "number must be finite".into())
        }
        DynamicType::F64 => {
            let number = value.as_f64().ok_or_else(|| "expected number".to_owned())?;
            number
                .is_finite()
                .then_some(DynamicValue::F64(number))
                .ok_or_else(|| "number must be finite".into())
        }
        DynamicType::Char => {
            let text = value
                .as_str()
                .ok_or_else(|| "expected one Unicode scalar".to_owned())?;
            let mut chars = text.chars();
            let ch = chars
                .next()
                .ok_or_else(|| "expected one Unicode scalar".to_owned())?;
            chars
                .next()
                .is_none()
                .then_some(DynamicValue::Char(ch))
                .ok_or_else(|| "expected one Unicode scalar".into())
        }
        DynamicType::String => value
            .as_str()
            .map(|value| DynamicValue::String(value.to_owned()))
            .ok_or_else(|| "expected string".into()),
        DynamicType::ResourceUri => {
            let text = value
                .as_str()
                .ok_or_else(|| "expected URI string".to_owned())?;
            artist_kernel::ResourceUri::parse(text)
                .map(DynamicValue::ResourceUri)
                .map_err(|error| error.to_string())
        }
        DynamicType::List(element) => {
            let values = value
                .as_array()
                .ok_or_else(|| "expected array".to_owned())?;
            Ok(DynamicValue::List(
                values
                    .iter()
                    .cloned()
                    .map(|value| normalize_json(value, element))
                    .collect::<Result<_, _>>()?,
            ))
        }
        DynamicType::Tuple(types) => {
            let values = value
                .as_array()
                .ok_or_else(|| "expected tuple array".to_owned())?;
            if values.len() != types.len() {
                return Err(format!(
                    "expected tuple of length {}, got {}",
                    types.len(),
                    values.len()
                ));
            }
            Ok(DynamicValue::Tuple(
                values
                    .iter()
                    .cloned()
                    .zip(types)
                    .map(|(value, ty)| normalize_json(value, ty))
                    .collect::<Result<_, _>>()?,
            ))
        }
        DynamicType::Option(inner) => {
            if value.is_null() {
                Ok(DynamicValue::Option(None))
            } else {
                Ok(DynamicValue::Option(Some(Box::new(normalize_json(
                    value, inner,
                )?))))
            }
        }
        DynamicType::Record(types) => {
            let fields = value
                .as_object()
                .ok_or_else(|| "expected object".to_owned())?;
            let mut result = BTreeMap::new();
            for (name, ty) in types {
                match field(fields, name)? {
                    Some(value) => {
                        result.insert(name.clone(), normalize_json(value.clone(), ty)?);
                    }
                    None if matches!(ty, DynamicType::Option(_)) => {
                        result.insert(name.clone(), DynamicValue::Option(None));
                    }
                    None => return Err(format!("missing required field {name}")),
                }
            }
            for key in fields.keys() {
                if !types.keys().any(|name| normalized(name) == normalized(key)) {
                    return Err(format!("unknown field {key}"));
                }
            }
            Ok(DynamicValue::Record(result))
        }
        DynamicType::Enum(cases) => {
            let text = value
                .as_str()
                .ok_or_else(|| "expected enum string".to_owned())?;
            let matches = cases
                .iter()
                .filter(|case| normalized(case) == normalized(text))
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                Ok(DynamicValue::Enum(matches[0].clone()))
            } else {
                Err(format!("unknown or ambiguous enum case {text}"))
            }
        }
        DynamicType::Variant(cases) => {
            if let Some(text) = value.as_str() {
                if cases.contains_key("at") && text.starts_with('#') {
                    return Ok(DynamicValue::Variant(
                        "at".to_owned(),
                        Some(Box::new(DynamicValue::String(
                            text.trim_start_matches('#').to_owned(),
                        ))),
                    ));
                }
                let expected_case = cases
                    .keys()
                    .find(|name| normalized(name) == normalized(text))
                    .ok_or_else(|| format!("unknown variant case {text}"))?;
                if cases[expected_case].is_none() {
                    return Ok(DynamicValue::Variant(expected_case.clone(), None));
                }
                return Err(format!("variant case {text} requires a payload"));
            }
            let object = value
                .as_object()
                .ok_or_else(|| "expected variant case string or object".to_owned())?;
            if object.len() != 1 {
                return Err("variant object must contain exactly one case".into());
            }
            let (case, payload) = object
                .iter()
                .next()
                .ok_or_else(|| "variant object is empty".to_owned())?;
            let expected_case = cases
                .keys()
                .find(|name| normalized(name) == normalized(case))
                .ok_or_else(|| format!("unknown variant case {case}"))?;
            let Some(ty) = cases[expected_case].as_ref() else {
                if payload.is_null() {
                    return Ok(DynamicValue::Variant(expected_case.clone(), None));
                }
                return Err(format!("variant case {case} does not accept a payload"));
            };
            Ok(DynamicValue::Variant(
                expected_case.clone(),
                Some(Box::new(normalize_json(payload.clone(), ty)?)),
            ))
        }
        DynamicType::Flags(flags) => {
            let values = value
                .as_array()
                .ok_or_else(|| "expected flags array".to_owned())?;
            let mut output = Vec::new();
            for value in values {
                let text = value
                    .as_str()
                    .ok_or_else(|| "flag names must be strings".to_owned())?;
                let matched = flags
                    .iter()
                    .find(|flag| normalized(flag) == normalized(text))
                    .ok_or_else(|| format!("unknown flag {text}"))?;
                if output.contains(matched) {
                    return Err(format!("duplicate flag {text}"));
                }
                output.push(matched.clone());
            }
            Ok(DynamicValue::Flags(output))
        }
        DynamicType::Result { ok, err } => {
            let object = value
                .as_object()
                .ok_or_else(|| "result requires explicit ok or err object".to_owned())?;
            if object.len() != 1 {
                return Err("result requires exactly one of ok or err".into());
            }
            if let Some(value) = object.get("ok") {
                return Ok(DynamicValue::Result(Ok(Box::new(normalize_json(
                    value.clone(),
                    ok.as_ref().ok_or_else(|| "result has no ok type")?,
                )?))));
            }
            if let Some(value) = object.get("err") {
                return Ok(DynamicValue::Result(Err(Box::new(normalize_json(
                    value.clone(),
                    err.as_ref().ok_or_else(|| "result has no err type")?,
                )?))));
            }
            Err("result requires explicit ok or err".into())
        }
    }
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
        DynamicValue::Result(Ok(value)) => serde_json::json!({"ok": dynamic_to_json(*value)}),
        DynamicValue::Result(Err(value)) => serde_json::json!({"err": dynamic_to_json(*value)}),
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
