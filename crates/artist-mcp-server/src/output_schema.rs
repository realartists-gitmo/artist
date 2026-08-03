//! MCP output contracts for Artist's exported harness tools.
//!
//! Rig's portable tool contract predates MCP `outputSchema`, and most Artist
//! tools deliberately return model-oriented text. The MCP adapter therefore
//! publishes a small structured envelope for every successful result while
//! preserving the existing text content verbatim. Each Artist tool gets its
//! own schema (identified by a `tool` const and a tool-specific description),
//! and JSON-shaped results are also carried in `data`.

use serde_json::{Value, json};

/// Every tool the Artist-owned MCP surface can currently publish. Keep this
/// exhaustive so a newly exported tool cannot silently lose its output
/// contract.
pub(crate) const ARTIST_MCP_TOOL_NAMES: &[&str] = &[
    "bash",
    "read",
    "find",
    "grep",
    "edit",
    "write",
    "skill",
    "todo",
    "memory",
    "code_map",
    "code_show",
    "code_surface",
    "code_implements",
    "code_deps",
    "code_cycles",
    "code_trace",
    "code_impact",
    "code_search",
    "code_related",
    "ast_query",
    "ast_rewrite",
    "computer",
    "canvas",
    "subagent",
    "tell",
    "query",
    "reply",
    "gc",
    "ask",
    "ask_result",
    "ask_answer",
    "ask_list",
    "init",
];

/// Build the MCP `outputSchema` for one published tool.
///
/// Unknown dynamic tools still receive an honest generic envelope. Artist's
/// own surface is covered exhaustively by [`ARTIST_MCP_TOOL_NAMES`] and tested.
pub(crate) fn for_tool(name: &str) -> Value {
    let description = result_description(name);
    let data = data_schema(name);
    let title = if ARTIST_MCP_TOOL_NAMES.contains(&name) {
        format!("Artist {name} result")
    } else {
        format!("Dynamic tool {name} result")
    };
    json!({
        "type": "object",
        "title": title,
        "description": format!(
            "Structured Artist MCP result for `{name}`. {description} The `text` field is the canonical model-visible rendering."
        ),
        "properties": {
            "tool": {
                "type": "string",
                "const": name,
                "description": "The Artist tool that produced this result."
            },
            "kind": {
                "type": "string",
                "enum": ["text", "json", "multimodal"],
                "description": "How the underlying harness result was represented."
            },
            "text": {
                "type": "string",
                "description": description
            },
            "data": data
        },
        "required": ["tool", "kind", "text"],
        "additionalProperties": false
    })
}

/// Normalize a successful harness result into the object promised by
/// [`for_tool`]. JSON returned as a native block, or as a legacy JSON string,
/// is retained in `data`; ordinary text and images remain in the canonical
/// `text` rendering and MCP content blocks.
pub(crate) fn structured_result(name: &str, output: &rig_core::tool::ToolOutput) -> Value {
    let text = output.render();
    let native_json = output.as_json().cloned();
    let legacy_json = output
        .as_text()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let data = native_json.or(legacy_json);
    let kind = if data.is_some() {
        "json"
    } else if output.as_text().is_some() {
        "text"
    } else {
        "multimodal"
    };

    let mut result = serde_json::Map::from_iter([
        ("tool".to_owned(), Value::String(name.to_owned())),
        ("kind".to_owned(), Value::String(kind.to_owned())),
        ("text".to_owned(), Value::String(text)),
    ]);
    if let Some(data) = data {
        result.insert("data".to_owned(), data);
    }
    Value::Object(result)
}

