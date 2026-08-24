use artist_plugin_sdk::artist::plugin::{native_plugins, types::*};

struct PluginsChildren;

impl artist_plugin_sdk::exports::artist::plugin::resource_provider::Guest for PluginsChildren {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Ok(vec![ResourceRoute {
            base_glob: "plugins:///**".into(),
            projection_glob: None,
            operations: vec![ResourceOperation::Children],
            signals: Vec::new(),
        }])
    }

    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let ResourceRequest::Children(uri) = request else {
            return Err(ResourceError::Invalid("expected children request".into()));
        };
        native_plugins::children(&uri)
            .map(ResourceReply::Children)
            .map_err(ResourceError::Provider)
    }
}

artist_plugin_sdk::unadvertised_lifecycle!(
    PluginsChildren,
    "artist.plugins.children",
    Capability::Resources
);
artist_plugin_sdk::unadvertised_tools!(PluginsChildren);
artist_plugin_sdk::unadvertised_slash_commands!(PluginsChildren);
artist_plugin_sdk::export!(PluginsChildren with_types_in artist_plugin_sdk);
