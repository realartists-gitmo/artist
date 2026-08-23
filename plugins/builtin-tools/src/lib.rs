wit_bindgen::generate!({ path: "../../wit", world: "artist-plugin" });

use artist::plugin::types::{
    Capability, ContextFragment, FindRequest, GrepRequest, HookDecision, HookEvent, Message,
    ModelConfig, MoveRequest, PluginDescriptor, PollOutcome, PollRequest, ReadRequest,
    ResourceError, ResourceReply, ResourceRequest, ResourceRoute, ToolDefinition, WriteRequest,
};
use exports::artist::plugin::{
    lifecycle::Guest as Lifecycle, resource_provider::Guest as Resources,
    tool_provider::Guest as Tools,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    uri: String,
    #[schemars(range(min = 1))]
    start_line: Option<u64>,
    #[schemars(range(min = 1))]
    line_count: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FindArgs {
    uri: String,
    glob: Option<String>,
    max_depth: Option<u64>,
    cursor: Option<String>,
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GrepArgs {
    uri: String,
    regex: String,
    include_glob: Option<String>,
    context: Option<u64>,
    cursor: Option<String>,
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    uri: String,
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    from: String,
    to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PollArgs {
    uri: String,
    #[serde(rename = "match")]
    #[schemars(rename = "match")]
    pattern: Option<String>,
    timeout_ms: Option<u64>,
}

struct BuiltinTools;

impl Lifecycle for BuiltinTools {
    fn descriptor() -> PluginDescriptor {
        PluginDescriptor {
            id: "artist.builtin.tools".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec![Capability::Tools],
        }
    }

    fn compose_prompt(_: Vec<ContextFragment>) -> Result<Vec<ContextFragment>, String> {
        Err("prompt socket is not advertised".into())
    }

    fn transform_context(_: Vec<Message>) -> Result<Vec<Message>, String> {
        Err("context socket is not advertised".into())
    }

    fn observe_hook(_: HookEvent) -> Result<HookDecision, String> {
        Err("hook socket is not advertised".into())
    }

    fn configure_model(_: ModelConfig) -> Result<ModelConfig, String> {
        Err("model socket is not advertised".into())
    }

    fn observe_event(_: String) -> Result<(), String> {
        Err("event socket is not advertised".into())
    }
}

impl Tools for BuiltinTools {
    fn definitions() -> Result<Vec<ToolDefinition>, String> {
        Ok(vec![
            definition::<ReadArgs>("read", "Read UTF-8 text from a resource URI."),
            definition::<FindArgs>(
                "find",
                "Find descendants of a resource URI. With no glob and depth one, lists immediate children.",
            ),
            definition::<GrepArgs>("grep", "Search text resources with a regular expression."),
            definition::<WriteArgs>("write", "Write UTF-8 text to a resource URI."),
            definition::<MoveArgs>("move", "Move a resource, or remove it when `to` is null."),
            definition::<PollArgs>(
                "poll",
                "Wait for subsequently appended text, a match, close, or timeout.",
            ),
        ])
    }

    fn invoke(name: String, arguments: String) -> Result<String, String> {
        match name.as_str() {
            "read" => {
                let args: ReadArgs = parse_arguments(&arguments)?;
                if args.start_line == Some(0) || args.line_count == Some(0) {
                    return Err(
                        "invalid tool arguments: start_line and line_count must be positive".into(),
                    );
                }
                resource(ResourceRequest::Read(ReadRequest {
                    uri: args.uri,
                    start_line: args.start_line,
                    line_count: args.line_count,
                }))
            }
            "find" => {
                let args: FindArgs = parse_arguments(&arguments)?;
                artist::plugin::host_resources::find(&FindRequest {
                    uri: args.uri,
                    glob: args.glob,
                    max_depth: args.max_depth,
                    cursor: args.cursor,
                    limit: Some(args.limit.unwrap_or(50)),
                })
            }
            "grep" => {
                let args: GrepArgs = parse_arguments(&arguments)?;
                artist::plugin::host_resources::grep(&GrepRequest {
                    uri: args.uri,
                    regex: args.regex,
                    include_glob: args.include_glob,
                    context: args.context,
                    cursor: args.cursor,
                    limit: Some(args.limit.unwrap_or(100)),
                })
            }
            "write" => {
                let args: WriteArgs = parse_arguments(&arguments)?;
                resource(ResourceRequest::Write(WriteRequest {
                    uri: args.uri,
                    text: args.text,
                }))
            }
            "move" => {
                let args: MoveArgs = parse_arguments(&arguments)?;
                resource(ResourceRequest::Move(MoveRequest {
                    source: args.from,
                    to: args.to,
                }))
            }
            "poll" => {
                let args: PollArgs = parse_arguments(&arguments)?;
                resource(ResourceRequest::Poll(PollRequest {
                    uri: args.uri,
                    match_: args.pattern,
                    timeout_ms: args.timeout_ms,
                }))
            }
            _ => Err(format!("unknown tool: {name}")),
        }
    }
}

impl Resources for BuiltinTools {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Err("resource socket is not advertised".into())
    }

    fn handle(_: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        Err(ResourceError {
            kind: "unsupported".into(),
            message: "resource socket is not advertised".into(),
        })
    }
}

fn definition<T: JsonSchema>(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: serde_json::to_string(&schemars::schema_for!(T))
            .expect("tool schema is serializable"),
    }
}

fn parse_arguments<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|error| format!("invalid tool arguments: {error}"))
}

fn resource(request: ResourceRequest) -> Result<String, String> {
    let reply = artist::plugin::host_resources::handle(&request).map_err(resource_error)?;
    Ok(match reply {
        ResourceReply::Text(text) => json!({"text": text}),
        ResourceReply::Children(children) => json!({"children": children}),
        ResourceReply::Written => json!({"written": true}),
        ResourceReply::Moved => json!({"moved": true}),
        ResourceReply::Poll(reply) => json!({
            "text": reply.text,
            "outcome": match reply.outcome {
                PollOutcome::Matched => "matched",
                PollOutcome::Closed => "closed",
                PollOutcome::TimedOut => "timed-out",
            }
        }),
    }
    .to_string())
}

fn resource_error(error: ResourceError) -> String {
    format!("{}: {}", error.kind, error.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_exactly_the_six_closed_typed_contracts() {
        let definitions = <BuiltinTools as Tools>::definitions().unwrap();
        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            ["read", "find", "grep", "write", "move", "poll"]
        );
        for definition in &definitions {
            let schema: serde_json::Value = serde_json::from_str(&definition.input_schema).unwrap();
            assert_eq!(schema["additionalProperties"], false);
        }
        assert!(parse_arguments::<ReadArgs>(r#"{"uri":"x","extra":true}"#).is_err());
        assert!(parse_arguments::<GrepArgs>(r#"{"uri":"x"}"#).is_err());
        assert!(
            <BuiltinTools as Tools>::invoke("read".into(), r#"{"uri":"x","start_line":0}"#.into(),)
                .unwrap_err()
                .contains("must be positive")
        );
    }
}

export!(BuiltinTools);
