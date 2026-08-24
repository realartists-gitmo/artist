use artist_plugin_sdk::artist::plugin::{native_plugins, types::*};

struct PluginsWrite;

impl artist_plugin_sdk::exports::artist::plugin::resource_provider::Guest for PluginsWrite {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Ok(vec![ResourceRoute {
            base_glob: "plugins:///**".into(),
            projection_glob: None,
            operations: vec![ResourceOperation::Write],
            signals: Vec::new(),
        }])
    }

    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let ResourceRequest::Write(request) = request else {
            return Err(ResourceError::Invalid("expected write request".into()));
        };
        native_plugins::write(&request.uri, &request.text)
            .map(|_| ResourceReply::Written)
            .map_err(ResourceError::Provider)
    }
}

artist_plugin_sdk::unadvertised_lifecycle!(
    PluginsWrite,
    "artist.plugins.write",
    Capability::Resources
);
artist_plugin_sdk::unadvertised_tools!(PluginsWrite);
artist_plugin_sdk::unadvertised_slash_commands!(PluginsWrite);
artist_plugin_sdk::export!(PluginsWrite with_types_in artist_plugin_sdk);
