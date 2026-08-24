use artist_plugin_sdk::artist::plugin::{native_filesystem, types::*};

struct FileRead;

fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
    let ResourceRequest::Read(request) = request else {
        return Err(unsupported());
    };
    native_filesystem::read(&request.uri, request.start_line, request.line_count)
        .map(ResourceReply::Text)
        .map_err(provider)
}

fn provider(message: String) -> ResourceError {
    ResourceError::Provider(message)
}

fn unsupported() -> ResourceError {
    ResourceError::Unsupported(RouteError {
        uri: "file:/".into(),
        operation: ResourceOperation::Read,
    })
}

artist_plugin_sdk::resource_component!(
    FileRead,
    "artist.file.read",
    ResourceOperation::Read,
    handle
);
