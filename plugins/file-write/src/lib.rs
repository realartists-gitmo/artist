use artist_plugin_sdk::artist::plugin::{native_filesystem, types::*};

struct FileWrite;

fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
    let ResourceRequest::Write(request) = request else {
        return Err(unsupported());
    };
    native_filesystem::write(&request.uri, &request.text)
        .map(|_| ResourceReply::Written)
        .map_err(provider)
}

fn provider(message: String) -> ResourceError {
    ResourceError::Provider(message)
}

fn unsupported() -> ResourceError {
    ResourceError::Unsupported(RouteError {
        uri: "file:/".into(),
        operation: ResourceOperation::Write,
    })
}

artist_plugin_sdk::resource_component!(
    FileWrite,
    "artist.file.write",
    ResourceOperation::Write,
    handle
);
