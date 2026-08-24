use artist_plugin_sdk::artist::plugin::{native_profiles, types::*};

struct ProfilesRead;

impl artist_plugin_sdk::exports::artist::plugin::resource_provider::Guest for ProfilesRead {
    fn routes() -> Result<Vec<ResourceRoute>, String> {
        Ok(vec![ResourceRoute {
            base_glob: "profiles:///**".into(),
            projection_glob: None,
            operations: vec![ResourceOperation::Read],
            signals: Vec::new(),
        }])
    }

    fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let ResourceRequest::Read(request) = request else {
            return Err(ResourceError::Invalid("expected read request".into()));
        };
        native_profiles::read(&request.uri, request.start_line, request.line_count)
            .map(ResourceReply::Text)
            .map_err(ResourceError::Provider)
    }
}

artist_plugin_sdk::unadvertised_lifecycle!(
    ProfilesRead,
    "artist.profiles.read",
    Capability::Resources
);
artist_plugin_sdk::unadvertised_tools!(ProfilesRead);
artist_plugin_sdk::unadvertised_slash_commands!(ProfilesRead);
artist_plugin_sdk::export!(ProfilesRead with_types_in artist_plugin_sdk);
