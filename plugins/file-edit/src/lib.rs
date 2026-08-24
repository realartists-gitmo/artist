use artist_plugin_sdk::artist::plugin::{native_filesystem, types::*};

struct FileEdit;

fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
    let ResourceRequest::Edit(request) = request else {
        return Err(ResourceError::Unsupported(RouteError {
            uri: "file:/".into(),
            operation: ResourceOperation::Edit,
        }));
    };
    native_filesystem::edit(&request)
        .map(|revision| ResourceReply::Edited(EditedReply { revision }))
}

artist_plugin_sdk::resource_component!(
    FileEdit,
    "artist.file.edit",
    ResourceOperation::Edit,
    handle
);
