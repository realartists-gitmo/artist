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
use std::cell::RefCell;

thread_local! {
    static NOTES: RefCell<String> = RefCell::new("ordinary fixture body\n".into());
}

struct ToolFixture;

impl Lifecycle for ToolFixture {
    fn descriptor() -> PluginDescriptor {
        PluginDescriptor {
            id: "artist.fixture.tools".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec![Capability::Tools, Capability::Resources],
        }
    }
    fn compose_prompt(value: Vec<ContextFragment>) -> Result<Vec<ContextFragment>, String> {
        let _ = value;
        Err("prompt socket is not advertised".into())
    }
    fn transform_context(value: Vec<Message>) -> Result<Vec<Message>, String> {
        let _ = value;
        Err("context socket is not advertised".into())
    }
    fn observe_hook(_: HookEvent) -> Result<HookDecision, String> {
        Err("hook socket is not advertised".into())
    }
    fn configure_model(value: ModelConfig) -> Result<ModelConfig, String> {
        let _ = value;
        Err("model socket is not advertised".into())
    }
    fn observe_event(_: String) -> Result<(), String> {
        Err("event socket is not advertised".into())
    }
}

impl Tools for ToolFixture {
    fn definitions() -> Result<Vec<ToolDefinition>, String> {
        Ok(vec![
            definition("fixture-leaf"),
            definition("fixture-cycle-b"),
        ])
    }

    fn invoke(name: String, arguments: String) -> Result<String, String> {
        match name.as_str() {
            "fixture-leaf" => Ok(arguments),
            "fixture-cycle-b" => artist::plugin::host_tools::call_tool("fixture-cycle-a", "{}"),
            _ => Err(format!("unknown tool: {name}")),
        }
    }
}

impl Resources for ToolFixture {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Ok(vec![ResourceRoute {
            base_glob: "fixture://**".into(),
            projection_glob: None,
            operations: vec![
                ResourceOperation::Read,
                ResourceOperation::Children,
                ResourceOperation::Write,
            ],
        }])
    }
    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let uri = match &request {
            ResourceRequest::Read(request) => request.uri.clone(),
            ResourceRequest::Children(uri) => uri.clone(),
            ResourceRequest::Write(request) => request.uri.clone(),
            _ => return Err(resource_error("unsupported", "fixture tree is read-only")),
        };
        match (request, uri.as_str()) {
            (ResourceRequest::Children(_), "fixture:///") => {
                Ok(ResourceReply::Children(vec!["fixture://workspace/".into()]))
            }
            (ResourceRequest::Children(_), "fixture://workspace/") => {
                Ok(ResourceReply::Children(vec![
                    "fixture://workspace/rust.rs".into(),
                    "fixture://workspace/notes.txt".into(),
                ]))
            }
            (ResourceRequest::Read(_), "fixture://workspace/rust.rs") => Ok(ResourceReply::Text(
                "pub fn fixture_symbol() {}\nfn caller() { fixture_symbol(); }\n".into(),
            )),
            (ResourceRequest::Read(_), "fixture://workspace/notes.txt") => Ok(ResourceReply::Text(
                NOTES.with(|notes| notes.borrow().clone()),
            )),
            (ResourceRequest::Write(request), "fixture://workspace/notes.txt") => {
                NOTES.with(|notes| *notes.borrow_mut() = request.text);
                Ok(ResourceReply::Written)
            }
            _ => Err(resource_error(
                "not-found",
                &format!("unknown resource: {uri}"),
            )),
        }
    }
}

fn resource_error(kind: &str, message: &str) -> ResourceError {
    ResourceError {
        kind: kind.into(),
        message: message.into(),
    }
}

fn definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: "WASM tool bridge fixture".into(),
        input_schema: json!({"type": "object"}).to_string(),
    }
}

export!(ToolFixture);
