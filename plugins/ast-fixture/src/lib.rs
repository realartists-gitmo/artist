wit_bindgen::generate!({ path: "../../wit", world: "artist-plugin" });

use artist::plugin::types::{
    Capability, ContextFragment, HookDecision, HookEvent, Message, ModelConfig, PluginDescriptor,
    ResourceError, ResourceOperation, ResourceReply, ResourceRequest, ResourceRoute,
    ToolDefinition,
};
use exports::artist::plugin::{
    lifecycle::Guest as Lifecycle, resource_provider::Guest as Resources,
    tool_provider::Guest as Tools,
};
use serde_json::json;

struct AstFixture;

impl Lifecycle for AstFixture {
    fn descriptor() -> PluginDescriptor {
        PluginDescriptor {
            id: "artist.fixture.rust-symbols".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec![Capability::Resources, Capability::Tools],
        }
    }
    fn compose_prompt(v: Vec<ContextFragment>) -> Result<Vec<ContextFragment>, String> {
        Ok(v)
    }
    fn transform_context(v: Vec<Message>) -> Result<Vec<Message>, String> {
        Ok(v)
    }
    fn observe_hook(_: HookEvent) -> Result<HookDecision, String> {
        Ok(HookDecision::Proceed)
    }
    fn configure_model(v: ModelConfig) -> Result<ModelConfig, String> {
        Ok(v)
    }
    fn observe_event(_: String) -> Result<(), String> {
        Ok(())
    }
}
impl Tools for AstFixture {
    fn definitions() -> Result<Vec<ToolDefinition>, String> {
        Ok(vec![
            definition("fixture-cross"),
            definition("fixture-cycle-a"),
            definition("fixture-direct-cycle"),
            definition("fixture-same-a"),
            definition("fixture-same-b"),
            definition("fixture-list-tools"),
        ])
    }
    fn invoke(name: String, arguments: String) -> Result<String, String> {
        match name.as_str() {
            "fixture-cross" => artist::plugin::host_tools::call_tool("fixture-leaf", &arguments),
            "fixture-cycle-a" => artist::plugin::host_tools::call_tool("fixture-cycle-b", "{}"),
            "fixture-direct-cycle" => {
                artist::plugin::host_tools::call_tool("fixture-direct-cycle", "{}")
            }
            "fixture-same-a" => artist::plugin::host_tools::call_tool("fixture-same-b", "{}"),
            "fixture-same-b" => Ok(arguments),
            "fixture-list-tools" => artist::plugin::host_tools::list_tools()
                .map(|tools| serde_json::to_string(&tools.len()).unwrap()),
            _ => Err(format!("unknown tool: {name}")),
        }
    }
}
impl Resources for AstFixture {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Ok(["file:///**/*.rs", "fixture://**/*.rs"]
            .into_iter()
            .map(|base_glob| ResourceRoute {
                base_glob: base_glob.into(),
                projection_glob: Some("symbols/**".into()),
                operations: vec![ResourceOperation::Read, ResourceOperation::Children],
            })
            .collect())
    }
    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let uri = match &request {
            ResourceRequest::Read(r) => &r.uri,
            ResourceRequest::Children(u) => u,
            _ => return Err(error("unsupported", "fixture resources are read-only")),
        };
        let (base, projection) = uri
            .split_once('?')
            .ok_or_else(|| error("invalid", "missing symbols projection"))?;
        let text = call("read", json!({"uri": base}))?
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let segments: Vec<_> = projection.split('/').collect();
        if segments.first() != Some(&"symbols") {
            return Err(error("not-found", "unknown projection"));
        }
        let symbols = rust_functions(&text);
        match request {
            ResourceRequest::Children(_) if segments.len() == 1 => Ok(ResourceReply::Children(
                symbols
                    .into_iter()
                    .map(|name| format!("{base}?symbols/{name}"))
                    .collect(),
            )),
            ResourceRequest::Children(_) if segments.len() == 2 => Ok(ResourceReply::Children(
                vec![format!("{base}?symbols/{}/callers", segments[1])],
            )),
            ResourceRequest::Read(_) if segments.len() == 2 => symbols
                .into_iter()
                .find(|name| name == segments[1])
                .map(|name| ResourceReply::Text(format!("symbol {name}\n")))
                .ok_or_else(|| error("not-found", "symbol not found")),
            ResourceRequest::Read(_) if segments.len() == 3 && segments[2] == "callers" => {
                let needle = format!("{}(", segments[1]);
                let callers = text
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| line.contains(&needle))
                    .map(|(line, text)| format!("{}:{}\n", line + 1, text.trim()))
                    .collect();
                Ok(ResourceReply::Text(callers))
            }
            _ => Err(error("not-found", "unknown symbols descendant")),
        }
    }
}

fn call(name: &str, arguments: serde_json::Value) -> Result<serde_json::Value, ResourceError> {
    artist::plugin::host_tools::call_tool(name, &arguments.to_string())
        .map_err(|e| error("provider", &e))
        .and_then(|v| serde_json::from_str(&v).map_err(|e| error("provider", &e.to_string())))
}
fn rust_functions(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let tail = line
                .trim_start()
                .strip_prefix("pub ")
                .unwrap_or(line.trim_start())
                .strip_prefix("fn ")?;
            Some(
                tail.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .next()?
                    .to_owned(),
            )
        })
        .collect()
}
fn error(kind: &str, message: &str) -> ResourceError {
    ResourceError {
        kind: kind.into(),
        message: message.into(),
    }
}

fn definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: "WASM host-tool bridge fixture".into(),
        input_schema: json!({"type": "object"}).to_string(),
    }
}

export!(AstFixture);
