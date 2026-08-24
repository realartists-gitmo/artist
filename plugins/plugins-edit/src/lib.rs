use artist_plugin_sdk::artist::plugin::{native_plugins, types::*};

struct PluginsEdit;

impl artist_plugin_sdk::exports::artist::plugin::resource_provider::Guest for PluginsEdit {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Ok(vec![ResourceRoute {
            base_glob: "plugins:///**".into(),
            projection_glob: None,
            operations: vec![ResourceOperation::Edit],
            signals: Vec::new(),
        }])
    }

    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let ResourceRequest::Edit(request) = request else {
            return Err(ResourceError::Invalid("expected edit request".into()));
        };
        native_plugins::edit(&request)
            .map(|revision| ResourceReply::Edited(EditedReply { revision }))
    }
}

artist_plugin_sdk::unadvertised_lifecycle!(
    PluginsEdit,
    "artist.plugins.edit",
    Capability::Resources
);
artist_plugin_sdk::unadvertised_tools!(PluginsEdit);
artist_plugin_sdk::unadvertised_slash_commands!(PluginsEdit);
artist_plugin_sdk::export!(PluginsEdit with_types_in artist_plugin_sdk);
