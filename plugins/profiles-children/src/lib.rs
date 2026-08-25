use artist_plugin_sdk::artist::plugin::{native_profiles, types::*};

struct ProfilesChildren;

impl artist_plugin_sdk::exports::artist::plugin::resource_provider::Guest for ProfilesChildren {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Ok(vec![ResourceRoute {
            base_glob: "profiles:///**".into(),
            projection_glob: None,
            operations: vec![ResourceOperation::Children],
            signals: Vec::new(),
        }])
    }

    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let ResourceRequest::Children(uri) = request else {
            return Err(ResourceError::Invalid("expected children request".into()));
        };
        native_profiles::children(&uri)
            .map(ResourceReply::Children)
            .map_err(ResourceError::Provider)
    }
}

artist_plugin_sdk::unadvertised_lifecycle!(
    ProfilesChildren,
    "artist.profiles.children",
    Capability::Resources
);
artist_plugin_sdk::unadvertised_tools!(ProfilesChildren);
artist_plugin_sdk::unadvertised_slash_commands!(ProfilesChildren);
artist_plugin_sdk::unadvertised_model_provider!(ProfilesChildren);
artist_plugin_sdk::export!(ProfilesChildren with_types_in artist_plugin_sdk);