fn data_schema(name: &str) -> Value {
    match name {
        "tell" => json!({
            "type": "object",
            "properties": {"delivered": {"type": "integer", "minimum": 0}},
            "required": ["delivered"],
            "additionalProperties": false,
            "description": "Delivery count when the messaging tool returns JSON."
        }),
        "ask_result" => json!({
            "type": "object",
            "properties": {
                "results": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "questionId": {"type": "string"},
                            "status": {"enum": ["pending", "answered"]},
                            "answer": {"type": "string"},
                            "question": {"type": ["object", "null"]}
                        },
                        "required": ["questionId", "status"],
                        "additionalProperties": true
                    }
                }
            },
            "required": ["results"],
            "additionalProperties": false,
            "description": "Per-question pending or answered state."
        }),
        "ask_list" => json!({
            "type": "object",
            "properties": {
                "pending": {"type": "array", "items": {"type": "object"}}
            },
            "required": ["pending"],
            "additionalProperties": false,
            "description": "Questions still awaiting an answer."
        }),
        "gc" => json!({
            "type": "object",
            "description": "Group preview or creation details.",
            "additionalProperties": true
        }),
        _ => json!({
            "description": "Optional machine-readable payload when this tool returns JSON."
        }),
    }
}

fn result_description(name: &str) -> &'static str {
    match name {
        "bash" => "Shell execution or persistent-session status and output.",
        "read" => "File, directory, or image content selected by the read request.",
        "find" => "Paths matching the requested name or glob search.",
        "grep" => "Text matches with their paths and source locations.",
        "edit" => "Confirmation and diagnostics for an applied anchored edit.",
        "write" => "Confirmation and diagnostics for a file write.",
        "skill" => "The selected skill's instructions and supporting context.",
        "todo" => "The current durable todo list and completion summary.",
        "memory" => "Memory write, retrieval, or maintenance results.",
        "code_map" => "A structural map of the requested code scope.",
        "code_show" => "The requested symbol or code object's definition and context.",
        "code_surface" => "Public surface information for the requested code scope.",
        "code_implements" => "Implementations associated with the requested abstraction.",
        "code_deps" => "Dependency relationships for the requested code object.",
        "code_cycles" => "Dependency cycles found in the requested scope.",
        "code_trace" => "A traced path through calls or dependencies.",
        "code_impact" => "Callers, dependants, or other code affected by a change.",
        "code_search" => "Semantic or lexical code-search hits.",
        "code_related" => "Code objects related to the requested symbol or passage.",
        "ast_query" => "Structural syntax matches for the AST query.",
        "ast_rewrite" => "Files and matches changed by the structural rewrite.",
        "computer" => "Computer surface state, observations, or action results.",
        "canvas" => "Canvas lifecycle, rendering, or interaction results.",
        "subagent" => "The delegated agent's completed response.",
        "tell" => "Delivery status for a non-blocking agent message.",
        "query" => "The response received from the addressed agent or group.",
        "reply" => "Delivery status for a reply to the most recent sender.",
        "gc" => "Selected group members, preview details, or the created group id.",
        "ask" => "Durable question ids posted for out-of-band user response.",
        "ask_result" => "Pending or answered state for each requested question id.",
        "ask_answer" => "Whether the supplied answer was recorded or superseded.",
        "ask_list" => "Every durable question currently awaiting an answer.",
        "init" => "The durable Artist identity claimed by this MCP connection.",
        _ => "Result returned by a dynamically supplied tool.",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_artist_mcp_tool_has_a_distinct_object_schema() {
        let names: BTreeSet<_> = ARTIST_MCP_TOOL_NAMES.iter().copied().collect();
        assert_eq!(names.len(), ARTIST_MCP_TOOL_NAMES.len());
        for name in ARTIST_MCP_TOOL_NAMES {
            let schema = for_tool(name);
            assert_eq!(schema["type"], "object", "{name}");
            assert_eq!(schema["properties"]["tool"]["const"], *name, "{name}");
            assert_eq!(
                schema["required"],
                json!(["tool", "kind", "text"]),
                "{name}"
            );
        }
    }

    #[test]
    fn text_and_legacy_json_results_match_the_envelope_shape() {
        let text = rig_core::tool::ToolOutput::text("done");
        assert_eq!(
            structured_result("write", &text),
            json!({"tool": "write", "kind": "text", "text": "done"})
        );

        let json_text = rig_core::tool::ToolOutput::text(r#"{"delivered":2}"#);
        assert_eq!(
            structured_result("tell", &json_text),
            json!({
                "tool": "tell",
                "kind": "json",
                "text": r#"{"delivered":2}"#,
                "data": {"delivered": 2}
            })
        );
    }
}
