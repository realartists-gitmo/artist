use artist_plugin_sdk::artist::plugin::{native_plugins, types::*};

struct PluginsMove;

impl artist_plugin_sdk::exports::artist::plugin::resource_provider::Guest for PluginsMove {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Ok(vec![ResourceRoute {
            base_glob: "plugins:///**".into(),
            projection_glob: None,
            operations: vec![ResourceOperation::Move],
            signals: Vec::new(),
        }])
    }

    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let ResourceRequest::Move(request) = request else {
            return Err(ResourceError::Invalid("expected move request".into()));
        };
        native_plugins::move_(&request.source, request.to.as_deref())
            .map(|_| ResourceReply::Moved)
            .map_err(ResourceError::Provider)
    }
}

artist_plugin_sdk::unadvertised_lifecycle!(
    PluginsMove,
    "artist.plugins.move",
    Capability::Resources
);
artist_plugin_sdk::unadvertised_tools!(PluginsMove);
artist_plugin_sdk::unadvertised_slash_commands!(PluginsMove);
artist_plugin_sdk::unadvertised_model_provider!(PluginsMove);
artist_plugin_sdk::export!(PluginsMove with_types_in artist_plugin_sdk);
