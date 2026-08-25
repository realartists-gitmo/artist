use artist_plugin_sdk::artist::plugin::{native_plugins, types::*};

struct PluginsRead;

impl artist_plugin_sdk::exports::artist::plugin::resource_provider::Guest for PluginsRead {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Ok(vec![route(ResourceOperation::Read)])
    }

    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let ResourceRequest::Read(request) = request else {
            return Err(ResourceError::Invalid("expected read request".into()));
        };
        native_plugins::read(&request.uri, request.start_line, request.line_count)
            .map(ResourceReply::Text)
            .map_err(ResourceError::Provider)
    }
}

fn route(operation: ResourceOperation) -> ResourceRoute {
    ResourceRoute {
        base_glob: "plugins:///**".into(),
        projection_glob: None,
        operations: vec![operation],
        signals: Vec::new(),
    }
}

artist_plugin_sdk::unadvertised_lifecycle!(
    PluginsRead,
    "artist.plugins.read",
    Capability::Resources
);
artist_plugin_sdk::unadvertised_tools!(PluginsRead);
artist_plugin_sdk::unadvertised_slash_commands!(PluginsRead);
artist_plugin_sdk::unadvertised_model_provider!(PluginsRead);
artist_plugin_sdk::export!(PluginsRead with_types_in artist_plugin_sdk);
