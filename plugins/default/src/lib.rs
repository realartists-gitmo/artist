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

struct DefaultPlugin;

impl Lifecycle for DefaultPlugin {
    fn descriptor() -> PluginDescriptor {
        PluginDescriptor {
            id: "artist.default".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec![
                Capability::Prompt,
                Capability::Resources,
                Capability::Context,
                Capability::Hooks,
                Capability::Model,
                Capability::Events,
            ],
        }
    }
    fn compose_prompt(fragments: Vec<ContextFragment>) -> Result<Vec<ContextFragment>, String> {
        Ok(fragments
            .into_iter()
            .filter(|f| !f.content.trim().is_empty())
            .collect())
    }
    fn transform_context(messages: Vec<Message>) -> Result<Vec<Message>, String> {
        let from = messages.len().saturating_sub(200);
        Ok(messages.into_iter().skip(from).collect())
    }
    fn observe_hook(_: HookEvent) -> Result<HookDecision, String> {
        Ok(HookDecision::Proceed)
    }
    fn configure_model(config: ModelConfig) -> Result<ModelConfig, String> {
        Ok(config)
    }
    fn observe_event(_: String) -> Result<(), String> {
        Ok(())
    }
}

impl Tools for DefaultPlugin {
    fn definitions() -> Result<Vec<ToolDefinition>, String> {
        Err("tool socket is not advertised".into())
    }
    fn invoke(_: String, _: String) -> Result<String, String> {
        Err("unknown tool".into())
    }
}

impl Resources for DefaultPlugin {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Ok(vec![ResourceRoute {
            base_glob: "file:///**".into(),
            projection_glob: None,
            operations: vec![
                ResourceOperation::Read,
                ResourceOperation::Children,
                ResourceOperation::Write,
                ResourceOperation::Move,
            ],
        }])
    }
    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let failed = |message: String| ResourceError {
            kind: "provider".into(),
            message,
        };
        match request {
            ResourceRequest::Read(r) => {
                artist::plugin::native_filesystem::read(&r.uri, r.start_line, r.line_count)
                    .map(ResourceReply::Text)
                    .map_err(failed)
            }
            ResourceRequest::Children(uri) => artist::plugin::native_filesystem::children(&uri)
                .map(ResourceReply::Children)
                .map_err(failed),
            ResourceRequest::Write(r) => artist::plugin::native_filesystem::write(&r.uri, &r.text)
                .map(|_| ResourceReply::Written)
                .map_err(failed),
            ResourceRequest::Move(r) => {
                artist::plugin::native_filesystem::move_(&r.source, r.to.as_deref())
                    .map(|_| ResourceReply::Moved)
                    .map_err(failed)
            }
            ResourceRequest::Poll(r) => Err(ResourceError {
                kind: "unsupported".into(),
                message: format!("poll is unsupported on {}", r.uri),
            }),
            ResourceRequest::Edit(r) => Err(ResourceError {
                kind: "unsupported".into(),
                message: format!("edit is reserved on {}", r.uri),
            }),
        }
    }
}

export!(DefaultPlugin);
