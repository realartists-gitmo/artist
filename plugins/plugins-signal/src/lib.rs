use artist_plugin_sdk::artist::plugin::{native_plugins, types::*};

struct PluginsSignal;

impl artist_plugin_sdk::exports::artist::plugin::resource_provider::Guest for PluginsSignal {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        let payload_schema = String::from(r#"{"type":"null"}"#);
        Ok(vec![ResourceRoute {
            base_glob: "plugins:///**".into(),
            projection_glob: None,
            operations: vec![ResourceOperation::Signal],
            signals: ["build", "activate", "build-and-activate"]
                .into_iter()
                .map(|name| SignalDefinition {
                    name: name.into(),
                    description: match name {
                        "build" => "Test and compile the package into an inactive candidate.",
                        "activate" => "Validate and atomically activate the built candidate.",
                        _ => "Build, validate, and atomically activate the package.",
                    }
                    .into(),
                    payload_schema: payload_schema.clone(),
                })
                .collect(),
        }])
    }

    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let ResourceRequest::Signal(request) = request else {
            return Err(ResourceError::Invalid("expected signal request".into()));
        };
        native_plugins::signal(&request.uri, &request.name, request.payload.as_deref())
            .map(|_| ResourceReply::Signaled)
    }
}

artist_plugin_sdk::unadvertised_lifecycle!(
    PluginsSignal,
    "artist.plugins.signal",
    Capability::Resources
);
artist_plugin_sdk::unadvertised_tools!(PluginsSignal);
artist_plugin_sdk::unadvertised_slash_commands!(PluginsSignal);
artist_plugin_sdk::unadvertised_model_provider!(PluginsSignal);
artist_plugin_sdk::export!(PluginsSignal with_types_in artist_plugin_sdk);
